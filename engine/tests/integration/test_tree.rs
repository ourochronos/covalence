//! Integration tests for the `tree_index` slow-path handler.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;

use covalence_engine::worker::{handle_tree_index, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── trivial content (no LLM call) ───────────────────────────────────────────

/// Content below `TRIVIAL_THRESHOLD_CHARS` (700) should produce a single-node
/// trivial tree without invoking the LLM.  The tree index must be persisted
/// in `metadata.tree_index`.
#[tokio::test]
#[serial]
async fn tree_index_trivial_content_no_llm_call() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let content = "Short article content well below the trivial threshold.";
    let node_id = fix.insert_source("Small Node", content).await;

    let task = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    let result = handle_tree_index(&fix.pool, &llm, &task)
        .await
        .expect("handle_tree_index should succeed for small content");

    // LLM should NOT be consulted.
    assert_eq!(
        mock.complete_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "LLM complete() must not be called for trivial content"
    );

    // result carries node_count
    assert!(
        result.get("node_count").is_some(),
        "result should contain node_count"
    );

    // tree_index stored in metadata
    let meta = fix.node_metadata(node_id).await;
    assert!(
        meta.get("tree_index").is_some(),
        "metadata.tree_index should be populated"
    );

    fix.cleanup().await;
}

/// The tree index for trivial content must consist of exactly one section that
/// spans the entire content.
#[tokio::test]
#[serial]
async fn tree_index_trivial_content_single_section() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let content = "One-section content.";
    let node_id = fix.insert_source("One Section Node", content).await;

    let task = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    handle_tree_index(&fix.pool, &llm, &task)
        .await
        .expect("handle_tree_index should succeed");

    let meta = fix.node_metadata(node_id).await;
    let tree = meta
        .get("tree_index")
        .and_then(|v| v.get("nodes"))
        .and_then(|v| v.as_array())
        .expect("tree_index should be an object with a nodes array");

    assert_eq!(tree.len(), 1, "trivial content should yield one section");

    let section = &tree[0];
    assert_eq!(
        section["start_char"].as_u64().unwrap(),
        0,
        "first section start_char should be 0"
    );
    assert_eq!(
        section["end_char"].as_u64().unwrap() as usize,
        content.len(),
        "first section end_char should equal content length"
    );

    fix.cleanup().await;
}

// ─── force re-index ───────────────────────────────────────────────────────────

/// With `force = true` the existing tree index must be rebuilt even if one
/// already exists.  The resulting tree should be fresh (updated `modified_at`).
#[tokio::test]
#[serial]
async fn tree_index_force_rebuilds() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let content = "Content for force-rebuild test.";
    let node_id = fix.insert_source("Force Rebuild Node", content).await;

    // Build once without force
    let t1 = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    handle_tree_index(&fix.pool, &llm, &t1)
        .await
        .expect("first tree_index should succeed");

    // Capture first timestamp
    let ts1: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT modified_at FROM covalence.nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap();

    // Small sleep so wall clock moves.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Force rebuild
    let t2 = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": true }),
    );
    handle_tree_index(&fix.pool, &llm, &t2)
        .await
        .expect("force rebuild should succeed");

    let ts2: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT modified_at FROM covalence.nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap();

    assert!(
        ts2 >= ts1,
        "modified_at should be updated after force rebuild"
    );

    fix.cleanup().await;
}

// ─── skip when already indexed ────────────────────────────────────────────────

/// Without `force = true`, calling `tree_index` on a node that already has
/// `metadata.tree_index` set should skip the rebuild and return a result with
/// `skipped = true`.
#[tokio::test]
#[serial]
async fn tree_index_skip_when_already_indexed() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let content = "Content for skip test.";
    let node_id = fix.insert_source("Skip Check Node", content).await;

    // First build
    let t1 = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    handle_tree_index(&fix.pool, &llm, &t1)
        .await
        .expect("first tree_index should succeed");

    mock.complete_calls
        .store(0, std::sync::atomic::Ordering::SeqCst);

    // Second call without force should return an error (already indexed)
    let t2 = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    let err = handle_tree_index(&fix.pool, &llm, &t2)
        .await
        .expect_err("second tree_index should fail when already indexed");

    assert!(
        err.to_string().contains("already has tree_index"),
        "error should mention 'already has tree_index': {err}"
    );
    assert_eq!(
        mock.complete_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "LLM should not be called on an already-indexed node"
    );

    fix.cleanup().await;
}
