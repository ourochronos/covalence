//! Integration tests for OCC Phase 0 (covalence#98).
//!
//! Covers:
//! * `test_occ_conflict`        — concurrent update at the same version → 409
//! * `test_occ_no_version`      — update without expected_version succeeds
//! * `test_contention_dedup`    — duplicate (article, source) contention is
//!                                silently de-duplicated at the DB level

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::models::ContentionType;
use covalence_engine::services::{
    article_service::{ArticleService, CreateArticleRequest, UpdateArticleRequest},
    contention_service::ContentionService,
};

use super::helpers::TestFixture;

// ─── OCC: version conflict ────────────────────────────────────────────────────

/// Simulating two concurrent writers who both read version N and attempt to
/// write: the second one must be rejected with a Conflict error.
///
/// Sequence:
///  1. Create article  (version = 1)
///  2. Writer A reads article — sees version 1, calls update with expected_version = 1 → succeeds, article is now version 2
///  3. Writer B tries update with expected_version = 1 (stale) → must fail with Conflict
#[tokio::test]
#[serial]
async fn test_occ_conflict() {
    let fix = TestFixture::new().await;
    let svc = ArticleService::new(fix.pool.clone());

    // Step 1 — create
    let article = svc
        .create(CreateArticleRequest {
            content: "Original content v1.".into(),
            title: Some("OCC Test Article".into()),
            domain_path: None,
            epistemic_type: None,
            source_ids: None,
            metadata: None,
            facet_function: None,
            facet_scope: None,
        })
        .await
        .expect("create should succeed");

    assert_eq!(
        article.version, 1,
        "freshly created article should be version 1"
    );

    // Step 2 — Writer A: update with correct expected_version → succeeds
    let updated = svc
        .update(
            article.id,
            UpdateArticleRequest {
                content: Some("Writer A content.".into()),
                title: None,
                domain_path: None,
                metadata: None,
                pinned: None,
                facet_function: None,
                facet_scope: None,
                expected_version: Some(1),
            },
        )
        .await
        .expect("writer A update with expected_version=1 should succeed");

    assert_eq!(
        updated.version, 2,
        "version should increment after successful write"
    );

    // Step 3 — Writer B: stale expected_version=1 must be rejected
    let result = svc
        .update(
            article.id,
            UpdateArticleRequest {
                content: Some("Writer B stale write.".into()),
                title: None,
                domain_path: None,
                metadata: None,
                pinned: None,
                facet_function: None,
                facet_scope: None,
                expected_version: Some(1), // stale — article is now at version 2
            },
        )
        .await;

    assert!(
        result.is_err(),
        "writer B update with stale expected_version=1 must fail"
    );

    // Verify the error maps to Conflict (HTTP 409 semantics)
    match result.unwrap_err() {
        covalence_engine::errors::AppError::Conflict(msg) => {
            assert!(
                msg.contains("version conflict"),
                "conflict message should mention 'version conflict'; got: {msg}"
            );
        }
        other => panic!("expected Conflict error, got: {other:?}"),
    }

    // Final state: article content should reflect Writer A's write, not B's
    let final_article = svc.get(article.id).await.expect("get should succeed");
    assert_eq!(
        final_article.content.as_deref(),
        Some("Writer A content."),
        "article content should be Writer A's (B was rejected)"
    );
    assert_eq!(final_article.version, 2, "version should remain at 2");

    // Cleanup
    sqlx::query("DELETE FROM covalence.slow_path_queue WHERE node_id = $1")
        .bind(article.id)
        .execute(&fix.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM covalence.nodes WHERE id = $1")
        .bind(article.id)
        .execute(&fix.pool)
        .await
        .ok();
}

/// Update without expected_version must still succeed (backwards compatibility).
#[tokio::test]
#[serial]
async fn test_occ_no_version_check() {
    let fix = TestFixture::new().await;
    let svc = ArticleService::new(fix.pool.clone());

    let article = svc
        .create(CreateArticleRequest {
            content: "Initial content.".into(),
            title: Some("OCC No-Version Test".into()),
            domain_path: None,
            epistemic_type: None,
            source_ids: None,
            metadata: None,
            facet_function: None,
            facet_scope: None,
        })
        .await
        .expect("create should succeed");

    // Update without supplying expected_version — should always succeed.
    let updated = svc
        .update(
            article.id,
            UpdateArticleRequest {
                content: Some("Updated without version guard.".into()),
                title: None,
                domain_path: None,
                metadata: None,
                pinned: None,
                facet_function: None,
                facet_scope: None,
                expected_version: None, // omit → no OCC check
            },
        )
        .await
        .expect("update without expected_version should always succeed");

    assert_eq!(updated.version, 2, "version should still increment");

    // Cleanup
    sqlx::query("DELETE FROM covalence.slow_path_queue WHERE node_id = $1")
        .bind(article.id)
        .execute(&fix.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM covalence.nodes WHERE id = $1")
        .bind(article.id)
        .execute(&fix.pool)
        .await
        .ok();
}

// ─── Contention dedup ────────────────────────────────────────────────────────

/// Inserting the same (article, source) contention pair twice must produce
/// exactly one row in `covalence.contentions` — the DB UNIQUE constraint and
/// ON CONFLICT DO NOTHING together ensure dedup at both layers.
#[tokio::test]
#[serial]
async fn test_contention_dedup() {
    let mut fix = TestFixture::new().await;
    let svc = ContentionService::new(fix.pool.clone());

    let article_id = fix
        .insert_article("Dedup Article", "Article content for dedup test.")
        .await;
    let source_id = fix
        .insert_source("Dedup Source", "Source content for dedup test.")
        .await;

    // First insert — must succeed.
    let first = svc
        .detect_typed(
            article_id,
            source_id,
            "First contention insert",
            ContentionType::Rebuttal,
        )
        .await
        .expect("first detect_typed should succeed");

    // Second insert — same pair, must NOT error and must return the existing row.
    let second = svc
        .detect_typed(
            article_id,
            source_id,
            "Duplicate contention insert",
            ContentionType::Rebuttal,
        )
        .await
        .expect("second detect_typed (duplicate) should not error");

    // IDs must be identical — second is the de-duplicated existing row.
    assert_eq!(
        first.id, second.id,
        "duplicate insert should return the existing contention row (same id)"
    );

    // Exactly one row must exist in the DB.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.contentions \
         WHERE node_id = $1 AND source_node_id = $2",
    )
    .bind(article_id)
    .bind(source_id)
    .fetch_one(&fix.pool)
    .await
    .expect("count query should succeed");

    assert_eq!(
        count, 1,
        "exactly one contention row should exist after two inserts for the same pair"
    );

    fix.cleanup().await;
}

/// The UNIQUE constraint must also block raw SQL duplicate inserts.
#[tokio::test]
#[serial]
async fn test_contention_unique_constraint_blocks_duplicate() {
    let fix = TestFixture::new().await;

    let article_id: Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content) \
         VALUES (gen_random_uuid(), 'article', 'active', 'Unique Test', 'body') \
         RETURNING id",
    )
    .fetch_one(&fix.pool)
    .await
    .expect("insert article");

    let source_id: Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content) \
         VALUES (gen_random_uuid(), 'source', 'active', 'Unique Source', 'body') \
         RETURNING id",
    )
    .fetch_one(&fix.pool)
    .await
    .expect("insert source");

    // First raw insert — OK.
    sqlx::query(
        "INSERT INTO covalence.contentions \
             (node_id, source_node_id, status) \
         VALUES ($1, $2, 'detected')",
    )
    .bind(article_id)
    .bind(source_id)
    .execute(&fix.pool)
    .await
    .expect("first insert should succeed");

    // Second raw insert — must violate UNIQUE constraint.
    let result = sqlx::query(
        "INSERT INTO covalence.contentions \
             (node_id, source_node_id, status) \
         VALUES ($1, $2, 'detected')",
    )
    .bind(article_id)
    .bind(source_id)
    .execute(&fix.pool)
    .await;

    assert!(
        result.is_err(),
        "duplicate (node_id, source_node_id) insert must violate UNIQUE constraint"
    );

    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("contentions_article_source_uniq")
            || err_str.contains("unique")
            || err_str.contains("duplicate"),
        "error should mention the unique constraint; got: {err_str}"
    );

    fix.cleanup().await;
}
