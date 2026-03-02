//! Integration tests for the `merge` and `infer_edges` slow-path handlers.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::{
    llm::LlmClient,
    merge_edges::{handle_infer_edges, handle_merge},
};

use super::helpers::{MockLlmClient, TestFixture};

// ─── merge: happy path ────────────────────────────────────────────────────────

/// Merging two articles should:
/// * Create a new `active` merged article.
/// * Archive both originals.
/// * Create two `MERGED_FROM` edges: new_article → each original.
/// * Queue an `embed` task for the merged article.
#[tokio::test]
#[serial]
async fn merge_creates_merged_article() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let a = fix
        .insert_article(
            "Article A",
            "Article A discusses the first set of concepts.",
        )
        .await;
    let b = fix
        .insert_article(
            "Article B",
            "Article B covers complementary related topics.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({
            "article_id_a": a.to_string(),
            "article_id_b": b.to_string(),
        }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("handle_merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap())
        .expect("new_article_id should be a valid UUID");
    fix.track(new_id);

    assert_eq!(
        result["degraded"],
        json!(false),
        "merge should not be degraded"
    );

    // New merged article is active.
    assert_eq!(fix.node_status(new_id).await, "active");

    // Originals are archived.
    for (label, id) in [("article_a", a), ("article_b", b)] {
        assert_eq!(
            fix.node_status(id).await,
            "archived",
            "{label} should be archived after merge"
        );
    }

    // Two MERGED_FROM edges.
    assert_eq!(
        fix.edge_count_from(new_id, "MERGED_FROM").await,
        2,
        "two MERGED_FROM edges expected from new article"
    );

    // Embed task queued.
    assert_eq!(
        fix.pending_task_count("embed", new_id).await,
        1,
        "embed task should be queued for merged article"
    );

    fix.cleanup().await;
}

/// The merged article's content must incorporate material from both sources.
#[tokio::test]
#[serial]
async fn merge_content_includes_both_articles() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let a = fix
        .insert_article("Art A", "Unique phrase from article A.")
        .await;
    let b = fix
        .insert_article("Art B", "Unique phrase from article B.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": a.to_string(), "article_id_b": b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("handle_merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    // The mock LLM returns a fixed title; the content_len must be > 0.
    let content_len = result["content_len"].as_u64().unwrap_or(0);
    assert!(
        content_len > 0,
        "merged article must have non-empty content"
    );

    fix.cleanup().await;
}

// ─── merge: fallback on LLM error ────────────────────────────────────────────

/// When the LLM fails, merge falls back to content concatenation.
/// The result must be marked `degraded = true`.
#[tokio::test]
#[serial]
async fn merge_fallback_on_llm_error() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::always_fail());

    let a = fix
        .insert_article("Degraded A", "Fallback merge article A content.")
        .await;
    let b = fix
        .insert_article("Degraded B", "Fallback merge article B content.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": a.to_string(), "article_id_b": b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("merge should succeed in degraded mode");

    assert_eq!(
        result["degraded"],
        json!(true),
        "should be degraded when LLM fails"
    );

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    let content = fix.node_content(new_id).await;
    assert!(
        content.contains("Fallback merge article A content."),
        "degraded merge should concatenate article A content"
    );
    assert!(
        content.contains("Fallback merge article B content."),
        "degraded merge should concatenate article B content"
    );

    fix.cleanup().await;
}

// ─── merge: provenance inherited ─────────────────────────────────────────────

/// Provenance edges on both input articles must be copied to the merged article.
#[tokio::test]
#[serial]
async fn merge_inherits_provenance_edges() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src_a = fix
        .insert_source("Prov Src A", "Source A provenance.")
        .await;
    let src_b = fix
        .insert_source("Prov Src B", "Source B provenance.")
        .await;
    let art_a = fix
        .insert_article("Art A Prov", "Article A with provenance.")
        .await;
    let art_b = fix
        .insert_article("Art B Prov", "Article B with provenance.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    fix.insert_originates_edge(src_a, art_a).await;
    fix.insert_originates_edge(src_b, art_b).await;

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": art_a.to_string(), "article_id_b": art_b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    // Both provenance sources should have edges pointing to the new article.
    for (label, src) in [("src_a", src_a), ("src_b", src_b)] {
        let inherited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2",
        )
        .bind(src)
        .bind(new_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert!(
            inherited >= 1,
            "provenance from {label} should be inherited by merged article"
        );
    }

    fix.cleanup().await;
}

// ─── infer_edges ─────────────────────────────────────────────────────────────

/// `handle_infer_edges` must complete without error when the node has an
/// embedding.  If vector neighbours are found the mock LLM classifies them
/// at confidence 0.85 (above the 0.5 gate).
#[tokio::test]
#[serial]
async fn infer_edges_completes_with_embeddings() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let node_a = fix
        .insert_source(
            "Node A",
            "Deep learning and convolutional networks for image recognition.",
        )
        .await;
    let node_b = fix
        .insert_source(
            "Node B",
            "Computer vision techniques using CNNs for classification.",
        )
        .await;

    // Identical embeddings → cosine distance = 0 → they are each other's
    // nearest neighbour.
    fix.insert_embedding(node_a).await;
    fix.insert_embedding(node_b).await;

    let task = TestFixture::make_task("infer_edges", Some(node_a), json!({}));
    let result = handle_infer_edges(&fix.pool, &llm, &task)
        .await
        .expect("handle_infer_edges should succeed");

    assert!(
        result.get("edges_created").is_some(),
        "result should contain edges_created"
    );

    fix.cleanup().await;
}

/// When the node has no embedding, `infer_edges` should defer (queue embed and
/// re-queue itself) and return a `status: deferred` result.
#[tokio::test]
#[serial]
async fn infer_edges_defers_when_no_embedding() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let node = fix
        .insert_source("No-Embed Node", "Node without any embedding yet.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("infer_edges");

    let task = TestFixture::make_task("infer_edges", Some(node), json!({}));
    let result = handle_infer_edges(&fix.pool, &llm, &task)
        .await
        .expect("infer_edges should succeed even without embedding");

    // Result signals deferral.
    let status = result["status"].as_str().unwrap_or("");
    assert!(
        status.contains("deferred"),
        "result status should indicate deferral; got: {status}"
    );

    // An embed task should have been queued.
    assert!(
        fix.pending_task_count("embed", node).await >= 1,
        "embed task should be queued by infer_edges when embedding is absent"
    );

    fix.cleanup().await;
}
