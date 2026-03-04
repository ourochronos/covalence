//! Integration tests for the `explain=true` search flag (covalence#118).
//!
//! ## Tests
//!
//! * `test_search_default_no_explanation` — omitting `explain` (or `explain=false`)
//!   means every result's `explanation` field is `None`.
//!
//! * `test_search_explain_true_returns_scores` — `explain=true` populates an
//!   `explanation` object on each result with all four sub-score fields
//!   present and in [0.0, 1.0], plus the effective fusion weights.

use serial_test::serial;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── test_search_default_no_explanation ──────────────────────────────────────

/// When `explain` is absent from the request, every result's `explanation`
/// field must be `None`.
#[tokio::test]
#[serial]
async fn test_search_default_no_explanation() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_source(
            "Explain Flag Default Test",
            "covalence explain flag default absent no explanation object should be none",
        )
        .await;
    fix.insert_embedding(node_id).await;

    let svc = SearchService::new(fix.pool.clone());

    // No explain field → defaults to false.
    let req = SearchRequest {
        query: "explain flag default absent no explanation".to_string(),
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

    assert!(
        !results.is_empty(),
        "search must return at least one result for the seeded node"
    );

    // Every result must have explanation = None when explain is not set.
    for result in &results {
        assert!(
            result.explanation.is_none(),
            "result {:?} should have explanation=None when explain is not set",
            result.node_id
        );
    }

    fix.cleanup().await;
}

// ─── test_search_explain_true_returns_scores ─────────────────────────────────

/// When `explain=true`, every result must carry an `explanation` with all four
/// sub-score fields present and within [0.0, 1.0], plus non-negative weights
/// that sum to approximately 1.0.
#[tokio::test]
#[serial]
async fn test_search_explain_true_returns_scores() {
    let mut fix = TestFixture::new().await;

    let node_id = fix
        .insert_source(
            "Explain Flag True Test",
            "covalence explain true per-dimension sub-scores vector lexical graph structural",
        )
        .await;
    fix.insert_embedding(node_id).await;

    let svc = SearchService::new(fix.pool.clone());

    let req = SearchRequest {
        query: "explain true per-dimension sub-scores".to_string(),
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
        explain: Some(true),
    };

    let (results, _meta) = svc.search(req).await.expect("search should succeed");

    assert!(
        !results.is_empty(),
        "search must return at least one result for the seeded node"
    );

    // The seeded node must appear.
    let seeded = results
        .iter()
        .find(|r| r.node_id == node_id)
        .expect("seeded node should appear in results");

    // Explanation must be present.
    let expl = seeded
        .explanation
        .as_ref()
        .expect("explanation should be Some when explain=true");

    // All four sub-score fields must be present and in [0.0, 1.0].
    // (At minimum lexical should fire because we have matching content.)
    let scores = [
        ("vector_score", expl.vector_score),
        ("lexical_score", expl.lexical_score),
        ("graph_score", expl.graph_score),
        ("structural_score", expl.structural_score),
    ];
    for (name, score_opt) in &scores {
        if let Some(score) = score_opt {
            assert!(
                *score >= 0.0 && *score <= 1.0,
                "{name} value {score} is outside [0.0, 1.0]"
            );
        }
    }

    // At least one sub-score must be Some (the result appeared, so something scored it).
    let any_score = scores.iter().any(|(_, s)| s.is_some());
    assert!(
        any_score,
        "at least one of vector/lexical/graph/structural scores should be Some"
    );

    // Dimension weights must all be non-negative and sum to ~1.0.
    let dw = &expl.dimension_weights;
    assert!(dw.vector >= 0.0, "vector weight must be >= 0");
    assert!(dw.lexical >= 0.0, "lexical weight must be >= 0");
    assert!(dw.graph >= 0.0, "graph weight must be >= 0");
    assert!(dw.structural >= 0.0, "structural weight must be >= 0");

    let weight_sum = dw.vector + dw.lexical + dw.graph + dw.structural;
    assert!(
        (weight_sum - 1.0_f32).abs() < 0.01,
        "dimension weights should sum to ~1.0 (got {weight_sum:.4})"
    );

    // Verify ALL results have explanations when explain=true.
    for result in &results {
        assert!(
            result.explanation.is_some(),
            "all results should have explanation when explain=true; \
             result {:?} had None",
            result.node_id
        );
    }

    fix.cleanup().await;
}
