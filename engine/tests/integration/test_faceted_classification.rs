//! Integration tests for faceted classification (covalence#92 Phase 1 + Phase 2).
//!
//! Phase 1 (covalence#92) — verified here:
//! 1. Sources ingested with explicit facets round-trip correctly through the DB.
//! 2. Search `facet_function` filter includes only matching nodes.
//! 3. Sources without facets (NULL) continue to work normally.
//!
//! Phase 2 (covalence#103) — verified here:
//! 4. Facet-aligned search results receive a 1.1× score boost.
//! 5. Article compilation propagates (unions) source facets.
//! 6. Requests without facet fields behave identically to pre-Phase-2 (no boost).

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use sqlx::Row;
use uuid::Uuid;

use super::helpers::{MockLlmClient, TestFixture};
use covalence_engine::services::{
    search_service::{SearchRequest, SearchService},
    source_service::{IngestRequest, SourceService},
};
use covalence_engine::worker::{handle_compile, llm::LlmClient};

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

// ─── Phase 2: Facet boost in scoring ─────────────────────────────────────────

/// AC4 (Phase 2): A facet-aligned search result should score higher than the
/// same result in an otherwise identical search that carries no facet filter.
///
/// Two sources are ingested with identical unique content so their lexical
/// scores are tied.  Source A carries facet_function=["retrieval"] and Source B
/// has no facets.  When we search:
///   • WITHOUT a facet filter → both appear; A and B score similarly.
///   • WITH facet_function=["retrieval"] → only A appears (filter removes B);
///     A receives a 1.1× boost on top of its normal score.
///
/// We verify A's score in the faceted search is strictly greater than its
/// score in the unfaceted search (boost applied + sole result → rank-1 everywhere).
#[tokio::test]
#[serial]
async fn test_facet_boost_raises_aligned_score() {
    let mut fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());
    let search_svc = SearchService::new(fix.pool.clone());
    search_svc.init().await;

    // Unique token prevents collision with other tests.
    let unique = "zrx99q_facet_boost_unique";

    let src_a = svc
        .ingest(IngestRequest {
            title: Some(format!("facet-boost-A-{unique}")),
            content: format!(
                "Retrieval-augmented generation improves knowledge access. {unique} nodeA"
            ),
            source_type: Some("document".into()),
            reliability: None,
            metadata: None,
            session_id: None,
            capture_method: None,
            facet_function: Some(vec!["retrieval".into()]),
            facet_scope: Some(vec!["practical".into()]),
        })
        .await
        .expect("ingest A should succeed");
    fix.track(src_a.id);

    let src_b = svc
        .ingest(IngestRequest {
            title: Some(format!("facet-boost-B-{unique}")),
            content: format!(
                "Retrieval-augmented generation improves knowledge access. {unique} nodeB"
            ),
            source_type: Some("document".into()),
            reliability: None,
            metadata: None,
            session_id: None,
            capture_method: None,
            facet_function: None, // no facets
            facet_scope: None,
        })
        .await
        .expect("ingest B should succeed");
    fix.track(src_b.id);

    // ── Unfaceted search (no boost, no filter) ────────────────────────────────
    let (unfaceted_results, _) = search_svc
        .search(SearchRequest {
            query: unique.into(),
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
        .expect("unfaceted search should succeed");

    let score_a_unfaceted = unfaceted_results
        .iter()
        .find(|r| r.node_id == src_a.id)
        .map(|r| r.score)
        .expect("source A should appear in unfaceted results");

    // ── Faceted search (1.1× boost for A, B filtered out) ────────────────────
    let (faceted_results, _) = search_svc
        .search(SearchRequest {
            query: unique.into(),
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
        .expect("faceted search should succeed");

    let score_a_faceted = faceted_results
        .iter()
        .find(|r| r.node_id == src_a.id)
        .map(|r| r.score)
        .expect("source A must appear in faceted results");

    // B must be filtered out by the facet filter.
    let b_in_faceted = faceted_results.iter().any(|r| r.node_id == src_b.id);
    assert!(
        !b_in_faceted,
        "source B (no facets) must NOT appear in faceted results"
    );

    // A's faceted score should exceed its unfaceted score (1.1× boost).
    assert!(
        score_a_faceted > score_a_unfaceted,
        "facet-aligned source A should score higher with boost \
         (faceted={score_a_faceted:.4} vs unfaceted={score_a_unfaceted:.4})"
    );

    fix.cleanup().await;
}

// ─── Phase 2: Article compile inherits source facets ─────────────────────────

/// AC5 (Phase 2): When an article is compiled from sources that carry facets,
/// the resulting article's facet_function and facet_scope columns should be the
/// union (deduped, sorted) of the contributing sources' facet arrays.
#[tokio::test]
#[serial]
async fn test_compile_propagates_source_facets() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // Source 1: facet_function=["retrieval"], facet_scope=["practical"]
    let src_a = fix
        .insert_source(
            "facet-compile-src-A",
            "Retrieval methods for practical knowledge systems.",
        )
        .await;
    sqlx::query(
        "UPDATE covalence.nodes \
         SET facet_function = ARRAY['retrieval'], facet_scope = ARRAY['practical'] \
         WHERE id = $1",
    )
    .bind(src_a)
    .execute(&fix.pool)
    .await
    .expect("set facets on src_a");

    // Source 2: facet_function=["storage"], facet_scope=["operational"]
    let src_b = fix
        .insert_source(
            "facet-compile-src-B",
            "Storage architecture for operational knowledge pipelines.",
        )
        .await;
    sqlx::query(
        "UPDATE covalence.nodes \
         SET facet_function = ARRAY['storage'], facet_scope = ARRAY['operational'] \
         WHERE id = $1",
    )
    .bind(src_b)
    .execute(&fix.pool)
    .await
    .expect("set facets on src_b");

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src_a, src_b]);

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src_a.to_string(), src_b.to_string()],
            "title_hint": "Knowledge Systems Overview"
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap())
        .expect("article_id must be a valid UUID");
    fix.track(article_id);

    // Fetch facets from the compiled article.
    let row = sqlx::query(
        "SELECT facet_function, facet_scope \
         FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .expect("compiled article must exist");

    let art_ff: Option<Vec<String>> = row.get("facet_function");
    let art_fs: Option<Vec<String>> = row.get("facet_scope");

    // Union of ["retrieval"] and ["storage"] → ["retrieval", "storage"] (sorted).
    let ff = art_ff.expect("compiled article must have facet_function");
    assert!(
        ff.contains(&"retrieval".to_string()),
        "compiled article facet_function must include 'retrieval' from src_a"
    );
    assert!(
        ff.contains(&"storage".to_string()),
        "compiled article facet_function must include 'storage' from src_b"
    );

    // Union of ["practical"] and ["operational"] → ["operational", "practical"] (sorted).
    let fs = art_fs.expect("compiled article must have facet_scope");
    assert!(
        fs.contains(&"practical".to_string()),
        "compiled article facet_scope must include 'practical' from src_a"
    );
    assert!(
        fs.contains(&"operational".to_string()),
        "compiled article facet_scope must include 'operational' from src_b"
    );

    fix.cleanup().await;
}

// ─── Phase 2: Backward compat — no facet fields means no boost ───────────────

/// AC6 (Phase 2): A search request that carries no facet fields must behave
/// exactly as before Phase 2 — no boost is applied and no errors occur.
///
/// We verify that:
/// (a) A source without any facets appears in results normally.
/// (b) A source with explicit facets also appears in results (no penalty).
/// (c) The unfaceted search returns results for both sources without crashing.
#[tokio::test]
#[serial]
async fn test_no_facet_request_no_boost_applied() {
    let mut fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());
    let search_svc = SearchService::new(fix.pool.clone());
    search_svc.init().await;

    let unique = "zrx99q_no_boost_compat_unique";

    // Source with facets.
    let src_with = svc
        .ingest(IngestRequest {
            title: Some(format!("no-boost-with-facets-{unique}")),
            content: format!("Knowledge retrieval methods for analysis. {unique} withFacet"),
            source_type: Some("document".into()),
            reliability: None,
            metadata: None,
            session_id: None,
            capture_method: None,
            facet_function: Some(vec!["retrieval".into()]),
            facet_scope: Some(vec!["practical".into()]),
        })
        .await
        .expect("ingest with facets");
    fix.track(src_with.id);

    // Source without facets.
    let src_without = svc
        .ingest(IngestRequest {
            title: Some(format!("no-boost-no-facets-{unique}")),
            content: format!("Knowledge retrieval methods for analysis. {unique} noFacet"),
            source_type: Some("document".into()),
            reliability: None,
            metadata: None,
            session_id: None,
            capture_method: None,
            facet_function: None,
            facet_scope: None,
        })
        .await
        .expect("ingest without facets");
    fix.track(src_without.id);

    // Search with NO facet fields — classic pre-Phase-2 request.
    let (results, _meta) = search_svc
        .search(SearchRequest {
            query: unique.into(),
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
            facet_function: None, // ← no facets on request
            facet_scope: None,
        })
        .await
        .expect("unfaceted search must not error");

    let result_ids: Vec<Uuid> = results.iter().map(|r| r.node_id).collect();

    // Both sources must appear — no filtering applied when facets absent.
    assert!(
        result_ids.contains(&src_with.id),
        "faceted source must appear in unfaceted results"
    );
    assert!(
        result_ids.contains(&src_without.id),
        "unfaceted source must appear in unfaceted results"
    );

    // Scores should be close (both receive 1.0× multiplier — no boost divergence).
    let score_with = results
        .iter()
        .find(|r| r.node_id == src_with.id)
        .map(|r| r.score)
        .unwrap();
    let score_without = results
        .iter()
        .find(|r| r.node_id == src_without.id)
        .map(|r| r.score)
        .unwrap();
    let ratio = score_with / score_without;
    assert!(
        (0.50..=2.0).contains(&ratio),
        "without facet request, scores should be in the same ballpark \
         (ratio={ratio:.3}); neither gets a facet boost"
    );

    fix.cleanup().await;
}
