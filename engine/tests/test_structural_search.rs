//! Integration tests for Phase 5: Structural Similarity Search (covalence#52).
//!
//! Tests cover:
//!   1. Feature-flag gate: adaptor returns empty when `COVALENCE_STRUCTURAL_SEARCH`
//!      is not `"true"`.
//!   2. `WeightsInput` accepts a `structural` field without error.
//!   3. `SearchStrategy::Structural` resolves to the expected strategy label.

use covalence_engine::search::dimension::{DimensionAdaptor, DimensionQuery};
use covalence_engine::search::structural::StructuralAdaptor;
use covalence_engine::services::search_service::{SearchStrategy, WeightsInput};
use uuid::Uuid;

// ─── Test 1: feature-flag gate ────────────────────────────────────────────────

/// When `COVALENCE_STRUCTURAL_SEARCH` is absent (or not `"true"`), the adaptor
/// returns an empty result set without touching the DB.
///
/// The test skips the pool-required path when no `DATABASE_URL` is set — the
/// unit-level env-var assertion still runs unconditionally.
#[tokio::test]
async fn test_structural_search_feature_flag_off() {
    // Ensure the flag is OFF for this test.
    // SAFETY: single-threaded test, no other threads read this var concurrently.
    unsafe {
        std::env::remove_var("COVALENCE_STRUCTURAL_SEARCH");
    }

    // Confirm env var is not set.
    assert_ne!(
        std::env::var("COVALENCE_STRUCTURAL_SEARCH").as_deref(),
        Ok("true"),
        "COVALENCE_STRUCTURAL_SEARCH must NOT be 'true' for this test"
    );

    // If a DB is available, exercise the full code path and confirm empty results.
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("DATABASE_URL not set — skipping DB path, env-var assertion passed");
            return;
        }
    };

    let pool = match sqlx::PgPool::connect(&db_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot connect to DB ({e}) — skipping DB path");
            return;
        }
    };

    let adaptor = StructuralAdaptor::new();
    let query = DimensionQuery {
        text: "test query".to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        max_hops: None,
        namespace: "default".to_string(),
    };
    // Use a fake anchor so the candidate list is non-empty.
    let fake_anchor = vec![Uuid::new_v4()];

    let results = adaptor
        .search(&pool, &query, Some(&fake_anchor), 10)
        .await
        .expect("search should not error");

    assert!(
        results.is_empty(),
        "structural adaptor must return empty results when feature flag is off, got {}",
        results.len()
    );
}

// ─── Test 2: WeightsInput accepts `structural` ────────────────────────────────

/// Verifies that `WeightsInput` has a `structural` field and that the service
/// accepts it without panicking or returning an error.
///
/// This is primarily a compile-time + API surface test.
#[test]
fn test_structural_search_weights_accepted() {
    // Construct WeightsInput with a structural weight — must compile cleanly.
    let weights = WeightsInput {
        vector: Some(0.4),
        lexical: Some(0.2),
        graph: Some(0.1),
        structural: Some(0.3),
    };

    // Basic sanity checks on the values.
    assert_eq!(weights.structural, Some(0.3));
    assert_eq!(weights.vector, Some(0.4));
    assert_eq!(weights.lexical, Some(0.2));
    assert_eq!(weights.graph, Some(0.1));

    // Weights sum to 1.0 (within float tolerance).
    let sum = weights.vector.unwrap_or(0.0)
        + weights.lexical.unwrap_or(0.0)
        + weights.graph.unwrap_or(0.0)
        + weights.structural.unwrap_or(0.0);
    assert!(
        (sum - 1.0_f32).abs() < 1e-5,
        "weights should sum to 1.0, got {sum}"
    );

    // Optional fields can be None — must still compile.
    let weights_partial = WeightsInput {
        vector: None,
        lexical: None,
        graph: None,
        structural: Some(0.5),
    };
    assert_eq!(weights_partial.structural, Some(0.5));
}

// ─── Test 3: SearchStrategy::Structural resolves correctly ────────────────────

/// Verifies that the `Structural` strategy variant exists and that a search
/// using it reports `strategy: "structural"` in the response metadata.
///
/// When no DB is available the test confirms the variant compiles and
/// serialises correctly, then returns early.
#[tokio::test]
async fn test_structural_strategy_preset() {
    // Confirm the variant exists and can be constructed.
    let strategy = SearchStrategy::Structural;

    // Verify the variant serialises to the expected snake_case string.
    let serialised = serde_json::to_string(&strategy).expect("serialise SearchStrategy");
    assert_eq!(
        serialised, "\"structural\"",
        "SearchStrategy::Structural should serialise to \"structural\", got {serialised}"
    );

    // If a DB is available, run a real search and check the meta label.
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!(
                "DATABASE_URL not set — serialisation assertion passed, skipping live search"
            );
            return;
        }
    };

    let pool = match sqlx::PgPool::connect(&db_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot connect to DB ({e}) — skipping live search");
            return;
        }
    };

    use covalence_engine::services::search_service::{SearchRequest, SearchService};

    let service = SearchService::new(pool);

    let req = SearchRequest {
        query: "structural similarity test".to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 5,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: Some(SearchStrategy::Structural),
        max_hops: None,
        after: None,
        before: None,
        min_score: None,
        spreading_activation: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (_results, meta) = service.search(req).await.expect("search should not error");

    assert_eq!(
        meta.strategy, "structural",
        "strategy label should be 'structural', got '{}'",
        meta.strategy
    );
}
