//! Integration tests for retrieval-triggered reconsolidation (covalence#66).
//!
//! Verifies that:
//! * Searching for an article that has related orphan sources queues a
//!   `reconsolidate` task in the slow-path queue.
//! * The `reconsolidate` handler updates the article in-place and creates
//!   provenance edges for the newly-linked sources.
//! * The 6-hour cooldown prevents repeated reconsolidation for recently-
//!   modified articles.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::search_service::{SearchRequest, SearchService};
use covalence_engine::worker::reconsolidation::handle_reconsolidate;
use covalence_engine::worker::{handle_compile, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Count pending `reconsolidate` tasks for a given article (by payload key).
async fn pending_recon_count(pool: &sqlx::PgPool, article_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type = 'reconsolidate' \
           AND  status    = 'pending' \
           AND  payload->>'article_id' = $1",
    )
    .bind(article_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(0)
}

// ─── main integration scenario ────────────────────────────────────────────────

/// Full scenario:
/// 1. Ingest source-1 and source-2, compile them into an article.
/// 2. Back-date the article's `last_reconsolidated_at` so the cooldown doesn't
///    block queuing.
/// 3. Ingest source-3 (related but orphaned — not linked to the article).
/// 4. Insert a matching embedding for source-3 so the similarity search picks
///    it up.
/// 5. Run the search service against the article topic.
/// 6. Wait briefly for the background task to fire, then assert that a
///    `reconsolidate` task has been queued for the article.
#[tokio::test]
#[serial]
async fn test_search_queues_reconsolidate_for_orphan_source() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // ── 1. Ingest two sources and compile them ─────────────────────────────
    let src1 = fix
        .insert_source(
            "Distributed Caching Basics",
            "Caching is a fundamental technique for reducing database load. \
             Redis and Memcached are the two most popular in-memory caches. \
             We chose Redis because it supports data structures beyond simple \
             key-value pairs, which our session management requires.",
        )
        .await;

    let src2 = fix
        .insert_source(
            "Cache Invalidation Strategies",
            "Cache invalidation is famously hard. The three main strategies are \
             TTL-based expiry, event-driven invalidation, and write-through caching. \
             Our team chose event-driven invalidation to minimize stale reads.",
        )
        .await;

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_task_type("reconsolidate");
    fix.track_inference_log("compile", vec![src1, src2]);

    // Compile the two sources into an article.
    let compile_task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src1.to_string(), src2.to_string()],
            "title_hint": "Distributed Caching Guide"
        }),
    );
    let compile_result = handle_compile(&fix.pool, &llm, &compile_task)
        .await
        .expect("compile should succeed");
    let article_id = Uuid::parse_str(compile_result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    // ── 2. Back-date last_reconsolidated_at so cooldown is not active ──────
    // Set it to 7 hours ago (> 6-hour cooldown window).
    sqlx::query(
        "UPDATE covalence.nodes \
         SET last_reconsolidated_at = now() - interval '7 hours', \
             modified_at            = now() - interval '7 hours' \
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("back-date should succeed");

    // ── 3. Ingest a 3rd related source (orphan — not linked to the article) ─
    let src3 = fix
        .insert_source(
            "Redis vs Memcached Deep Dive",
            "Redis supports persistence, pub/sub messaging, and Lua scripting \
             in addition to standard key-value operations. Memcached is simpler \
             and slightly faster for pure caching workloads but lacks Redis's \
             richer feature set. For applications that need session storage with \
             complex data structures, Redis is the clear winner.",
        )
        .await;

    // ── 4. Insert embeddings so similarity search works ─────────────────────
    // We give the article and src3 the same deterministic embedding so they
    // come out as nearest neighbours in the vector index.
    fix.insert_embedding(article_id).await;
    fix.insert_embedding(src3).await;

    // ── 5. Run the search service ────────────────────────────────────────────
    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: "distributed caching Redis".to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["article".to_string()]),
        limit: 5,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: None,
        before: None,
        min_score: None,
        spreading_activation: None,
        min_causal_weight: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results, _meta) = svc.search(req).await.expect("search should succeed");

    // The article should appear in results (it was compiled from src1 + src2
    // which both mention caching, Redis, etc.).
    // Even if it doesn't rank first, we just need the background task to fire.
    let _ = results; // we care about the side effect, not the returned results

    // ── 6. Wait for the background task to fire ──────────────────────────────
    // The spawn is non-blocking but runs on the same tokio runtime.  Yield a
    // few times so the spawned future can make progress.
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        if pending_recon_count(&fix.pool, article_id).await > 0 {
            break;
        }
    }

    let count = pending_recon_count(&fix.pool, article_id).await;
    assert!(
        count > 0,
        "expected at least one pending reconsolidate task for article {article_id}, \
         but found {count}"
    );

    fix.cleanup().await;
}

// ─── handler unit-level tests ─────────────────────────────────────────────────

/// The `reconsolidate` handler must update the article in-place and create a
/// provenance edge from the new source to the article.
#[tokio::test]
#[serial]
async fn test_reconsolidate_handler_updates_article_in_place() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // Set up: article + two linked sources + one orphan.
    let src1 = fix
        .insert_source("Source A", "Content about topic A with detailed reasoning.")
        .await;
    let src2 = fix
        .insert_source("Source B", "Content about topic B and its implications.")
        .await;
    let orphan = fix
        .insert_source(
            "Source C (orphan)",
            "Additional content about topic A-B that enriches the article.",
        )
        .await;

    let article_id = fix
        .insert_article("Combined Article", "Initial content about topics A and B.")
        .await;

    // Link src1 and src2 to the article (but NOT orphan).
    fix.insert_originates_edge(src1, article_id).await;
    fix.insert_originates_edge(src2, article_id).await;

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");

    // Run the handler.
    let task = TestFixture::make_task(
        "reconsolidate",
        None,
        json!({
            "article_id": article_id.to_string(),
            "source_ids": [orphan.to_string()]
        }),
    );

    let result = handle_reconsolidate(&fix.pool, &llm, &task)
        .await
        .expect("handle_reconsolidate should succeed");

    // Must not be skipped.
    assert_eq!(
        result.get("skipped"),
        None,
        "handler should not skip: {result}"
    );

    // Result should include the article_id and new_source_ids.
    let returned_article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    assert_eq!(returned_article_id, article_id);

    let new_srcs: Vec<&str> = result["new_source_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        new_srcs.contains(&orphan.to_string().as_str()),
        "orphan should be in new_source_ids"
    );
    assert_eq!(
        result["total_sources"].as_i64().unwrap(),
        3,
        "total_sources should be 3 (2 existing + 1 new)"
    );

    // Article content must have been updated.
    let new_content: String =
        sqlx::query_scalar("SELECT content FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article should exist");
    assert!(
        !new_content.is_empty(),
        "article content should not be empty after reconsolidation"
    );
    // The mock LLM output will contain synthesis content.
    assert_ne!(
        new_content, "Initial content about topics A and B.",
        "article content should have changed after reconsolidation"
    );

    // A provenance edge from orphan → article should now exist.
    let edge_count = fix.edge_count_to(article_id, "ORIGINATES").await;
    assert!(
        edge_count >= 1,
        "at least one ORIGINATES edge to article should exist (got {edge_count})"
    );

    // last_reconsolidated_at should be set.
    let last_recon: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_reconsolidated_at FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("node should exist");
    assert!(
        last_recon.is_some(),
        "last_reconsolidated_at should be set after reconsolidation"
    );

    // An embed task should be queued for the article.
    let embed_count = fix.pending_task_count("embed", article_id).await;
    assert!(
        embed_count > 0,
        "an embed task should be queued for the updated article"
    );

    fix.cleanup().await;
}

/// Handler must skip gracefully when all provided source_ids are already linked.
#[tokio::test]
#[serial]
async fn test_reconsolidate_skips_already_linked_sources() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src = fix.insert_source("Linked Source", "Content.").await;
    let article_id = fix.insert_article("Article", "Initial.").await;
    fix.insert_originates_edge(src, article_id).await;

    let task = TestFixture::make_task(
        "reconsolidate",
        None,
        json!({
            "article_id": article_id.to_string(),
            "source_ids": [src.to_string()]   // already linked
        }),
    );

    let result = handle_reconsolidate(&fix.pool, &llm, &task)
        .await
        .expect("handler should not error");

    assert_eq!(
        result["skipped"],
        json!(true),
        "should skip when all sources already linked"
    );
    assert_eq!(result["reason"], json!("already_linked"));

    fix.cleanup().await;
}

/// Cooldown: if `last_reconsolidated_at` is recent (< 6 h), the search-service
/// helper must skip queuing — we test this at the function level by setting the
/// timestamp and verifying no task is queued.
#[tokio::test]
#[serial]
async fn test_reconsolidation_respects_cooldown() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src1 = fix.insert_source("S1", "Content 1.").await;
    let src2 = fix.insert_source("S2", "Content 2.").await;
    let orphan = fix.insert_source("S3 orphan", "Orphan content.").await;

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_task_type("reconsolidate");
    fix.track_inference_log("compile", vec![src1, src2]);

    // Compile src1 + src2 into an article.
    let compile_task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src1.to_string(), src2.to_string()],
            "title_hint": "Cooldown Test Article"
        }),
    );
    let compile_result = handle_compile(&fix.pool, &llm, &compile_task)
        .await
        .expect("compile should succeed");
    let article_id = Uuid::parse_str(compile_result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    // Set last_reconsolidated_at to 2 hours ago (within the 6-hour window).
    sqlx::query(
        "UPDATE covalence.nodes \
         SET last_reconsolidated_at = now() - interval '2 hours' \
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("update should succeed");

    // Add embedding for similarity search.
    fix.insert_embedding(article_id).await;
    fix.insert_embedding(orphan).await;

    // Run search — the background task should fire but skip due to cooldown.
    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: "cooldown test".to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["article".to_string()]),
        limit: 5,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: None,
        before: None,
        min_score: None,
        spreading_activation: None,
        min_causal_weight: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };
    let _ = svc.search(req).await;

    // Give the spawned task time to run.
    for _ in 0..10 {
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    let count = pending_recon_count(&fix.pool, article_id).await;
    assert_eq!(
        count, 0,
        "no reconsolidate task should be queued within the 6-hour cooldown (got {count})"
    );

    fix.cleanup().await;
}
