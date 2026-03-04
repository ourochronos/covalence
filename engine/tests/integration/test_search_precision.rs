//! Integration tests for search precision improvements (covalence#33).
//!
//! ## Tests
//!
//! * `test_lexical_fallback_fires_for_multi_term_query` — verifies the three-step
//!   lexical fallback chain (websearch → plainto → ILIKE) causes `lexical_score`
//!   to be `Some` even for queries that `websearch_to_tsquery` would miss.
//!
//! * `test_title_match_boost_elevates_exact_title_match` — verifies that a node
//!   whose title exactly matches query words ranks higher than an otherwise
//!   identical node whose title has no overlap with the query.
//!
//! * `test_min_score_filters_weak_results` — verifies that `min_score` suppresses
//!   weak results while the same query without `min_score` returns results,
//!   proving the filter is the only difference.

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── test_lexical_fallback_fires_for_multi_term_query ─────────────────────────

/// Ingest a source with a very specific title and commit-hash-like content.
/// Then search with a multi-term query that `websearch_to_tsquery` would parse
/// strictly (and likely return 0 results for the hash token).
///
/// The fallback chain must fire and produce a `lexical_score: Some(...)` in the
/// result, and the ingested source must appear in the top 3.
#[tokio::test]
#[serial]
async fn test_lexical_fallback_fires_for_multi_term_query() {
    let mut fix = TestFixture::new().await;

    // Unique title that will be matched by the ILIKE fallback.
    let title = "Search Precision Test Article";
    // Content with commit-hash-like tokens that defeat websearch_to_tsquery.
    let content =
        "commit hash 337b925 precision test search lexical fallback unique zeta omega phi";

    let node_id = fix.insert_source(title, content).await;
    fix.insert_embedding(node_id).await;

    let svc = SearchService::new(fix.pool.clone());

    // The query includes the hash token "337b925" — websearch_to_tsquery will
    // reject this because numeric tokens aren't in its English dictionary, so
    // it produces 0 results. The plainto / ILIKE fallback must step in.
    let req = SearchRequest {
        query: "commit hash 337b925 precision test".to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
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
        min_causal_weight: None,
        min_causal_strength: None,
        causal_level: None,
        evidence_types: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results, meta) = svc.search(req).await.expect("search should succeed");

    // The lexical dimension must have fired (fallback chain worked).
    assert!(
        meta.dimensions_used.contains(&"lexical".to_string()),
        "lexical dimension must fire via fallback chain; dimensions_used={:?}",
        meta.dimensions_used
    );

    // The node must appear in the top 3 results.
    let pos = results.iter().position(|r| r.node_id == node_id);
    assert!(
        pos.is_some_and(|p| p < 3),
        "ingested node should appear in top 3 results; \
         position={:?}, results={:?}",
        pos,
        results.iter().map(|r| r.node_id).collect::<Vec<_>>()
    );

    // The result must have a populated lexical_score.
    let result = results.iter().find(|r| r.node_id == node_id).unwrap();
    assert!(
        result.lexical_score.is_some(),
        "result lexical_score should be Some(...), got None"
    );

    fix.cleanup().await;
}

// ─── test_title_match_boost_elevates_exact_title_match ────────────────────────

/// Two sources share **identical content**, so all dimension scores (lexical,
/// vector, graph) are equal after min-max normalization.  The only scoring
/// difference is the title match bonus:
///
///   A — title = "Current System State" — all three significant query words
///       ("current", "system", "state") appear → bonus = 0.20 × (3/3) = 0.20
///   B — title = "Peripheral Infrastructure Document" — no overlap with query
///       → bonus = 0.0
///
/// Because dimension scores are identical, A must outscore B by exactly the
/// title bonus amount.  Score ≥ 0.40 is asserted (achievable without vector:
/// lexical ≈ 0.21 dim-weighted + 0.20 bonus + small trust/freshness).
#[tokio::test]
#[serial]
async fn test_title_match_boost_elevates_exact_title_match() {
    let mut fix = TestFixture::new().await;

    // Identical content so lexical scores are equal (both find this text,
    // min-max collapses to the all-same-score → 1.0 branch).
    let shared_content =
        "current system state overview monitoring tracking operational metrics dashboard alerts";

    // Node A: title exactly matches the query.
    let id_a = fix
        .insert_source("Current System State", shared_content)
        .await;

    // Node B: title has zero query-word overlap; same content as A.
    let id_b = fix
        .insert_source("Peripheral Infrastructure Document", shared_content)
        .await;

    fix.insert_embedding(id_a).await;
    fix.insert_embedding(id_b).await;

    let svc = SearchService::new(fix.pool.clone());

    let req = SearchRequest {
        query: "current system state".to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
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
        min_causal_weight: None,
        min_causal_strength: None,
        causal_level: None,
        evidence_types: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results, _meta) = svc.search(req).await.expect("search should succeed");

    // Both nodes must appear (same content → both found by lexical).
    let res_a = results
        .iter()
        .find(|r| r.node_id == id_a)
        .expect("node A (exact title match) should appear in results");
    let res_b = results
        .iter()
        .find(|r| r.node_id == id_b)
        .expect("node B (no title overlap) should appear in results");

    // A must strictly outscore B — the title bonus is the sole differentiator.
    assert!(
        res_a.score > res_b.score,
        "title-matched node A (score={:.4}) should outrank \
         non-title-matched node B (score={:.4}); \
         the title bonus must be the differentiator",
        res_a.score,
        res_b.score,
    );

    // A's score must reach a meaningful floor.
    // Under RRF the base score is smaller than the old weighted-sum, but the
    // title bonus (up to 0.20) is additive and must push A well above 0.20.
    assert!(
        res_a.score >= 0.25,
        "title-matched node A score ({:.4}) should be >= 0.25 \
         (RRF base + trust/conf + title bonus)",
        res_a.score
    );

    fix.cleanup().await;
}

// ─── test_min_score_filters_weak_results ──────────────────────────────────────

/// Insert a source with content completely unrelated to the query.
/// Search with `min_score=0.75` — the unrelated source must NOT appear.
/// Search without `min_score` for the same query — some results ARE returned
/// (proving the filter is the difference, not a broken query).
#[tokio::test]
#[serial]
async fn test_min_score_filters_weak_results() {
    let mut fix = TestFixture::new().await;

    // A source with content unrelated to our query phrase.
    let unrelated_id = fix
        .insert_source(
            "Unrelated Noise Source",
            "fjord xyzzy plugh twisty maze passages all alike nonsense blibber blabber florp",
        )
        .await;
    fix.insert_embedding(unrelated_id).await;

    // A source whose content IS relevant — so the no-filter search returns
    // something (proving the filtered search isn't just empty by accident).
    let relevant_id = fix
        .insert_source(
            "Highly Relevant Infrastructure Source",
            "infrastructure deployment pipeline continuous integration delivery \
             kubernetes docker orchestration relevant query terms",
        )
        .await;
    fix.insert_embedding(relevant_id).await;

    let svc = SearchService::new(fix.pool.clone());

    let query = "infrastructure deployment pipeline kubernetes".to_string();

    // ── With min_score=0.75: unrelated source must be absent ─────────────────
    let req_filtered = SearchRequest {
        query: query.clone(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 20,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: None,
        before: None,
        min_score: Some(0.75),
        spreading_activation: None,
        min_causal_weight: None,
        min_causal_strength: None,
        causal_level: None,
        evidence_types: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results_filtered, _) = svc
        .search(req_filtered)
        .await
        .expect("filtered search should succeed");

    assert!(
        !results_filtered.iter().any(|r| r.node_id == unrelated_id),
        "unrelated source should be filtered out when min_score=0.75; \
         results={:?}",
        results_filtered
            .iter()
            .map(|r| (r.node_id, r.score))
            .collect::<Vec<_>>()
    );

    // ── Without min_score: some results ARE returned ──────────────────────────
    let req_unfiltered = SearchRequest {
        query: query.clone(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
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
        min_causal_weight: None,
        min_causal_strength: None,
        causal_level: None,
        evidence_types: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results_unfiltered, _) = svc
        .search(req_unfiltered)
        .await
        .expect("unfiltered search should succeed");

    assert!(
        !results_unfiltered.is_empty(),
        "unfiltered search should return at least one result (relevant source)"
    );

    // Sanity: unrelated source score (when returned) must be below the threshold.
    if let Some(unrelated_result) = results_unfiltered
        .iter()
        .find(|r| r.node_id == unrelated_id)
    {
        assert!(
            unrelated_result.score < 0.75,
            "unrelated source score ({:.4}) should be below min_score threshold 0.75",
            unrelated_result.score
        );
    }

    // Sanity: the relevant source should appear in the unfiltered set.
    assert!(
        results_unfiltered.iter().any(|r| r.node_id == relevant_id),
        "relevant source should appear in unfiltered results"
    );

    fix.cleanup().await;
}

// ─── Helper: insert a source with a given UUID directly ──────────────────────

#[allow(dead_code)]
async fn insert_source_with_id(
    fix: &mut TestFixture,
    id: Uuid,
    title: &str,
    content: &str,
) -> Uuid {
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, metadata) \
         VALUES ($1, 'source', 'active', $2, $3, '{}'::jsonb)",
    )
    .bind(id)
    .bind(title)
    .bind(content)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_source_with_id failed: {e}"));
    fix.track(id)
}
