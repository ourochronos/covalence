//! Integration tests for the `decay_check` slow-path handler.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;

use covalence_engine::worker::{decay::handle_decay_check, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── basic decay score ────────────────────────────────────────────────────────

/// A freshly-inserted active node with no contentions and no inferred edges
/// should produce a decay score driven purely by edge-staleness (0.20) and
/// near-zero age — so the score should be between 0.0 and 0.25 inclusive.
#[tokio::test]
#[serial]
async fn decay_check_fresh_node_low_score() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_article("Fresh Node", "Fresh article content with no issues.")
        .await;

    let task = TestFixture::make_task("decay_check", Some(node_id), json!({}));
    let result = handle_decay_check(&fix.pool, &task)
        .await
        .expect("decay_check should succeed for a fresh node");

    assert_eq!(result["skipped"], json!(false));

    let score = result["decay_score"].as_f64().unwrap();
    // Edge staleness (no inferred edges) contributes 0.20.
    // Age for a just-created node is ~0.
    // Score ≈ 0.20 for a fresh node.
    assert!(
        score <= 0.25,
        "fresh node decay score should be ≤ 0.25; got {score}"
    );

    // Score must be persisted in metadata.
    let meta = fix.node_metadata(node_id).await;
    let stored_score: f64 = meta["decay_score"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| meta["decay_score"].as_f64())
        .expect("decay_score must be stored in metadata");
    assert!(
        (stored_score - score).abs() < 1e-9,
        "persisted score should match returned score"
    );

    fix.cleanup().await;
}

// ─── contention raises score ──────────────────────────────────────────────────

/// Adding an unresolved contention should increase the contention component
/// from 0 to 0.5 (1 open contention ÷ 2 = 0.50), raising the composite score.
#[tokio::test]
#[serial]
async fn decay_check_unresolved_contention_raises_score() {
    let mut fix = TestFixture::new().await;

    let article = fix
        .insert_article(
            "Contended Article",
            "This article has an unresolved contention.",
        )
        .await;
    let source = fix
        .insert_source("Contention Source", "Conflicting source content.")
        .await;
    fix.insert_contention(article, source, "medium", 0.6).await;

    let task = TestFixture::make_task("decay_check", Some(article), json!({}));
    let result = handle_decay_check(&fix.pool, &task)
        .await
        .expect("decay_check should succeed");

    let open_contentions = result["open_contentions"].as_i64().unwrap_or(0);
    assert_eq!(open_contentions, 1, "should detect the open contention");

    let contention_score = result["contention_score"].as_f64().unwrap();
    // 1 / (1+1) = 0.5
    assert!(
        (contention_score - 0.5).abs() < 1e-9,
        "contention_score for 1 open contention should be 0.5; got {contention_score}"
    );

    fix.cleanup().await;
}

// ─── inferred edges lower edge-staleness ─────────────────────────────────────

/// When the node already has at least one inferred outbound edge, the
/// edge-staleness component should be 0.0.
#[tokio::test]
#[serial]
async fn decay_check_inferred_edge_reduces_staleness() {
    let mut fix = TestFixture::new().await;

    let source = fix
        .insert_article(
            "Inferred Edge Source",
            "This article has an inferred outbound edge.",
        )
        .await;
    let target = fix.insert_article("Edge Target", "Target article.").await;

    // Insert one inferred edge.
    sqlx::query(
        "INSERT INTO covalence.edges \
             (id, source_node_id, target_node_id, edge_type, weight, metadata) \
         VALUES (gen_random_uuid(), $1, $2, 'RELATES_TO', 0.8, \
                 '{\"inferred\":true}'::jsonb)",
    )
    .bind(source)
    .bind(target)
    .execute(&fix.pool)
    .await
    .expect("insert inferred edge");

    let task = TestFixture::make_task("decay_check", Some(source), json!({}));
    let result = handle_decay_check(&fix.pool, &task)
        .await
        .expect("decay_check should succeed");

    let inferred = result["inferred_edges"].as_i64().unwrap_or(0);
    assert_eq!(inferred, 1, "should detect the inferred edge");

    let edge_staleness = result["edge_staleness"].as_f64().unwrap();
    assert_eq!(
        edge_staleness, 0.0,
        "edge_staleness should be 0.0 when an inferred edge exists"
    );

    // Score should be lower without the 0.20 staleness penalty.
    let score = result["decay_score"].as_f64().unwrap();
    assert!(
        score < 0.20,
        "score should be below 0.20 without edge staleness; got {score}"
    );

    fix.cleanup().await;
}

// ─── archived node is skipped ────────────────────────────────────────────────

/// Archived nodes must be skipped with `skipped = true`.
#[tokio::test]
#[serial]
async fn decay_check_archived_node_skipped() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_article("Archived Node", "Will be archived.")
        .await;

    // Manually archive the node.
    sqlx::query("UPDATE covalence.nodes SET status = 'archived' WHERE id = $1")
        .bind(node_id)
        .execute(&fix.pool)
        .await
        .expect("archive node");

    let task = TestFixture::make_task("decay_check", Some(node_id), json!({}));
    let result = handle_decay_check(&fix.pool, &task)
        .await
        .expect("decay_check should succeed even for archived node");

    assert_eq!(result["skipped"], json!(true));
    assert_eq!(result["reason"].as_str().unwrap(), "archived");
    assert_eq!(
        result["decay_score"],
        json!(null),
        "decay_score should be null for archived nodes"
    );

    fix.cleanup().await;
}

// ─── recompile task queued at threshold ──────────────────────────────────────

/// When we manipulate the node so the score is guaranteed to exceed the
/// recompile threshold (≥ 0.70), a `compile` task must be queued.
///
/// Strategy: fake the `modified_at` to 400 days ago so age_score = 1.0,
/// add a contention for contention_score = 0.5, and ensure no inferred
/// edges (edge_staleness = 1.0).
/// Composite = 0.5·1 + 0.3·0.5 + 0.2·1 = 0.5 + 0.15 + 0.2 = 0.85 ≥ 0.70 ✓
#[tokio::test]
#[serial]
async fn decay_check_queues_recompile_when_above_threshold() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_article(
            "Stale Article",
            "This article is very stale and needs recompiling.",
        )
        .await;

    // Age the node by 400 days.
    sqlx::query(
        "UPDATE covalence.nodes \
         SET modified_at = now() - interval '400 days' \
         WHERE id = $1",
    )
    .bind(node_id)
    .execute(&fix.pool)
    .await
    .expect("age the node");

    // Add an unresolved contention.
    let source = fix
        .insert_source("Decay Source", "Source for decay contention.")
        .await;
    fix.insert_contention(node_id, source, "high", 0.9).await;
    fix.track_task_type("compile");

    let task = TestFixture::make_task("decay_check", Some(node_id), json!({}));
    let result = handle_decay_check(&fix.pool, &task)
        .await
        .expect("decay_check should succeed");

    let score = result["decay_score"].as_f64().unwrap();
    assert!(
        score >= covalence_engine::worker::decay::RECOMPILE_THRESHOLD,
        "score {score} should be ≥ RECOMPILE_THRESHOLD"
    );

    assert_eq!(
        result["recompile_queued"],
        json!(true),
        "recompile_queued should be true when above threshold"
    );

    // compile task should now be in the queue.
    assert_eq!(
        fix.pending_task_count("compile", node_id).await,
        1,
        "compile task should be queued for the stale article"
    );

    fix.cleanup().await;
}

// ─── recompile task NOT queued below threshold ───────────────────────────────

/// A fresh article (score ≈ 0.20) must NOT have a compile task queued.
#[tokio::test]
#[serial]
async fn decay_check_no_recompile_below_threshold() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_article(
            "Fresh Healthy Article",
            "Content that does not need recompile.",
        )
        .await;

    let task = TestFixture::make_task("decay_check", Some(node_id), json!({}));
    let result = handle_decay_check(&fix.pool, &task)
        .await
        .expect("decay_check should succeed");

    assert_eq!(
        result["recompile_queued"],
        json!(false),
        "recompile_queued should be false for a healthy node"
    );

    assert_eq!(
        fix.pending_task_count("compile", node_id).await,
        0,
        "compile task should NOT be queued for a healthy node"
    );

    fix.cleanup().await;
}

// ─── score is deterministic ───────────────────────────────────────────────────

/// Calling `decay_check` twice on the same node (without changing any data)
/// must return the same score.
#[tokio::test]
#[serial]
async fn decay_check_score_is_deterministic() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_article("Deterministic Node", "Stable content.")
        .await;

    let task1 = TestFixture::make_task("decay_check", Some(node_id), json!({}));
    let r1 = handle_decay_check(&fix.pool, &task1)
        .await
        .expect("first decay_check");
    let score1 = r1["decay_score"].as_f64().unwrap();

    // Wait a tick to make sure `modified_at` doesn't drift.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let task2 = TestFixture::make_task("decay_check", Some(node_id), json!({}));
    let r2 = handle_decay_check(&fix.pool, &task2)
        .await
        .expect("second decay_check");
    let score2 = r2["decay_score"].as_f64().unwrap();

    // Scores may differ by at most 1 second of age drift (< 0.001 difference).
    assert!(
        (score1 - score2).abs() < 0.01,
        "repeated decay_check scores should be essentially identical; got {score1} vs {score2}"
    );

    fix.cleanup().await;
}

// ─── missing node returns error ───────────────────────────────────────────────

/// Attempting `decay_check` on a non-existent node_id must return an error.
#[tokio::test]
#[serial]
async fn decay_check_missing_node_returns_error() {
    let fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    let _ = llm; // satisfy import

    let missing_id = uuid::Uuid::new_v4();
    let task = TestFixture::make_task("decay_check", Some(missing_id), json!({}));
    let result = handle_decay_check(&fix.pool, &task).await;

    assert!(
        result.is_err(),
        "decay_check on missing node should return Err"
    );

    fix.cleanup().await;
}
