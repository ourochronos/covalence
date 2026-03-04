//! Integration tests for source quality — intentionality via `capture_method` (issue #37).
//!
//! Verifies that the `capture_method` field on `IngestRequest`:
//!   * raises reliability for `manual` observations/conversations
//!   * lowers reliability for `auto` observations/conversations
//!   * leaves default reliability unchanged when absent
//!   * is persisted in metadata and surfaced on `SourceResponse`

use serial_test::serial;

use covalence_engine::services::source_service::{IngestRequest, SourceService};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

fn make_observation_request(capture_method: Option<&str>) -> IngestRequest {
    // Use a unique suffix so the SHA-256 fingerprint differs between tests
    // (the DB is truncated per-suite, but this is good hygiene).
    let suffix = capture_method.unwrap_or("none");
    IngestRequest {
        content: format!("test observation content for capture_method={suffix}"),
        source_type: Some("observation".to_string()),
        title: Some(format!("test-obs-{suffix}")),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: capture_method.map(str::to_owned),
        facet_function: None,
        facet_scope: None,
    }
}

// ─── manual observation ───────────────────────────────────────────────────────

/// A manually-ingested observation must have reliability ≥ 0.60
/// (spec: manual + observation → 0.65).
#[tokio::test]
#[serial]
async fn test_manual_observation_gets_high_reliability() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let resp = svc
        .ingest(make_observation_request(Some("manual")))
        .await
        .expect("ingest should succeed");

    assert!(
        resp.reliability >= 0.60,
        "manual observation reliability should be ≥ 0.60, got {}",
        resp.reliability
    );

    fix.cleanup().await;
}

// ─── auto observation ─────────────────────────────────────────────────────────

/// An auto-captured observation must have reliability ≤ 0.30
/// (spec: auto + observation → 0.25).
#[tokio::test]
#[serial]
async fn test_auto_observation_gets_low_reliability() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let resp = svc
        .ingest(make_observation_request(Some("auto")))
        .await
        .expect("ingest should succeed");

    assert!(
        resp.reliability <= 0.30,
        "auto observation reliability should be ≤ 0.30, got {}",
        resp.reliability
    );

    fix.cleanup().await;
}

// ─── default observation (no capture_method) ──────────────────────────────────

/// When `capture_method` is absent the baseline reliability for `observation`
/// must remain exactly 0.4 (backward-compat).
#[tokio::test]
#[serial]
async fn test_default_observation_unchanged() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let resp = svc
        .ingest(make_observation_request(None))
        .await
        .expect("ingest should succeed");

    assert!(
        (resp.reliability - 0.4_f32).abs() < 1e-4,
        "default observation reliability should be exactly 0.4, got {}",
        resp.reliability
    );

    fix.cleanup().await;
}

// ─── capture_method persisted in metadata ────────────────────────────────────

/// After ingestion, `GET /sources/{id}` (via `SourceService::get`) must
/// return a `SourceResponse` where `capture_method == "manual"` and
/// `metadata["capture_method"] == "manual"`.
#[tokio::test]
#[serial]
async fn test_capture_method_stored_in_metadata() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let created = svc
        .ingest(IngestRequest {
            content: "stored capture method test content".to_string(),
            source_type: Some("observation".to_string()),
            title: Some("capture-method-storage-test".to_string()),
            metadata: None,
            session_id: None,
            reliability: None,
            capture_method: Some("manual".to_string()),
            facet_function: None,
            facet_scope: None,
        })
        .await
        .expect("ingest should succeed");

    // Re-fetch via get() to confirm round-trip through the DB.
    let fetched = svc
        .get(created.id)
        .await
        .expect("get should succeed for newly ingested source");

    // Top-level field
    assert_eq!(
        fetched.capture_method.as_deref(),
        Some("manual"),
        "SourceResponse.capture_method should be 'manual'"
    );

    // Metadata JSONB
    let meta_value = fetched
        .metadata
        .get("capture_method")
        .and_then(|v| v.as_str());
    assert_eq!(
        meta_value,
        Some("manual"),
        "metadata[\"capture_method\"] should be 'manual', got: {:?}",
        fetched.metadata
    );

    fix.cleanup().await;
}
