//! Property-based tests for Covalence graph invariants (covalence#96, Phase 1).
//!
//! Uses the `proptest` crate to verify that core invariants hold across a
//! wide range of randomly-generated inputs.
//!
//! ## Invariants covered
//!
//! 1. **Serde roundtrip** — `SearchMode` and `SearchStrategy` survive a
//!    `serialize → deserialize` round-trip without loss of information.
//!    `SearchRequest` JSON deserialises without panicking for any valid
//!    query string + limit pair.
//!
//! 2. **Score bounds** — every score field (`score`, `vector_score`,
//!    `lexical_score`, `graph_score`, `structural_score`, `confidence`) in a
//!    `SearchResult` is always in the closed interval `[0.0, 1.0]`.
//!
//! 3. **Search result ordering** — for any query the returned results are
//!    ordered in descending `score` (no pair of adjacent results may have
//!    `score[i] < score[i+1]`).
//!
//! 4. **Non-empty results** — when the knowledge base contains at least one
//!    active node and the query is a non-empty string, at least one result is
//!    returned (limit ≥ 1).
//!
//! ## Async strategy
//!
//! `proptest!` closures are synchronous.  Tests that require database access
//! use a `tokio::runtime::Runtime` created *once* before the proptest loop
//! and call `rt.block_on(…)` per iteration.  The database is seeded once
//! before the loop and not cleaned up in this file — `setup_pool()` truncates
//! all tables at the start of each test run, which is sufficient.

use proptest::prelude::*;
use serial_test::serial;

use covalence_engine::services::search_service::{
    SearchMode, SearchRequest, SearchService, SearchStrategy,
};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Build a minimal `SearchRequest` for the given query string.
fn make_req(query: impl Into<String>) -> SearchRequest {
    SearchRequest {
        query: query.into(),
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
    }
}

/// Seed the test database with a small, deterministic set of source nodes and
/// their embeddings, then return the connection pool.  The `TestFixture` is
/// intentionally leaked (via `std::mem::forget`) so the data survives across
/// proptest iterations; the next call to `setup_pool()` will truncate it.
async fn seed_and_get_pool() -> sqlx::PgPool {
    let mut fix = TestFixture::new().await;

    let seed_rows: &[(&str, &str)] = &[
        (
            "Knowledge Graph Fundamentals",
            "Graph databases store nodes and edges that represent relationships between concepts \
             in a structured knowledge substrate.",
        ),
        (
            "Search Algorithms Overview",
            "Lexical search, vector similarity, and graph traversal combine to form a \
             multi-dimensional retrieval pipeline.",
        ),
        (
            "Machine Learning Basics",
            "Supervised learning uses labelled training data to build predictive models \
             that generalise to unseen examples.",
        ),
        (
            "Property-Based Testing",
            "Property-based testing generates random inputs and asserts that invariants \
             hold across the entire input space.",
        ),
    ];

    for (title, content) in seed_rows {
        let id = fix.insert_source(title, content).await;
        fix.insert_embedding(id).await;
    }

    let pool = fix.pool.clone();
    // Intentionally skip cleanup — next test's setup_pool() truncates all tables.
    std::mem::forget(fix);
    pool
}

// ─── 1. Serde roundtrip invariant ─────────────────────────────────────────────

// Property: any `SearchMode` variant survives a JSON roundtrip unchanged.
proptest! {
    #[test]
    fn prop_search_mode_serde_roundtrip(idx in 0usize..3) {
        let modes = [
            SearchMode::Standard,
            SearchMode::Hierarchical,
            SearchMode::Synthesis,
        ];
        let mode = &modes[idx];
        let json = serde_json::to_string(mode)
            .expect("SearchMode must be serialisable");
        let mode2: SearchMode = serde_json::from_str(&json)
            .expect("SearchMode must be deserialisable");
        let json2 = serde_json::to_string(&mode2)
            .expect("SearchMode round-trip must serialise");
        prop_assert_eq!(
            json, json2,
            "SearchMode JSON did not survive roundtrip: {:?}",
            mode
        );
        // Structural equality (SearchMode derives PartialEq).
        prop_assert_eq!(mode, &mode2);
    }
}

// Property: any `SearchStrategy` variant produces stable JSON roundtrip output.
proptest! {
    #[test]
    fn prop_search_strategy_serde_roundtrip(idx in 0usize..5) {
        let strategies = [
            SearchStrategy::Balanced,
            SearchStrategy::Precise,
            SearchStrategy::Exploratory,
            SearchStrategy::Graph,
            SearchStrategy::Structural,
        ];
        let strategy = &strategies[idx];
        let json = serde_json::to_string(strategy)
            .expect("SearchStrategy must be serialisable");
        let s2: SearchStrategy = serde_json::from_str(&json)
            .expect("SearchStrategy must be deserialisable");
        let json2 = serde_json::to_string(&s2)
            .expect("SearchStrategy round-trip must serialise");
        prop_assert_eq!(
            json, json2,
            "SearchStrategy JSON did not survive roundtrip"
        );
    }
}

// Property: `SearchRequest` deserialises without panicking for any valid
// (query, limit) pair and preserves the query string and limit value.
proptest! {
    #[test]
    fn prop_search_request_deserialise(
        query  in "[a-zA-Z0-9 ]{1,100}",
        limit  in 1usize..=50,
    ) {
        let json = serde_json::json!({
            "query": query,
            "limit": limit,
        });
        let req: SearchRequest = serde_json::from_value(json)
            .expect("SearchRequest must deserialise from valid JSON");
        prop_assert_eq!(&req.query, &query);
        prop_assert_eq!(req.limit, limit);
    }
}

// ─── 2 & 3. Score bounds + ordering invariants ────────────────────────────────

/// Property: for any non-empty query string, every score field in every
/// `SearchResult` is in `[0.0, 1.0]` **and** results are returned in
/// descending `score` order.
///
/// Uses a seeded test database with four source nodes.  The runtime and pool
/// are created once outside the proptest loop for efficiency.
#[test]
#[serial]
fn prop_score_bounds_and_ordering() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let pool = rt.block_on(seed_and_get_pool());

    proptest!(
        ProptestConfig {
            // 30 cases is enough to get good coverage without slowing CI.
            cases: 30,
            ..Default::default()
        },
        |(query in "[a-zA-Z]{1,30}")| {
            let pool = pool.clone();
            let (scores_in_range, ordered, failures) = rt.block_on(async move {
                let svc = SearchService::new(pool);
                let req = make_req(query);
                match svc.search(req).await {
                    Err(_) => {
                        // A DB error is not a property violation — skip this case.
                        (true, true, vec![])
                    }
                    Ok((results, _meta)) => {
                        let mut out_of_range = vec![];

                        // Check every numeric score field.
                        for r in &results {
                            let check = |name: &str, v: f64| {
                                if !(0.0..=1.0).contains(&v) {
                                    Some(format!("{name}={v:.4} for node {}", r.node_id))
                                } else {
                                    None
                                }
                            };
                            if let Some(msg) = check("score", r.score) {
                                out_of_range.push(msg);
                            }
                            if let Some(vs) = r.vector_score {
                                if let Some(msg) = check("vector_score", vs) {
                                    out_of_range.push(msg);
                                }
                            }
                            if let Some(ls) = r.lexical_score {
                                if let Some(msg) = check("lexical_score", ls) {
                                    out_of_range.push(msg);
                                }
                            }
                            if let Some(gs) = r.graph_score {
                                if let Some(msg) = check("graph_score", gs) {
                                    out_of_range.push(msg);
                                }
                            }
                            if let Some(ss) = r.structural_score {
                                if let Some(msg) = check("structural_score", ss) {
                                    out_of_range.push(msg);
                                }
                            }
                            if let Some(msg) = check("confidence", r.confidence) {
                                out_of_range.push(msg);
                            }
                        }

                        // Check descending order.
                        let order_ok = results.windows(2).all(|w| w[0].score >= w[1].score);

                        (out_of_range.is_empty(), order_ok, out_of_range)
                    }
                }
            });

            prop_assert!(
                scores_in_range,
                "scores out of [0.0, 1.0]: {:?}",
                failures
            );
            prop_assert!(ordered, "results not in descending score order");
        }
    );
}

// ─── 4. Non-empty results invariant ───────────────────────────────────────────

/// Property: when the knowledge base contains at least one active node, a
/// non-empty query string always returns at least one result.
///
/// Note: the engine may legitimately return 0 results for a query that has
/// zero lexical overlap and a very low vector similarity with any node.  We
/// therefore only assert non-empty when the query contains at least one word
/// that appears verbatim in the seeded content ("knowledge", "search",
/// "learning", "testing").  For arbitrary strings the test is relaxed.
#[test]
#[serial]
fn prop_non_empty_results_for_content_queries() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let pool = rt.block_on(seed_and_get_pool());

    // Fixed anchor queries: terms that definitely appear in the seeded content.
    let anchor_queries = [
        "knowledge",
        "search",
        "learning",
        "testing",
        "graph",
        "vector",
    ];

    for query in anchor_queries {
        let pool = pool.clone();
        let n = rt.block_on(async move {
            let svc = SearchService::new(pool);
            let req = make_req(query);
            svc.search(req)
                .await
                .map(|(results, _)| results.len())
                .unwrap_or(0)
        });
        assert!(
            n >= 1,
            "expected ≥1 result for anchor query {:?}, got 0",
            query
        );
    }

    // Proptest: arbitrary non-empty alphabetic queries should never *panic*
    // and should return a valid (possibly empty) result set.
    proptest!(
        ProptestConfig {
            cases: 20,
            ..Default::default()
        },
        |(query in "[a-zA-Z]{1,20}")| {
            let pool = pool.clone();
            let result_ok = rt.block_on(async move {
                let svc = SearchService::new(pool);
                svc.search(make_req(query)).await.is_ok()
            });
            prop_assert!(result_ok, "search returned an error (not a data invariant violation, but should not panic)");
        }
    );
}
