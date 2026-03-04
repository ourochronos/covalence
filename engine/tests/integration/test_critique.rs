//! Integration tests for the Reflexion-style critique loop (covalence#105).
//!
//! Verifies:
//! 1. After `handle_compile` runs, a `critique_article` task is enqueued for
//!    the produced article (with `execute_after` set ~1 hour in the future).
//! 2. When `handle_critique_article` runs, a `CRITIQUES` edge is created from
//!    the new critique source node to the article.
//! 3. A low-quality critique (overall_quality < 0.6 + recommendation = "recompile"
//!    + consolidation_count < 5) triggers an immediate `consolidate_article` task.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::{critique::handle_critique_article, handle_compile, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── Test 1: handle_compile enqueues a deferred critique_article task ─────────

/// After `handle_compile` succeeds, a `critique_article` task must appear in
/// `slow_path_queue` for the produced article, with `status = 'pending'` and
/// `execute_after` approximately 1 hour in the future.
#[tokio::test]
#[serial]
async fn test_compile_enqueues_critique_task() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_task_type("critique_article");
    fix.track_inference_log("compile", vec![]);

    let src_a = fix
        .insert_source(
            "Critique Test Source A",
            "Content about distributed systems.",
        )
        .await;
    let src_b = fix
        .insert_source(
            "Critique Test Source B",
            "Content about consensus algorithms.",
        )
        .await;

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src_a.to_string(), src_b.to_string()],
            "title_hint": "Distributed Consensus Overview"
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap())
        .expect("article_id should be a valid UUID");
    fix.track(article_id);

    // A critique_article task must exist for this article.
    let critique_task: Option<(String, Option<chrono::DateTime<Utc>>)> = sqlx::query_as(
        "SELECT status, execute_after
         FROM   covalence.slow_path_queue
         WHERE  task_type = 'critique_article'
           AND  node_id   = $1
         LIMIT 1",
    )
    .bind(article_id)
    .fetch_optional(&fix.pool)
    .await
    .expect("queue query should succeed");

    assert!(
        critique_task.is_some(),
        "a critique_article task must be enqueued for article {article_id}"
    );

    let (status, execute_after) = critique_task.unwrap();
    assert_eq!(
        status, "pending",
        "critique_article task must be in pending state"
    );

    // The task should be delayed (execute_after > now), since critique runs
    // 1 hour after compilation to let embeddings settle.
    assert!(
        execute_after.is_some(),
        "critique_article task should have execute_after set"
    );
    assert!(
        execute_after.unwrap() > Utc::now(),
        "execute_after must be in the future (deferred 1 hour)"
    );

    fix.cleanup().await;
}

// ─── Test 2: handle_critique_article creates a CRITIQUES edge ─────────────────

/// When `handle_critique_article` runs successfully it must:
/// - Create a new `source` node (type = observation) with `metadata.critique = true`.
/// - Create a `CRITIQUES` edge from the critique source to the article.
#[tokio::test]
#[serial]
async fn test_critique_creates_critiques_edge() {
    let mut fix = TestFixture::new().await;

    // Use a fixed LLM response that returns an "accept" critique.
    let critique_response = json!({
        "overall_quality": 0.82,
        "issues": ["Minor gap in coverage of edge cases"],
        "recommendation": "accept"
    });
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::with_fixed_response(
        critique_response.to_string(),
    ));

    let src = fix
        .insert_source("Edge Test Source", "Content about graph edge semantics.")
        .await;

    let article_id = fix
        .insert_article(
            "Edge Semantics Article",
            "A comprehensive article about graph edge semantics and traversal.",
        )
        .await;

    fix.insert_originates_edge(src, article_id).await;

    // Build an in-memory task with node_id = article_id.
    let task = TestFixture::make_task("critique_article", Some(article_id), json!({}));

    let result = handle_critique_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_critique_article should succeed");

    // Validate result shape.
    assert!(
        result.get("skipped").is_none(),
        "handler must not skip an active article: {result}"
    );
    assert_eq!(
        result["recommendation"].as_str(),
        Some("accept"),
        "recommendation in result must match LLM output"
    );

    let critique_source_id = Uuid::parse_str(result["critique_source_id"].as_str().unwrap_or(""))
        .expect("critique_source_id must be a valid UUID");
    fix.track(critique_source_id);

    // The critique source must exist with correct metadata.
    let meta: serde_json::Value =
        sqlx::query_scalar("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(critique_source_id)
            .fetch_one(&fix.pool)
            .await
            .expect("critique source node must exist");

    assert_eq!(
        meta["critique"].as_bool(),
        Some(true),
        "critique source metadata.critique must be true: {meta}"
    );
    assert_eq!(
        meta["article_id"].as_str(),
        Some(article_id.to_string().as_str()),
        "critique source metadata.article_id must match: {meta}"
    );
    assert_eq!(
        meta["recommendation"].as_str(),
        Some("accept"),
        "critique source metadata.recommendation must be 'accept': {meta}"
    );

    // The CRITIQUES edge must exist: critique_source → article.
    let edge_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.edges \
         WHERE source_node_id = $1 \
           AND target_node_id = $2 \
           AND edge_type      = 'CRITIQUES'",
    )
    .bind(critique_source_id)
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        edge_count, 1,
        "exactly one CRITIQUES edge must exist from critique source to article"
    );

    // No recompile should have been triggered (recommendation = accept).
    assert_eq!(
        result["triggered_recompile"].as_bool(),
        Some(false),
        "accept recommendation must not trigger recompile"
    );

    fix.cleanup().await;
}

// ─── Test 3: low-quality article triggers consolidate_article re-queue ─────────

/// When the LLM returns `overall_quality < 0.6` AND `recommendation = "recompile"`
/// AND the article's `consolidation_count < 5`, the handler must enqueue an
/// immediate `consolidate_article` task.
#[tokio::test]
#[serial]
async fn test_critique_low_quality_triggers_recompile() {
    let mut fix = TestFixture::new().await;

    // Fixed LLM response: low quality + recompile recommendation.
    let low_quality_response = json!({
        "overall_quality": 0.35,
        "issues": [
            "Article is missing key context about Byzantine fault tolerance",
            "Several claims are unsupported by the source material",
            "Contradicts source B on consensus round complexity"
        ],
        "recommendation": "recompile"
    });
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::with_fixed_response(
        low_quality_response.to_string(),
    ));

    fix.track_task_type("consolidate_article");

    let src = fix
        .insert_source(
            "Consensus Source",
            "Detailed content about Paxos and Raft consensus algorithms.",
        )
        .await;

    let article_id = fix
        .insert_article(
            "Consensus Algorithms Article",
            "An article about consensus algorithms with some gaps.",
        )
        .await;

    fix.insert_originates_edge(src, article_id).await;

    // Set consolidation_count = 2 (below the MAX_AUTO_RECOMPILE_COUNT of 5).
    sqlx::query("UPDATE covalence.nodes SET consolidation_count = 2 WHERE id = $1")
        .bind(article_id)
        .execute(&fix.pool)
        .await
        .expect("consolidation_count update should succeed");

    let task = TestFixture::make_task("critique_article", Some(article_id), json!({}));

    let result = handle_critique_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_critique_article should succeed");

    let critique_source_id = Uuid::parse_str(result["critique_source_id"].as_str().unwrap_or(""))
        .expect("critique_source_id must be a valid UUID");
    fix.track(critique_source_id);

    // Validate the handler reported a recompile trigger.
    assert_eq!(
        result["triggered_recompile"].as_bool(),
        Some(true),
        "low quality + recompile recommendation must trigger recompile: {result}"
    );
    assert_eq!(
        result["recommendation"].as_str(),
        Some("recompile"),
        "recommendation must be 'recompile': {result}"
    );

    // A consolidate_article task must now exist for this article.
    // Pass should be consolidation_count + 1 = 3.
    let recompile_task_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.slow_path_queue
         WHERE  task_type               = 'consolidate_article'
           AND  payload->>'article_id'  = $1
           AND  (payload->>'pass')::int = 3
           AND  status                  = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        recompile_task_count > 0,
        "a consolidate_article pass-3 task must be enqueued after low-quality critique \
         (consolidation_count=2 → next pass=3)"
    );

    // CRITIQUES edge must also exist (created regardless of recommendation).
    let edge_count = fix.edge_count_from(critique_source_id, "CRITIQUES").await;
    assert_eq!(
        edge_count, 1,
        "CRITIQUES edge must exist even when recompile is triggered"
    );

    fix.cleanup().await;
}
