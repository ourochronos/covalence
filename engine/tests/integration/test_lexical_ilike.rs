//! Integration tests for the ILIKE title fallback in the lexical search
//! dimension (covalence#122).
//!
//! ## Tests
//!
//! * `lexical_ilike_single_quote_escaping` — verifies that a search query
//!   containing words with apostrophes (e.g. "what's", "agent's") does NOT
//!   produce a SQL syntax error when Step C (ILIKE title fallback) fires.
//!   Before the fix, the unescaped single quote broke the dynamically-built
//!   SQL literal and caused a Postgres syntax error (~every 10 min in prod).

use serial_test::serial;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

/// Searching with an apostrophe-containing query must not crash with a SQL
/// syntax error when the ILIKE fallback (Step C) fires.
///
/// Setup:
///   • No nodes are inserted, so Steps A (websearch_to_tsquery) and B
///     (plainto_tsquery) both return 0 results, forcing Step C to execute.
///   • The query contains several words >4 chars that include apostrophes
///     ("what's", "agent's") — before the fix these produced broken SQL of
///     the form `title ILIKE '%what's%'`, triggering a Postgres parse error.
///
/// The test succeeds if `search()` returns `Ok(_)` (i.e. 200 OK equivalent)
/// rather than `Err(_)` (i.e. 500 Internal Server Error equivalent).
#[tokio::test]
#[serial]
async fn lexical_ilike_single_quote_escaping() {
    let fix = TestFixture::new().await;
    // Intentionally insert NO nodes — Steps A and B return 0 results,
    // guaranteeing the ILIKE fallback (Step C) is reached.

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        // Query words >4 chars with apostrophes: "what's" (6), "agent's" (7).
        // These are the characters that previously broke the ILIKE SQL literal.
        query: "what's the agent's state".to_string(),
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
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    // Must not return Err — a SQL syntax error would surface as an Err here,
    // corresponding to a 500 response from the /search HTTP endpoint.
    let result = svc.search(req).await;
    assert!(
        result.is_ok(),
        "search with apostrophe-containing query must not produce a SQL error \
         (covalence#122); got: {:?}",
        result.err()
    );

    fix.cleanup().await;
}
