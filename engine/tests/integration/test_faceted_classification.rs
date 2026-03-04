//! Integration tests for faceted classification (covalence#92 Phase 1).
//!
//! Verifies that:
//! 1. Sources ingested with explicit facets round-trip correctly through the DB.
//! 2. Search `facet_function` filter includes only matching nodes.
//! 3. Sources without facets (NULL) continue to work normally.

use serial_test::serial;

use super::helpers::TestFixture;
use covalence_engine::services::{
    search_service::{SearchRequest, SearchService},
    source_service::{IngestRequest, SourceService},
};

// ─── test helpers ─────────────────────────────────────────────────────────────

fn ingest_req(
    title: &str,
    content: &str,
    facet_function: Option<Vec<String>>,
    facet_scope: Option<Vec<String>>,
) -> IngestRequest {
    IngestRequest {
        title: Some(title.into()),
        content: content.into(),
        source_type: Some("document".into()),
        reliability: None,
        metadata: None,
        session_id: None,
        capture_method: None,
        facet_function,
        facet_scope,
    }
}

// ─── test_ingest_with_facets ──────────────────────────────────────────────────

/// AC1: A source ingested with facet_function and facet_scope returns both
/// facets verbatim when fetched.
#[tokio::test]
#[serial]
async fn test_ingest_with_facets() {
    let mut fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    // Ingest a source with explicit facets.
    let resp = svc
        .ingest(ingest_req(
            "faceted-source-retrieval",
            "This document covers retrieval algorithms in practical settings.",
            Some(vec!["retrieval".into()]),
            Some(vec!["practical".into()]),
        ))
        .await
        .expect("ingest with facets should succeed");

    fix.track(resp.id);

    // Verify the ingest response contains the facets.
    assert_eq!(
        resp.facet_function.as_deref(),
        Some(["retrieval".to_string()].as_slice()),
        "ingest response must expose facet_function verbatim"
    );
    assert_eq!(
        resp.facet_scope.as_deref(),
        Some(["practical".to_string()].as_slice()),
        "ingest response must expose facet_scope verbatim"
    );

    // Re-fetch and verify round-trip through the DB.
    let fetched = svc.get(resp.id).await.expect("get should succeed");
    assert_eq!(
        fetched.facet_function.as_deref(),
        Some(["retrieval".to_string()].as_slice()),
        "fetched source must preserve facet_function"
    );
    assert_eq!(
        fetched.facet_scope.as_deref(),
        Some(["practical".to_string()].as_slice()),
        "fetched source must preserve facet_scope"
    );

    fix.cleanup().await;
}

// ─── test_search_facet_filter ─────────────────────────────────────────────────

/// AC2: When a search request includes facet_function=["retrieval"], only
/// the retrieval-faceted source appears in results; the storage-faceted source
/// is filtered out.
#[tokio::test]
#[serial]
async fn test_search_facet_filter() {
    let mut fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());
    let search_svc = SearchService::new(fix.pool.clone());
    search_svc.init().await;

    // Ingest two sources with different facet_function values.
    let retrieval_resp = svc
        .ingest(ingest_req(
            "faceted-retrieval-unique-xkq92",
            "Retrieval-augmented generation improves knowledge access patterns.",
            Some(vec!["retrieval".into()]),
            Some(vec!["practical".into()]),
        ))
        .await
        .expect("ingest retrieval source should succeed");
    fix.track(retrieval_resp.id);

    let storage_resp = svc
        .ingest(ingest_req(
            "faceted-storage-unique-xkq92",
            "Storage layer design for knowledge persistence and retrieval.",
            Some(vec!["storage".into()]),
            Some(vec!["operational".into()]),
        ))
        .await
        .expect("ingest storage source should succeed");
    fix.track(storage_resp.id);

    // Search with facet_function=["retrieval"] filter.
    let (results, _meta) = search_svc
        .search(SearchRequest {
            query: "xkq92".into(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: Some(vec!["source".into()]),
            limit: 20,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
            spreading_activation: None,
            facet_function: Some(vec!["retrieval".into()]),
            facet_scope: None,
        })
        .await
        .expect("search should succeed");

    let result_ids: Vec<_> = results.iter().map(|r| r.node_id).collect();

    assert!(
        result_ids.contains(&retrieval_resp.id),
        "retrieval-faceted source must appear in filtered results"
    );
    assert!(
        !result_ids.contains(&storage_resp.id),
        "storage-faceted source must NOT appear when filtering for facet_function=[retrieval]"
    );

    fix.cleanup().await;
}

// ─── test_backward_compat ─────────────────────────────────────────────────────

/// AC3: Sources without facets (NULL) are unaffected — they can be ingested,
/// fetched, and searched without error.  NULL facets do not interfere with
/// searches that carry no facet filter.
#[tokio::test]
#[serial]
async fn test_backward_compat() {
    let mut fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());
    let search_svc = SearchService::new(fix.pool.clone());
    search_svc.init().await;

    // Ingest without facets — mimics a pre-92 source.
    let resp = svc
        .ingest(ingest_req(
            "no-facets-compat-unique-bk77z",
            "Legacy source without any facet classification.",
            None,
            None,
        ))
        .await
        .expect("ingest without facets should succeed");
    fix.track(resp.id);

    // Facets must be None in response.
    assert!(
        resp.facet_function.is_none(),
        "facet_function must be None when not provided"
    );
    assert!(
        resp.facet_scope.is_none(),
        "facet_scope must be None when not provided"
    );

    // Re-fetch — facets still None.
    let fetched = svc.get(resp.id).await.expect("get should succeed");
    assert!(
        fetched.facet_function.is_none(),
        "fetched facet_function must still be None"
    );
    assert!(
        fetched.facet_scope.is_none(),
        "fetched facet_scope must still be None"
    );

    // A search with NO facet filter must include the legacy source.
    let (results, _meta) = search_svc
        .search(SearchRequest {
            query: "bk77z".into(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: Some(vec!["source".into()]),
            limit: 20,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
            spreading_activation: None,
            facet_function: None,
            facet_scope: None,
        })
        .await
        .expect("search without facet filter should succeed");

    let result_ids: Vec<_> = results.iter().map(|r| r.node_id).collect();
    assert!(
        result_ids.contains(&resp.id),
        "source without facets must appear in results when no facet filter is active"
    );

    fix.cleanup().await;
}
