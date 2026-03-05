//! Integration tests for covalence#196 — metadata filter on source_list
//! and idempotency key on source_ingest.

use serde_json::json;
use serial_test::serial;

use covalence_engine::services::source_service::{IngestRequest, ListParams, SourceService};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

fn make_ingest(content: &str) -> IngestRequest {
    IngestRequest {
        content: content.to_string(),
        source_type: Some("conversation".to_string()),
        title: Some("test source".to_string()),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: None,
        facet_function: None,
        facet_scope: None,
        idempotency_key: None,
    }
}

// ─── Change 1: Metadata filter on source_list ───────────────────────────────

/// Listing with a metadata filter returns only sources whose metadata contains
/// the specified key-value pairs (JSONB @> containment).
#[tokio::test]
#[serial]
async fn test_metadata_filter_basic() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // Ingest two sources with different session_id metadata.
    let mut req1 = make_ingest("conversation about cats");
    req1.metadata = Some(json!({"session_id": "sess-aaa", "chunk_index": 0}));
    let src1 = svc.ingest(req1).await.expect("ingest 1");

    let mut req2 = make_ingest("conversation about dogs");
    req2.metadata = Some(json!({"session_id": "sess-bbb", "chunk_index": 0}));
    let _src2 = svc.ingest(req2).await.expect("ingest 2");

    let mut req3 = make_ingest("more about cats");
    req3.metadata = Some(json!({"session_id": "sess-aaa", "chunk_index": 1}));
    let src3 = svc.ingest(req3).await.expect("ingest 3");

    // Filter by session_id = "sess-aaa"
    let results = svc
        .list(ListParams {
            metadata: Some(json!({"session_id": "sess-aaa"})),
            ..Default::default()
        })
        .await
        .expect("list with metadata filter");

    assert_eq!(results.len(), 2, "should find 2 sources for sess-aaa");
    let ids: Vec<_> = results.iter().map(|r| r.id).collect();
    assert!(ids.contains(&src1.id));
    assert!(ids.contains(&src3.id));

    // Filter by session_id + chunk_index (multi-key containment)
    let results = svc
        .list(ListParams {
            metadata: Some(json!({"session_id": "sess-aaa", "chunk_index": 1})),
            ..Default::default()
        })
        .await
        .expect("list with multi-key metadata filter");

    assert_eq!(
        results.len(),
        1,
        "should find 1 source for sess-aaa chunk 1"
    );
    assert_eq!(results[0].id, src3.id);

    fix.cleanup().await;
}

/// Listing with an empty metadata filter or None returns all sources (no filter applied).
#[tokio::test]
#[serial]
async fn test_metadata_filter_none_returns_all() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let mut req = make_ingest("source with meta");
    req.metadata = Some(json!({"key": "val"}));
    svc.ingest(req).await.expect("ingest");

    svc.ingest(make_ingest("source without meta"))
        .await
        .expect("ingest 2");

    // No metadata filter
    let all = svc.list(ListParams::default()).await.expect("list all");
    assert!(all.len() >= 2, "should return all sources without filter");

    fix.cleanup().await;
}

// ─── Change 2: Idempotency key on source_ingest ────────────────────────────

/// Ingesting with the same idempotency_key returns the existing source
/// without creating a duplicate.
#[tokio::test]
#[serial]
async fn test_idempotency_key_dedup() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let mut req1 = make_ingest("session chunk content A");
    req1.idempotency_key = Some("session:x:chunk:0".to_string());
    let first = svc.ingest(req1).await.expect("first ingest");

    // Second ingest with same key but DIFFERENT content — should return
    // the existing source (idempotency key wins over different content).
    let mut req2 = make_ingest("totally different content");
    req2.idempotency_key = Some("session:x:chunk:0".to_string());
    let second = svc.ingest(req2).await.expect("second ingest (dedup)");

    assert_eq!(
        first.id, second.id,
        "should return same source on duplicate key"
    );
    assert_eq!(
        second.content, "session chunk content A",
        "content should be from the first ingest"
    );

    fix.cleanup().await;
}

/// The idempotency key is stored in metadata and can be queried back.
#[tokio::test]
#[serial]
async fn test_idempotency_key_stored_in_metadata() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let mut req = make_ingest("some content");
    req.idempotency_key = Some("session:y:chunk:2".to_string());
    let src = svc.ingest(req).await.expect("ingest");

    assert_eq!(
        src.metadata["idempotency_key"].as_str(),
        Some("session:y:chunk:2"),
        "idempotency_key should be in metadata"
    );

    fix.cleanup().await;
}

/// Idempotency key dedup is checked BEFORE fingerprint dedup.
/// Two sources with the same content but different idempotency keys should
/// still be deduped by fingerprint (second scenario: different key, same content).
/// But if the first has an idempotency key and the second uses the SAME key,
/// it's caught by the idempotency check before fingerprint is compared.
#[tokio::test]
#[serial]
async fn test_idempotency_key_checked_before_fingerprint() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // First ingest with idempotency key
    let mut req1 = make_ingest("identical content");
    req1.idempotency_key = Some("key-alpha".to_string());
    let first = svc.ingest(req1).await.expect("first ingest");

    // Second ingest with SAME idempotency key but same content too
    let mut req2 = make_ingest("identical content");
    req2.idempotency_key = Some("key-alpha".to_string());
    let second = svc.ingest(req2).await.expect("second ingest");

    assert_eq!(
        first.id, second.id,
        "same idempotency key should return existing source"
    );

    fix.cleanup().await;
}

/// Without an idempotency key, standard fingerprint dedup still works.
#[tokio::test]
#[serial]
async fn test_fingerprint_dedup_still_works() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let first = svc
        .ingest(make_ingest("fingerprint test content"))
        .await
        .expect("first");
    let second = svc
        .ingest(make_ingest("fingerprint test content"))
        .await
        .expect("second");

    assert_eq!(
        first.id, second.id,
        "fingerprint dedup should still work without idempotency key"
    );

    fix.cleanup().await;
}

/// Idempotency keys are namespace-scoped — same key in different namespaces
/// should NOT dedup.
#[tokio::test]
#[serial]
async fn test_idempotency_key_namespace_scoped() {
    let fix = TestFixture::new().await;
    let svc_a = SourceService::new(fix.pool.clone()).with_namespace("ns-alpha".to_string());
    let svc_b = SourceService::new(fix.pool.clone()).with_namespace("ns-beta".to_string());

    let mut req1 = make_ingest("ns-scoped content");
    req1.idempotency_key = Some("shared-key".to_string());
    let src_a = svc_a.ingest(req1).await.expect("ingest in ns-alpha");

    let mut req2 = make_ingest("ns-scoped content in beta");
    req2.idempotency_key = Some("shared-key".to_string());
    let src_b = svc_b.ingest(req2).await.expect("ingest in ns-beta");

    assert_ne!(
        src_a.id, src_b.id,
        "same idempotency key in different namespaces should create separate sources"
    );

    fix.cleanup().await;
}

// ─── Change 3: GIN index creation ──────────────────────────────────────────

/// The GIN index on metadata for source nodes should exist after migrations.
/// This test verifies the index was created by migration 043.
#[tokio::test]
#[serial]
async fn test_gin_index_exists() {
    let fix = TestFixture::new().await;

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM pg_indexes
            WHERE indexname = 'idx_nodes_metadata_gin'
              AND tablename = 'nodes'
              AND schemaname = 'covalence'
        )",
    )
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(false);

    assert!(
        exists,
        "GIN index idx_nodes_metadata_gin should exist on covalence.nodes"
    );

    fix.cleanup().await;
}

/// Metadata containment queries should work efficiently (at minimum, they must
/// work correctly — the index makes them fast).
#[tokio::test]
#[serial]
async fn test_metadata_containment_query_works() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // Ingest a source with nested metadata
    let mut req = make_ingest("nested metadata test");
    req.metadata = Some(json!({
        "platform": "openclaw",
        "session_id": "sess-nested",
        "tags": ["important", "review"]
    }));
    let src = svc.ingest(req).await.expect("ingest");

    // Query by a subset of metadata
    let results = svc
        .list(ListParams {
            metadata: Some(json!({"platform": "openclaw"})),
            ..Default::default()
        })
        .await
        .expect("list by platform");

    assert!(
        results.iter().any(|r| r.id == src.id),
        "should find source by platform metadata"
    );

    fix.cleanup().await;
}
