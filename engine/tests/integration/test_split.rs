//! Integration tests for the `split` slow-path handler.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::{handle_split, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── split with tree_index ────────────────────────────────────────────────────

/// When `metadata.tree_index` is present the handler must use the tree's
/// `end_char` boundaries to select the split point (no LLM call).
///
/// After the call:
/// * Original article → `archived`.
/// * Two new `active` articles created.
/// * Two `SPLIT_INTO` edges from original → each new article.
/// * Pre-existing provenance edges copied to both new articles.
/// * The two content halves must cover the full original content.
#[tokio::test]
#[serial]
async fn split_with_tree_index_no_llm() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let content = "A".repeat(100) + &"B".repeat(100);
    let tree_meta = json!({
        "tree_index": [
            { "title": "Part One", "start_char": 0, "end_char": 95 },
            { "title": "Part Two", "start_char": 95, "end_char": 200 }
        ]
    });

    let article_id = fix
        .insert_article_with_meta("Large Article", &content, &tree_meta)
        .await;

    // Insert a provenance source + ORIGINATES edge.
    let prov_src = fix
        .insert_source("Prov Source", "Provenance content.")
        .await;
    fix.insert_originates_edge(prov_src, article_id).await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task("split", Some(article_id), json!({}));
    let result = handle_split(&fix.pool, &llm, &task)
        .await
        .expect("handle_split should succeed with tree_index");

    // LLM must not have been called.
    assert_eq!(
        mock.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "LLM must not be called when tree_index is present"
    );

    let part_a = Uuid::parse_str(result["part_a_id"].as_str().unwrap()).unwrap();
    let part_b = Uuid::parse_str(result["part_b_id"].as_str().unwrap()).unwrap();
    fix.track(part_a);
    fix.track(part_b);

    // Both new articles are active.
    assert_eq!(
        fix.node_status(part_a).await,
        "active",
        "part_a should be active"
    );
    assert_eq!(
        fix.node_status(part_b).await,
        "active",
        "part_b should be active"
    );

    // Original is archived.
    assert_eq!(
        fix.node_status(article_id).await,
        "archived",
        "original should be archived after split"
    );

    // Two SPLIT_INTO edges.
    assert_eq!(
        fix.edge_count_from(article_id, "SPLIT_INTO").await,
        2,
        "two SPLIT_INTO edges should exist from original"
    );

    // Content lengths sum to original.
    let a_len = result["part_a_len"].as_u64().unwrap() as usize;
    let b_len = result["part_b_len"].as_u64().unwrap() as usize;
    assert_eq!(
        a_len + b_len,
        content.len(),
        "part contents must cover the full original"
    );

    // Provenance edge copied to both new articles.
    for &part_id in &[part_a, part_b] {
        let inherited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2 \
               AND edge_type = 'ORIGINATES'",
        )
        .bind(prov_src)
        .bind(part_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert_eq!(
            inherited, 1,
            "provenance edge should be inherited by part {part_id}"
        );
    }

    fix.cleanup().await;
}

// ─── split without tree_index (LLM path) ─────────────────────────────────────

/// Without `metadata.tree_index` the handler must call the LLM to obtain the
/// split point.
#[tokio::test]
#[serial]
async fn split_without_tree_index_calls_llm() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let content = "First half of the article with interesting content. ".repeat(5)
        + &"Second half covering a different sub-topic. ".repeat(5);

    let article_id = fix
        .insert_article("Article Without Tree Index", &content)
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task("split", Some(article_id), json!({}));
    let result = handle_split(&fix.pool, &llm, &task)
        .await
        .expect("handle_split should succeed without tree_index");

    assert!(
        mock.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst)
            >= 1,
        "LLM must be consulted when tree_index is absent"
    );

    let part_a = Uuid::parse_str(result["part_a_id"].as_str().unwrap()).unwrap();
    let part_b = Uuid::parse_str(result["part_b_id"].as_str().unwrap()).unwrap();
    fix.track(part_a);
    fix.track(part_b);

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.nodes \
         WHERE id = ANY($1) AND status = 'active'",
    )
    .bind(vec![part_a, part_b])
    .fetch_one(&fix.pool)
    .await
    .unwrap();
    assert_eq!(count, 2, "both split parts should be active");

    fix.cleanup().await;
}

// ─── embed tasks queued for both parts ───────────────────────────────────────

/// After a successful split, `embed` tasks must be queued for both new parts.
#[tokio::test]
#[serial]
async fn split_queues_embed_for_both_parts() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let content = "X".repeat(100) + &"Y".repeat(100);
    let tree_meta = json!({
        "tree_index": [
            { "title": "Sec 1", "start_char": 0,  "end_char": 100 },
            { "title": "Sec 2", "start_char": 100, "end_char": 200 }
        ]
    });
    let article_id = fix
        .insert_article_with_meta("Two-Section Article", &content, &tree_meta)
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task("split", Some(article_id), json!({}));
    let result = handle_split(&fix.pool, &llm, &task)
        .await
        .expect("split should succeed");

    let part_a = Uuid::parse_str(result["part_a_id"].as_str().unwrap()).unwrap();
    let part_b = Uuid::parse_str(result["part_b_id"].as_str().unwrap()).unwrap();
    fix.track(part_a);
    fix.track(part_b);

    for (label, part_id) in [("part_a", part_a), ("part_b", part_b)] {
        assert!(
            fix.pending_task_count("embed", part_id).await >= 1,
            "embed task should be queued for {label}"
        );
    }

    fix.cleanup().await;
}

// ─── idempotency guard ────────────────────────────────────────────────────────

/// If a `split` task for the same node is already complete, the handler should
/// skip and return `{"skipped": true, "reason": "already_complete"}`.
#[tokio::test]
#[serial]
async fn split_idempotency_guard() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let article_id = fix
        .insert_article("Idempotent Split Article", "Content to split.")
        .await;

    // Pre-insert a completed split task for this node.
    let guard_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
         (id, task_type, node_id, payload, status, priority) \
         VALUES ($1, 'split', $2, '{}'::jsonb, 'complete', 0)",
    )
    .bind(guard_id)
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("insert guard task");

    let task = TestFixture::make_task("split", Some(article_id), json!({}));
    let result = handle_split(&fix.pool, &llm, &task)
        .await
        .expect("split should return skip result");

    assert_eq!(result["skipped"], json!(true));
    assert_eq!(result["reason"], json!("already_complete"));

    sqlx::query("DELETE FROM covalence.slow_path_queue WHERE id = $1")
        .bind(guard_id)
        .execute(&fix.pool)
        .await
        .ok();

    fix.cleanup().await;
}
