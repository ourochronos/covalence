//! Integration tests for staleness detection (covalence#36).
//!
//! Covers:
//! * `test_staleness_scan_finds_stale_article`      — unlinked newer source in same domain → stale
//! * `test_staleness_scan_skips_linked_sources`     — already-linked source never stales article
//! * `test_staleness_scan_queues_for_recompilation` — stale article gets a `recompile` queue entry

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::admin_service::AdminService;

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Insert an article with an explicit `domain_path` and return its id.
async fn insert_article_with_domain(fix: &mut TestFixture, title: &str, domain: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, domain_path, metadata) \
         VALUES ($1, 'article', 'active', $2, 'article content', ARRAY[$3]::text[], '{}'::jsonb)",
    )
    .bind(id)
    .bind(title)
    .bind(domain)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_article_with_domain failed: {e}"));
    fix.track(id)
}

/// Insert a source with an explicit `domain_path` and return its id.
async fn insert_source_with_domain(fix: &mut TestFixture, title: &str, domain: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, domain_path, metadata) \
         VALUES ($1, 'source', 'active', $2, 'source content', ARRAY[$3]::text[], '{}'::jsonb)",
    )
    .bind(id)
    .bind(title)
    .bind(domain)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_source_with_domain failed: {e}"));
    fix.track(id)
}

/// Link a source to an article via an ORIGINATES edge.
async fn link_source_to_article(fix: &TestFixture, source_id: Uuid, article_id: Uuid) {
    sqlx::query(
        "INSERT INTO covalence.edges (source_node_id, target_node_id, edge_type) \
         VALUES ($1, $2, 'ORIGINATES') \
         ON CONFLICT DO NOTHING",
    )
    .bind(source_id)
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("link_source_to_article failed: {e}"));
}

/// Push the article's `modified_at` into the past so that any subsequently
/// inserted source will have a newer `created_at`.
async fn backdate_article(fix: &TestFixture, article_id: Uuid) {
    sqlx::query(
        "UPDATE covalence.nodes \
         SET modified_at = now() - interval '10 minutes' \
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("backdate_article failed: {e}"));
}

// ─── tests ────────────────────────────────────────────────────────────────────

/// An article whose domain matches a newer, unlinked source should be detected
/// as stale and reported in `stale_count`.
#[tokio::test]
#[serial]
async fn test_staleness_scan_finds_stale_article() {
    let mut fix = TestFixture::new().await;

    // Create article; push its modified_at into the past.
    let article_id = insert_article_with_domain(&mut fix, "Stale Article", "test_domain").await;
    backdate_article(&fix, article_id).await;

    // Create a source in the same domain — its created_at is now() > article's modified_at.
    let _source_id = insert_source_with_domain(&mut fix, "Newer Source", "test_domain").await;

    // Run the staleness scan.
    let svc = AdminService::new(fix.pool.clone());
    let result = svc
        .staleness_scan()
        .await
        .expect("staleness_scan should succeed");

    assert!(
        result.stale_count >= 1,
        "expected at least 1 stale article, got {}",
        result.stale_count
    );

    fix.cleanup().await;
}

/// An article whose linked sources are all already attached should NOT be
/// considered stale, even if those sources are newer.
#[tokio::test]
#[serial]
async fn test_staleness_scan_skips_linked_sources() {
    let mut fix = TestFixture::new().await;

    // Create article; backdate it so the source appears newer.
    let article_id =
        insert_article_with_domain(&mut fix, "Fresh Article (linked)", "linked_domain").await;
    backdate_article(&fix, article_id).await;

    // Create a source in the same domain and link it to the article.
    let source_id = insert_source_with_domain(&mut fix, "Linked Source", "linked_domain").await;
    link_source_to_article(&fix, source_id, article_id).await;

    // Count pending recompile jobs for this article before the scan.
    let before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.slow_path_queue \
         WHERE task_type = 'recompile' AND node_id = $1 AND status = 'pending'",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    // Run the staleness scan.
    let svc = AdminService::new(fix.pool.clone());
    let _result = svc
        .staleness_scan()
        .await
        .expect("staleness_scan should succeed");

    // Count again — must not have increased for this article.
    let after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.slow_path_queue \
         WHERE task_type = 'recompile' AND node_id = $1 AND status = 'pending'",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        before, after,
        "linked-source article should NOT be re-queued by staleness scan"
    );

    fix.cleanup().await;
}

/// A stale article should have a `recompile` task queued in `slow_path_queue`
/// after the scan, and `queued_count` must reflect it.
#[tokio::test]
#[serial]
async fn test_staleness_scan_queues_for_recompilation() {
    let mut fix = TestFixture::new().await;

    // Create article and backdate it.
    let article_id =
        insert_article_with_domain(&mut fix, "Stale For Recompile", "queue_domain").await;
    backdate_article(&fix, article_id).await;

    // Create an unlinked source in the same domain.
    let _source_id =
        insert_source_with_domain(&mut fix, "Queue Trigger Source", "queue_domain").await;

    // Ensure no existing recompile job for this article.
    sqlx::query(
        "DELETE FROM covalence.slow_path_queue \
         WHERE task_type = 'recompile' AND node_id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .ok();

    // Run the staleness scan.
    let svc = AdminService::new(fix.pool.clone());
    let result = svc
        .staleness_scan()
        .await
        .expect("staleness_scan should succeed");

    assert!(
        result.queued_count >= 1,
        "expected at least 1 article queued, got {}",
        result.queued_count
    );

    // Verify that a pending `recompile` entry exists for our article.
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.slow_path_queue \
         WHERE task_type = 'recompile' AND node_id = $1 AND status = 'pending'",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        pending >= 1,
        "expected a pending recompile queue entry for article {article_id}, found {pending}"
    );

    fix.cleanup().await;
}
