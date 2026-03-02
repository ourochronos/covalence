//! Integration tests for the `compile` slow-path handler.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use sqlx::Row;
use uuid::Uuid;

use covalence_engine::worker::{handle_compile, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── happy path ───────────────────────────────────────────────────────────────

/// Compiling two source nodes should create a new `article` node that is
/// `active`, has provenance edges back to both sources, and has a pending
/// `embed` task queued.
#[tokio::test]
#[serial]
async fn compile_creates_article_from_two_sources() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src_a = fix
        .insert_source(
            "Source Alpha",
            "Alpha content about machine learning and gradient descent.",
        )
        .await;
    let src_b = fix
        .insert_source(
            "Source Beta",
            "Beta content about neural network architectures.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src_a, src_b]);

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src_a.to_string(), src_b.to_string()],
            "title_hint": "ML Overview"
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed");

    // ── Result shape ──────────────────────────────────────────────────────────
    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap())
        .expect("article_id should be a valid UUID");
    fix.track(article_id);

    assert_eq!(result["degraded"], json!(false), "should not be degraded");
    assert_eq!(result["source_count"], json!(2));

    // ── Article node ──────────────────────────────────────────────────────────
    let row = sqlx::query(
        "SELECT node_type, status, title, content \
         FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .expect("article node should exist");

    assert_eq!(row.get::<String, _>("node_type"), "article");
    assert_eq!(row.get::<String, _>("status"), "active");
    assert!(
        !row.get::<String, _>("title").is_empty(),
        "article should have a title"
    );

    // ── Provenance edges ──────────────────────────────────────────────────────
    let edge_count = fix.edge_count_to(article_id, "ORIGINATES").await
        + fix.edge_count_to(article_id, "COMPILED_FROM").await;
    assert_eq!(
        edge_count, 2,
        "both sources should be linked via provenance edges"
    );

    // ── Follow-up embed task ──────────────────────────────────────────────────
    assert_eq!(
        fix.pending_task_count("embed", article_id).await,
        1,
        "one pending embed task should be queued for the new article"
    );

    fix.cleanup().await;
}

/// A single-source compile should produce a valid article.
#[tokio::test]
#[serial]
async fn compile_single_source() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src = fix
        .insert_source("Solo Source", "Solo source content for compiling.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src]);

    let task = TestFixture::make_task("compile", None, json!({ "source_ids": [src.to_string()] }));

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("single-source compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    assert_eq!(fix.node_status(article_id).await, "active");
    assert_eq!(result["source_count"], json!(1));

    fix.cleanup().await;
}

// ─── LLM fallback / degraded ─────────────────────────────────────────────────

/// When the LLM always fails, `handle_compile` falls back to a degraded
/// article whose content is the concatenation of all source contents.
#[tokio::test]
#[serial]
async fn compile_fallback_on_llm_error() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::always_fail());

    let src_a = fix
        .insert_source("Fallback A", "First source content.")
        .await;
    let src_b = fix
        .insert_source("Fallback B", "Second source content.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src_a.to_string(), src_b.to_string()],
            "title_hint": "Fallback Article"
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("compile should succeed in degraded mode when LLM fails");

    assert_eq!(
        result["degraded"],
        json!(true),
        "result should be marked degraded"
    );

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    let content = fix.node_content(article_id).await;
    assert!(
        content.contains("First source content."),
        "degraded content should include source A text"
    );
    assert!(
        content.contains("Second source content."),
        "degraded content should include source B text"
    );

    fix.cleanup().await;
}

// ─── dedup ────────────────────────────────────────────────────────────────────

/// When a vector-similar article already exists, `compile` should return its
/// id rather than creating a duplicate.
#[tokio::test]
#[serial]
async fn compile_deduplicates_against_existing_article() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // The mock's LLM response for compile is fixed; pre-insert an article with
    // the exact same content the mock would return and embed it with the same
    // deterministic vector.
    let existing_content = "This article synthesizes the provided source documents into a coherent knowledge unit. It covers the key facts and relationships described across the source material.";
    let existing_id = fix
        .insert_article("Existing Article", existing_content)
        .await;

    // Insert embedding for the existing article using the deterministic vector.
    let emb = MockLlmClient::deterministic_embedding(existing_content);
    let dims = emb.len();
    let vec_literal = format!(
        "[{}]",
        emb.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    sqlx::query(&format!(
        "INSERT INTO covalence.node_embeddings (node_id, embedding, model) \
         VALUES ($1, '{vec_literal}'::halfvec({dims}), 'test-mock') \
         ON CONFLICT (node_id) DO NOTHING"
    ))
    .bind(existing_id)
    .execute(&fix.pool)
    .await
    .expect("pre-insert embedding for existing article");

    let src = fix
        .insert_source("Dedup Source", "Content about knowledge synthesis.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src]);

    let task = TestFixture::make_task("compile", None, json!({ "source_ids": [src.to_string()] }));
    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("compile should succeed");

    let returned_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        returned_id, existing_id,
        "compile should return the existing article id when content is deduped"
    );

    fix.cleanup().await;
}

// ─── idempotency guard ────────────────────────────────────────────────────────

/// When an identical `compile` task is already `complete` in the queue,
/// the handler should skip and return `{"skipped": true, "reason": "already_complete"}`.
#[tokio::test]
#[serial]
async fn compile_idempotency_guard() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src = fix
        .insert_source("Idempotent Source", "Idempotent content.")
        .await;
    let payload = json!({ "source_ids": [src.to_string()] });

    // Pre-insert a completed task row.
    let guard_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
         (id, task_type, node_id, payload, status, priority) \
         VALUES ($1, 'compile', NULL, $2, 'complete', 0)",
    )
    .bind(guard_id)
    .bind(&payload)
    .execute(&fix.pool)
    .await
    .expect("insert guard task");

    let task = TestFixture::make_task("compile", None, payload.clone());
    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("compile should return a skip result");

    assert_eq!(result["skipped"], json!(true));
    assert_eq!(result["reason"], json!("already_complete"));

    // Clean up the guard row.
    sqlx::query("DELETE FROM covalence.slow_path_queue WHERE id = $1")
        .bind(guard_id)
        .execute(&fix.pool)
        .await
        .ok();

    fix.cleanup().await;
}

// ─── inference log ───────────────────────────────────────────────────────────

/// A successful (non-degraded) compile should write exactly one row to
/// `covalence.inference_log` with `operation = 'compile'`.
#[tokio::test]
#[serial]
async fn compile_logs_inference() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src_a = fix
        .insert_source("Log Source A", "Inference log source A content.")
        .await;
    let src_b = fix
        .insert_source("Log Source B", "Inference log source B content.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src_a, src_b]);

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({ "source_ids": [src_a.to_string(), src_b.to_string()] }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    let log_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.inference_log \
         WHERE operation = 'compile' AND input_node_ids @> $1",
    )
    .bind(&vec![src_a, src_b])
    .fetch_one(&fix.pool)
    .await
    .unwrap();

    assert_eq!(
        log_count, 1,
        "compile should write exactly one inference_log row"
    );

    fix.cleanup().await;
}
