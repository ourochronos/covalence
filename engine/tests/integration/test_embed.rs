//! Integration tests for the `embed` and `tree_embed` slow-path handlers.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;

use covalence_engine::worker::{
    handle_embed, handle_tree_embed, handle_tree_index, llm::LlmClient,
};

use super::helpers::{MockLlmClient, TestFixture};

// ─── embed: small node ────────────────────────────────────────────────────────

/// A small node (≤ TRIVIAL_THRESHOLD_CHARS) is embedded directly without
/// invoking the tree pipeline.  The resulting row must appear in
/// `covalence.node_embeddings` and the returned JSON must carry the
/// expected `node_id` and `dimensions` fields.
#[tokio::test]
#[serial]
async fn embed_small_node_stores_embedding() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let node_id = fix
        .insert_source(
            "Small Embed Node",
            "Short content for direct embedding test.",
        )
        .await;
    fix.track_task_type("embed");

    let task = TestFixture::make_task("embed", Some(node_id), json!({}));
    let result = handle_embed(&fix.pool, &llm, &task)
        .await
        .expect("handle_embed should succeed for small node");

    // Result shape
    assert_eq!(
        result["node_id"].as_str().unwrap(),
        node_id.to_string(),
        "result.node_id mismatch"
    );
    assert_eq!(
        result["dimensions"],
        json!(1536),
        "mock embed should produce 1536-dim vector"
    );

    // DB row
    assert!(
        fix.embedding_exists(node_id).await,
        "embedding row should exist in node_embeddings"
    );

    fix.cleanup().await;
}

/// Running `embed` twice on the same node must be idempotent: the second call
/// should succeed and the row count in `node_embeddings` should still be 1.
#[tokio::test]
#[serial]
async fn embed_idempotent() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let node_id = fix
        .insert_source("Idempotent Embed Node", "Some stable content.")
        .await;
    fix.track_task_type("embed");

    for _ in 0..2 {
        let task = TestFixture::make_task("embed", Some(node_id), json!({}));
        handle_embed(&fix.pool, &llm, &task)
            .await
            .expect("handle_embed should succeed on repeated call");
    }

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM covalence.node_embeddings WHERE node_id = $1")
            .bind(node_id)
            .fetch_one(&fix.pool)
            .await
            .unwrap();

    assert_eq!(
        count, 1,
        "ON CONFLICT should keep exactly one embedding row"
    );

    fix.cleanup().await;
}

// ─── embed: large node delegates to tree pipeline ────────────────────────────

/// Content exceeding `TRIVIAL_THRESHOLD_CHARS` (700) must be routed through
/// the tree-index pipeline.  After the call a node-level embedding must exist
/// and `metadata.tree_index` must be populated.
#[tokio::test]
#[serial]
async fn embed_large_node_delegates_to_tree_pipeline() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    // 1 000-char content — well above the 700-char threshold.
    let content = "A detailed paragraph about machine learning. ".repeat(23);
    assert!(content.len() > 700, "content must exceed threshold");

    let node_id = fix.insert_source("Large Embed Node", &content).await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task("embed", Some(node_id), json!({}));
    handle_embed(&fix.pool, &llm, &task)
        .await
        .expect("handle_embed should succeed for large content");

    // A node-level embedding should have been composed from the tree sections.
    assert!(
        fix.embedding_exists(node_id).await,
        "node embedding should exist after tree-pipeline embed"
    );

    // `metadata.tree_index` should be set.
    let meta = fix.node_metadata(node_id).await;
    assert!(
        meta.get("tree_index").is_some(),
        "metadata.tree_index must be present after large-node embed"
    );

    fix.cleanup().await;
}

// ─── tree_embed ───────────────────────────────────────────────────────────────

/// `handle_tree_embed` must embed each section from a pre-built tree index and
/// compose a node-level embedding.  An embedding row must appear in
/// `node_embeddings` after the call.
#[tokio::test]
#[serial]
async fn tree_embed_stores_node_embedding() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let content = "Content for tree embed test. This covers topic A and topic B.";
    let node_id = fix.insert_source("Tree Embed Node", content).await;
    fix.track_task_type("tree_embed");

    // First build the tree index (required before tree_embed).
    let ti_task = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    handle_tree_index(&fix.pool, &llm, &ti_task)
        .await
        .expect("tree_index prerequisite should succeed");

    // Now embed the sections.
    let te_task = TestFixture::make_task("tree_embed", Some(node_id), json!({}));
    handle_tree_embed(&fix.pool, &llm, &te_task)
        .await
        .expect("handle_tree_embed should succeed");

    assert!(
        fix.embedding_exists(node_id).await,
        "node embedding should be stored after tree_embed"
    );

    fix.cleanup().await;
}

/// covalence#70 — `tree_embed` (and therefore `embed_sections`) must **not**
/// fail when a node has no `tree_index` in its metadata.  This is the common
/// case for sources ingested via the `source_ingest` API.
///
/// Expected behaviour: the handler auto-builds a tree index, stores it in
/// metadata, and then embeds the sections — all without returning an error.
#[tokio::test]
#[serial]
async fn tree_embed_succeeds_without_prior_tree_index() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // Insert a source with no tree_index in metadata (raw ingestion path).
    let content = "A source ingested via the API that never went through the tree_index pipeline.";
    let node_id = fix.insert_source("No Tree Index Node", content).await;
    fix.track_task_type("tree_embed");

    // Verify precondition: no tree_index in metadata yet.
    let meta_before = fix.node_metadata(node_id).await;
    assert!(
        meta_before.get("tree_index").is_none(),
        "precondition: node must not have tree_index before the test"
    );

    // The handler must not error even though tree_index is absent.
    let te_task = TestFixture::make_task("tree_embed", Some(node_id), json!({}));
    handle_tree_embed(&fix.pool, &llm, &te_task)
        .await
        .expect("handle_tree_embed must succeed even when tree_index is missing (covalence#70)");

    // tree_index must now be present in metadata (stored by the on-the-fly build).
    let meta_after = fix.node_metadata(node_id).await;
    assert!(
        meta_after.get("tree_index").is_some(),
        "metadata.tree_index must be populated after auto-build"
    );

    // A node-level embedding must have been composed.
    assert!(
        fix.embedding_exists(node_id).await,
        "node embedding must exist after tree_embed with auto-built tree index"
    );

    fix.cleanup().await;
}

/// `tree_embed` called on a node that already has a tree index must not create
/// duplicate embedding rows.
#[tokio::test]
#[serial]
async fn tree_embed_idempotent() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let content = "Stable section content for idempotency check, long enough to exceed the minimum section length.";
    let node_id = fix.insert_source("Idem Tree Embed Node", content).await;
    fix.track_task_type("tree_embed");

    let ti_task = TestFixture::make_task(
        "tree_index",
        Some(node_id),
        json!({ "overlap": 0.1, "force": false }),
    );
    handle_tree_index(&fix.pool, &llm, &ti_task)
        .await
        .expect("tree_index should succeed");

    for _ in 0..2 {
        let te_task = TestFixture::make_task("tree_embed", Some(node_id), json!({}));
        handle_tree_embed(&fix.pool, &llm, &te_task)
            .await
            .expect("tree_embed should succeed on repeat");
    }

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM covalence.node_embeddings WHERE node_id = $1")
            .bind(node_id)
            .fetch_one(&fix.pool)
            .await
            .unwrap();

    assert_eq!(
        count, 1,
        "ON CONFLICT should keep exactly one embedding row"
    );

    fix.cleanup().await;
}
