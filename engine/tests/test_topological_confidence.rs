//! Integration tests for topological confidence scoring (Phase 6, covalence#53).

use std::collections::HashMap;
use uuid::Uuid;

use covalence_engine::graph::confidence::compute_topological_confidence;
use covalence_engine::graph::memory::CovalenceGraph;

/// A node with no inbound edges and absent from the pagerank map should score 0.
#[test]
fn test_isolated_node_has_zero_topo_score() {
    let g = CovalenceGraph::new();
    let isolated = Uuid::new_v4();

    // Empty pagerank map and no graph edges → all signals are zero.
    let tc = compute_topological_confidence(&isolated, &HashMap::new(), &g);

    assert_eq!(
        tc.pagerank, 0.0,
        "isolated node should have raw pagerank 0.0"
    );
    assert_eq!(
        tc.path_diversity, 0,
        "isolated node should have 0 inbound edges"
    );
    assert_eq!(
        tc.score, 0.0,
        "isolated node topological score should be 0.0"
    );
}

/// A well-connected node (many inbound edges) should score higher than an isolated one.
#[test]
fn test_well_connected_node_has_higher_score() {
    let mut g = CovalenceGraph::new();
    let hub = Uuid::new_v4();

    // Add 10 nodes that all point to `hub`, giving it 10 inbound edges.
    for _ in 0..10 {
        let src = Uuid::new_v4();
        g.add_edge(src, hub, "ORIGINATES".to_string());
    }

    // PageRank map is empty — only path diversity contributes.
    let scores: HashMap<Uuid, f64> = HashMap::new();
    let tc_hub = compute_topological_confidence(&hub, &scores, &g);

    // path_norm = 1 - e^(-10/5) = 1 - e^(-2) ≈ 0.8647
    // score = 0.6 * 0.0 + 0.4 * 0.8647 ≈ 0.3459
    let expected_path_norm = 1.0 - (-10.0_f64 / 5.0).exp();
    let expected_score = 0.4 * expected_path_norm;

    assert_eq!(tc_hub.path_diversity, 10);
    assert!(
        (tc_hub.score - expected_score).abs() < 1e-9,
        "hub score {:.6} != expected {:.6}",
        tc_hub.score,
        expected_score
    );

    // Compare with an isolated node.
    let isolated = Uuid::new_v4();
    let tc_isolated = compute_topological_confidence(&isolated, &scores, &g);
    assert!(
        tc_hub.score > tc_isolated.score,
        "well-connected node must outscore isolated node"
    );
}

/// PageRank normalisation: the highest-ranked node should get normalised PR = 1.0
/// and a lower-ranked node should be proportional.
#[test]
fn test_pagerank_normalization() {
    let g = CovalenceGraph::new();

    let node_a = Uuid::new_v4();
    let node_b = Uuid::new_v4();

    // Raw PageRank scores: node_a = 0.5 (max), node_b = 0.1.
    let mut pr_scores: HashMap<Uuid, f64> = HashMap::new();
    pr_scores.insert(node_a, 0.5);
    pr_scores.insert(node_b, 0.1);

    let tc_a = compute_topological_confidence(&node_a, &pr_scores, &g);
    let tc_b = compute_topological_confidence(&node_b, &pr_scores, &g);

    // Neither node is in the graph, so path_diversity = 0 → path_norm = 0.
    // node_a: normalised_pr = 0.5 / 0.5 = 1.0 → score = 0.6 * 1.0 = 0.60
    // node_b: normalised_pr = 0.1 / 0.5 = 0.2 → score = 0.6 * 0.2 = 0.12
    assert_eq!(tc_a.path_diversity, 0);
    assert_eq!(tc_b.path_diversity, 0);

    let expected_a = 0.6 * 1.0_f64;
    let expected_b = 0.6 * 0.2_f64;

    assert!(
        (tc_a.score - expected_a).abs() < 1e-9,
        "node_a score {:.6} != expected {:.6}",
        tc_a.score,
        expected_a
    );
    assert!(
        (tc_b.score - expected_b).abs() < 1e-9,
        "node_b score {:.6} != expected {:.6}",
        tc_b.score,
        expected_b
    );

    // Verify ordering.
    assert!(
        tc_a.score > tc_b.score,
        "higher-ranked node must have higher topological score"
    );
}
