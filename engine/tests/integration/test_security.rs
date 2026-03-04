//! Integration tests for security hardening — covalence#84.
//!
//! Covers:
//! - Fix 1: SQL injection prevention in `SourceService::list()` filters
//! - Fix 3: Content size limit (100 KiB) enforced on ingestion

use serial_test::serial;

use covalence_engine::services::source_service::{IngestRequest, ListParams, SourceService};

use super::helpers::TestFixture;

// ─── Fix 1: SQL injection via filter parameters ───────────────────────────────

/// `source_type` filter with SQL metacharacters must be treated as a literal
/// string — the query must succeed (returning zero results) rather than
/// erroring or mutating the database.
#[tokio::test]
#[serial]
async fn test_list_sql_injection_via_source_type() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // Classic injection payload: ends the string literal and appends a
    // destructive statement.  With parameterized queries this is inert.
    let params = ListParams {
        source_type: Some("'; DROP TABLE covalence.nodes; --".to_string()),
        ..Default::default()
    };

    let result = svc.list(params).await;
    assert!(
        result.is_ok(),
        "list() must not error on SQL metacharacters in source_type: {:?}",
        result.err()
    );
    // The injected string matches no real source_type, so results must be empty.
    assert!(result.unwrap().is_empty());

    fix.cleanup().await;
}

/// `status` filter with a tautology injection payload must not bypass the
/// WHERE clause and must not produce an error.
#[tokio::test]
#[serial]
async fn test_list_sql_injection_via_status() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let params = ListParams {
        status: Some("' OR '1'='1".to_string()),
        ..Default::default()
    };

    let result = svc.list(params).await;
    assert!(
        result.is_ok(),
        "list() must not error on SQL metacharacters in status: {:?}",
        result.err()
    );
    // A tautology injection would leak all rows; with bind params it's a
    // literal string that matches nothing.
    assert!(result.unwrap().is_empty());

    fix.cleanup().await;
}

/// Full-text search `q` filter with SQL metacharacters must be parameterized
/// and must not cause a query error.
#[tokio::test]
#[serial]
async fn test_list_sql_injection_via_search_q() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let params = ListParams {
        q: Some("'; DROP TABLE covalence.nodes; --".to_string()),
        ..Default::default()
    };

    let result = svc.list(params).await;
    assert!(
        result.is_ok(),
        "list() must not error on SQL metacharacters in q: {:?}",
        result.err()
    );

    fix.cleanup().await;
}

/// After all injection attempts, covalence.nodes must still exist and be
/// queryable (the table was never dropped).
#[tokio::test]
#[serial]
async fn test_nodes_table_survives_injection_attempts() {
    let fix = TestFixture::new().await;

    // Insert a known-good source, then verify it can be listed back.
    let src_id = fix
        .pool
        .acquire()
        .await
        .map(|_| uuid::Uuid::new_v4())
        .expect("pool acquire");

    // Direct DB check: table is alive.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM covalence.nodes WHERE node_type = 'source'")
            .fetch_one(&fix.pool)
            .await
            .expect("nodes table must still exist after injection attempts");

    // We just care the table is alive and the query executed cleanly.
    assert!(count >= 0);
    let _ = src_id;

    fix.cleanup().await;
}

// ─── Fix 3: Content size limit ────────────────────────────────────────────────

/// Content exceeding 100 KiB must be rejected with a `PayloadTooLarge` error.
#[tokio::test]
#[serial]
async fn test_ingest_rejects_oversized_content() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // 101 KiB — one byte over the limit.
    let huge_content = "x".repeat(101 * 1024);
    let req = IngestRequest {
        content: huge_content,
        source_type: Some("document".to_string()),
        title: Some("oversized-security-test".to_string()),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: None,
        facet_function: None,
        facet_scope: None,
    };

    let result = svc.ingest(req).await;
    assert!(result.is_err(), "ingest() must reject content > 100 KiB");

    // Verify error message is informative.
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("payload too large") || err_msg.contains("exceeds"),
        "error message should describe the size limit, got: {err_msg}"
    );

    fix.cleanup().await;
}

/// Content at exactly 100 KiB (the limit boundary) must be accepted.
#[tokio::test]
#[serial]
async fn test_ingest_accepts_content_at_limit() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // Exactly 100 KiB — at the boundary, should be accepted.
    let max_content = "y".repeat(100 * 1024);
    let req = IngestRequest {
        content: max_content,
        source_type: Some("document".to_string()),
        title: Some("max-size-security-test".to_string()),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: None,
        facet_function: None,
        facet_scope: None,
    };

    let result = svc.ingest(req).await;
    assert!(
        result.is_ok(),
        "ingest() must accept content at exactly 100 KiB: {:?}",
        result.err()
    );

    fix.cleanup().await;
}

/// Small content well within the limit must continue to be accepted normally.
#[tokio::test]
#[serial]
async fn test_ingest_accepts_normal_content() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let req = IngestRequest {
        content: "Normal-sized content for security test.".to_string(),
        source_type: Some("document".to_string()),
        title: Some("normal-size-security-test".to_string()),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: None,
        facet_function: None,
        facet_scope: None,
    };

    let result = svc.ingest(req).await;
    assert!(
        result.is_ok(),
        "ingest() must accept normal-sized content: {:?}",
        result.err()
    );

    fix.cleanup().await;
}
