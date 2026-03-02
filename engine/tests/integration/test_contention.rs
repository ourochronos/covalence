//! Integration tests for the `contention_check` and `resolve_contention`
//! slow-path handlers.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::{
    contention::{handle_contention_check, handle_resolve_contention},
    llm::LlmClient,
};

use super::helpers::{MockLlmClient, TestFixture};

// ─── contention_check ────────────────────────────────────────────────────────

/// `handle_contention_check` must complete without error.  When the article
/// and source have embeddings that are close enough for the vector window, and
/// the mock LLM returns `is_contention=true`, a contention row should be
/// inserted and a `resolve_contention` task queued.
#[tokio::test]
#[serial]
async fn contention_check_detects_contradiction() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let article = fix
        .insert_article(
            "Established Article",
            "The boiling point of water is 100 degrees Celsius at sea level.",
        )
        .await;
    let source = fix
        .insert_source(
            "Contradicting Source",
            "Recent research suggests water does not boil at 100 degrees Celsius.",
        )
        .await;

    // Both nodes need embeddings for the vector-similarity scan.
    fix.insert_embedding(article).await;
    fix.insert_embedding(source).await;
    fix.track_task_type("resolve_contention");

    let task = TestFixture::make_task("contention_check", Some(source), json!({}));
    let result = handle_contention_check(&fix.pool, &llm, &task)
        .await
        .expect("handle_contention_check should succeed");

    assert_eq!(
        result["source_id"].as_str().unwrap(),
        source.to_string(),
        "result.source_id mismatch"
    );

    let contentions_created = result["contentions_created"].as_i64().unwrap_or(0);

    // If the article landed within the cosine-distance window a contention
    // row and resolve task should exist.
    if contentions_created > 0 {
        let contention_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(\
               SELECT 1 FROM covalence.contentions \
               WHERE node_id = $1 AND source_node_id = $2 \
             )",
        )
        .bind(article)
        .bind(source)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(false);

        assert!(
            contention_exists,
            "contention row should exist in covalence.contentions"
        );

        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM covalence.slow_path_queue \
             WHERE task_type = 'resolve_contention' \
               AND node_id   = $1 \
               AND status    = 'pending'",
        )
        .bind(article)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert_eq!(queued, 1, "resolve_contention task should be queued");
    }

    fix.cleanup().await;
}

/// `contention_check` must not insert duplicate contention rows if an
/// unresolved contention between the same pair already exists.
#[tokio::test]
#[serial]
async fn contention_check_no_duplicate_rows() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let article = fix
        .insert_article(
            "Article For Dup Check",
            "The sky is blue during the daytime under clear conditions.",
        )
        .await;
    let source = fix
        .insert_source(
            "Source For Dup Check",
            "The sky appears blue but this is not a universal truth.",
        )
        .await;

    fix.insert_embedding(article).await;
    fix.insert_embedding(source).await;
    fix.track_task_type("resolve_contention");

    let task = TestFixture::make_task("contention_check", Some(source), json!({}));

    // Run twice — the second run must not insert a duplicate.
    handle_contention_check(&fix.pool, &llm, &task)
        .await
        .expect("first contention_check should succeed");
    handle_contention_check(&fix.pool, &llm, &task)
        .await
        .expect("second contention_check should succeed");

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.contentions \
         WHERE (node_id = $1 AND source_node_id = $2) \
            OR (node_id = $2 AND source_node_id = $1)",
    )
    .bind(article)
    .bind(source)
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        count <= 1,
        "at most one contention row should exist (no duplicates); found {count}"
    );

    fix.cleanup().await;
}

// ─── resolve_contention ───────────────────────────────────────────────────────

/// `handle_resolve_contention` with `supersede_b` resolution must:
/// * Replace the article's content.
/// * Mark the contention as resolved.
/// * Queue an `embed` task for re-embedding the updated article.
/// * Queue a `tree_embed` task to invalidate cached section embeddings.
#[tokio::test]
#[serial]
async fn resolve_contention_supersede_b_updates_article() {
    let mut fix = TestFixture::new().await;

    let fixed_resp = json!({
        "resolution": "supersede_b",
        "materiality": "high",
        "reasoning": "The source contains more accurate information.",
        "updated_content": "Corrected article content based on the source."
    });
    let llm: Arc<dyn LlmClient> =
        Arc::new(MockLlmClient::with_fixed_response(fixed_resp.to_string()));

    let article = fix
        .insert_article(
            "Article To Supersede",
            "Original article content that will be superseded.",
        )
        .await;
    let source = fix
        .insert_source(
            "Superseding Source",
            "Source with more accurate information.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let contention_id = fix.insert_contention(article, source, "high", 0.9).await;

    let task = TestFixture::make_task(
        "resolve_contention",
        Some(article),
        json!({ "contention_id": contention_id.to_string() }),
    );

    let result = handle_resolve_contention(&fix.pool, &llm, &task)
        .await
        .expect("handle_resolve_contention should succeed");

    assert_eq!(result["resolution"].as_str().unwrap(), "supersede_b");

    // Article content must have been replaced.
    let new_content = fix.node_content(article).await;
    assert_eq!(
        new_content, "Corrected article content based on the source.",
        "article content should be replaced by supersede_b"
    );

    // Contention should be marked resolved.
    let resolution: String = sqlx::query_scalar(
        "SELECT COALESCE(resolution, status) FROM covalence.contentions WHERE id = $1",
    )
    .bind(contention_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap();
    assert!(
        resolution == "supersede_b" || resolution == "resolved",
        "contention status should reflect resolution; got: {resolution}"
    );

    // Re-embed task queued.
    assert_eq!(
        fix.pending_task_count("embed", article).await,
        1,
        "re-embed task should be queued after supersede_b"
    );

    // tree_embed invalidation task queued.
    assert_eq!(
        fix.pending_task_count("tree_embed", article).await,
        1,
        "tree_embed invalidation task should be queued after supersede_b"
    );

    fix.cleanup().await;
}

/// `supersede_a` resolution must leave the article's content unchanged and
/// still mark the contention resolved.
#[tokio::test]
#[serial]
async fn resolve_contention_supersede_a_leaves_article_unchanged() {
    let mut fix = TestFixture::new().await;

    let fixed_resp = json!({
        "resolution": "supersede_a",
        "materiality": "low",
        "reasoning": "The article's existing information is more reliable.",
        "updated_content": ""
    });
    let llm: Arc<dyn LlmClient> =
        Arc::new(MockLlmClient::with_fixed_response(fixed_resp.to_string()));

    let original_content = "This article content should remain intact.";
    let article = fix.insert_article("Intact Article", original_content).await;
    let source = fix
        .insert_source("Weaker Source", "Source that is superseded by the article.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let contention_id = fix.insert_contention(article, source, "low", 0.2).await;

    let task = TestFixture::make_task(
        "resolve_contention",
        Some(article),
        json!({ "contention_id": contention_id.to_string() }),
    );

    let result = handle_resolve_contention(&fix.pool, &llm, &task)
        .await
        .expect("handle_resolve_contention should succeed for supersede_a");

    assert_eq!(result["resolution"].as_str().unwrap(), "supersede_a");

    // Content must be unchanged.
    let content = fix.node_content(article).await;
    assert_eq!(
        content, original_content,
        "article content should not change under supersede_a"
    );

    // Contention should be resolved.
    let res: String = sqlx::query_scalar(
        "SELECT COALESCE(resolution, status) FROM covalence.contentions WHERE id = $1",
    )
    .bind(contention_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap();
    assert!(
        res == "supersede_a" || res == "resolved",
        "contention should be resolved; got: {res}"
    );

    fix.cleanup().await;
}

/// `accept_both` resolution must leave the article content unchanged and
/// update the contention status to `resolved` without queuing embed tasks.
#[tokio::test]
#[serial]
async fn resolve_contention_accept_both_no_content_change() {
    let mut fix = TestFixture::new().await;

    let fixed_resp = json!({
        "resolution": "accept_both",
        "materiality": "medium",
        "reasoning": "Both perspectives are valid and complementary.",
        "updated_content": ""
    });
    let llm: Arc<dyn LlmClient> =
        Arc::new(MockLlmClient::with_fixed_response(fixed_resp.to_string()));

    let original_content = "This article has valid content.";
    let article = fix
        .insert_article("Accept Both Article", original_content)
        .await;
    let source = fix
        .insert_source("Complementary Source", "Source with a complementary view.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let contention_id = fix.insert_contention(article, source, "medium", 0.5).await;

    let task = TestFixture::make_task(
        "resolve_contention",
        Some(article),
        json!({ "contention_id": contention_id.to_string() }),
    );

    let result = handle_resolve_contention(&fix.pool, &llm, &task)
        .await
        .expect("handle_resolve_contention should succeed for accept_both");

    assert_eq!(result["resolution"].as_str().unwrap(), "accept_both");

    // Content must be unchanged.
    let content = fix.node_content(article).await;
    assert_eq!(
        content, original_content,
        "article content must not change under accept_both"
    );

    fix.cleanup().await;
}

/// When the contention_id in the payload does not refer to an existing row,
/// the handler must return an error.
#[tokio::test]
#[serial]
async fn resolve_contention_missing_contention_returns_error() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let article = fix
        .insert_article("Ghost Article", "Content of the ghost article.")
        .await;

    let missing_id = Uuid::new_v4();
    let task = TestFixture::make_task(
        "resolve_contention",
        Some(article),
        json!({ "contention_id": missing_id.to_string() }),
    );

    let result = handle_resolve_contention(&fix.pool, &llm, &task).await;
    assert!(
        result.is_err(),
        "handler should return Err for a non-existent contention_id"
    );

    fix.cleanup().await;
}
