//! Tests for admin service types and knowledge gap computation.

use super::*;
use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};

fn make_node(name: &str, ntype: &str) -> NodeMeta {
    NodeMeta {
        id: uuid::Uuid::new_v4(),
        node_type: ntype.into(),
        entity_class: None,
        canonical_name: name.into(),
        clearance_level: 0,
    }
}

fn make_edge() -> EdgeMeta {
    EdgeMeta {
        id: uuid::Uuid::new_v4(),
        rel_type: "related_to".into(),
        weight: 1.0,
        confidence: 0.9,
        causal_level: None,
        clearance_level: 0,
        is_synthetic: false,
        has_valid_from: false,
    }
}

/// Build a graph with a clear knowledge gap: "Subjective Logic"
/// has 4 incoming edges (referenced by A, B, C, D) but 0
/// outgoing edges.
fn build_gap_graph() -> GraphSidecar {
    let mut g = GraphSidecar::new();

    let gap_node = make_node("Subjective Logic", "concept");
    let gap_id = gap_node.id;
    g.add_node(gap_node).unwrap();

    // 4 nodes that reference the gap node.
    for name in &[
        "Epistemic Model",
        "Opinion Fusion",
        "Trust Framework",
        "Dempster-Shafer",
    ] {
        let n = make_node(name, "concept");
        let nid = n.id;
        g.add_node(n).unwrap();
        g.add_edge(nid, gap_id, make_edge()).unwrap();
    }

    // A well-explained node with both in and out edges.
    let explained = make_node("Bayesian Inference", "concept");
    let explained_id = explained.id;
    g.add_node(explained).unwrap();
    g.add_edge(explained_id, gap_id, make_edge()).unwrap();

    // Give "Bayesian Inference" outgoing edges so it's NOT a gap.
    let target = make_node("Probability Theory", "concept");
    let target_id = target.id;
    g.add_node(target).unwrap();
    g.add_edge(explained_id, target_id, make_edge()).unwrap();

    g
}

#[test]
fn detect_knowledge_gap() {
    let g = build_gap_graph();
    let candidates = compute_gap_candidates(
        g.graph(),
        3, // min_in_degree
        4, // min_label_length
        &[],
        20,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].1, "Subjective Logic");
    assert_eq!(candidates[0].3, 5); // in_degree
    assert_eq!(candidates[0].4, 0); // out_degree
}

#[test]
fn min_in_degree_filter() {
    let g = build_gap_graph();

    // Require 6 in-degree — no gaps qualify.
    let candidates = compute_gap_candidates(g.graph(), 6, 4, &[], 20);
    assert!(candidates.is_empty());

    // Require 5 — exactly matches.
    let candidates = compute_gap_candidates(g.graph(), 5, 4, &[], 20);
    assert_eq!(candidates.len(), 1);
}

#[test]
fn exclude_types_filter() {
    let g = build_gap_graph();

    // Exclude "concept" — no gaps.
    let exclude = vec!["concept".to_string()];
    let candidates = compute_gap_candidates(g.graph(), 3, 4, &exclude, 20);
    assert!(candidates.is_empty());
}

#[test]
fn min_label_length_filter() {
    let mut g = GraphSidecar::new();
    let short = make_node("AI", "concept");
    let short_id = short.id;
    g.add_node(short).unwrap();

    // 3 nodes referencing "AI".
    for name in &["Machine Learning", "Deep Learning", "Neural Networks"] {
        let n = make_node(name, "concept");
        let nid = n.id;
        g.add_node(n).unwrap();
        g.add_edge(nid, short_id, make_edge()).unwrap();
    }

    // "AI" has 3 in-degree but name length < 4.
    let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
    assert!(candidates.is_empty());

    // With min_label_length=2, it shows up.
    let candidates = compute_gap_candidates(g.graph(), 3, 2, &[], 20);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].1, "AI");
}

#[test]
fn limit_truncates_results() {
    let mut g = GraphSidecar::new();

    // Create 5 gap nodes each with 3 incoming edges.
    for gap_name in &[
        "Alpha Gap",
        "Beta Gap",
        "Gamma Gap",
        "Delta Gap",
        "Epsilon Gap",
    ] {
        let gap = make_node(gap_name, "concept");
        let gap_id = gap.id;
        g.add_node(gap).unwrap();
        for i in 0..3 {
            let src = make_node(&format!("{gap_name}-ref-{i}"), "entity");
            let src_id = src.id;
            g.add_node(src).unwrap();
            g.add_edge(src_id, gap_id, make_edge()).unwrap();
        }
    }

    let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 2);
    assert_eq!(candidates.len(), 2);
}

#[test]
fn sorted_by_gap_score_descending() {
    let mut g = GraphSidecar::new();

    // "Small Gap" has 3 in-degree.
    let small = make_node("Small Gap Node", "concept");
    let small_id = small.id;
    g.add_node(small).unwrap();
    for i in 0..3 {
        let src = make_node(&format!("small-ref-{i}"), "entity");
        let src_id = src.id;
        g.add_node(src).unwrap();
        g.add_edge(src_id, small_id, make_edge()).unwrap();
    }

    // "Big Gap" has 6 in-degree.
    let big = make_node("Big Gap Node", "concept");
    let big_id = big.id;
    g.add_node(big).unwrap();
    for i in 0..6 {
        let src = make_node(&format!("big-ref-{i}"), "entity");
        let src_id = src.id;
        g.add_node(src).unwrap();
        g.add_edge(src_id, big_id, make_edge()).unwrap();
    }

    let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].1, "Big Gap Node");
    assert_eq!(candidates[1].1, "Small Gap Node");
}

#[test]
fn no_gap_when_out_degree_matches() {
    let mut g = GraphSidecar::new();

    // Node with 3 in and 3 out — not a gap.
    let balanced = make_node("Balanced Node", "concept");
    let balanced_id = balanced.id;
    g.add_node(balanced).unwrap();

    for i in 0..3 {
        let src = make_node(&format!("src-{i}"), "entity");
        let src_id = src.id;
        g.add_node(src).unwrap();
        g.add_edge(src_id, balanced_id, make_edge()).unwrap();

        let tgt = make_node(&format!("tgt-{i}"), "entity");
        let tgt_id = tgt.id;
        g.add_node(tgt).unwrap();
        g.add_edge(balanced_id, tgt_id, make_edge()).unwrap();
    }

    let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
    assert!(candidates.is_empty());
}

#[test]
fn empty_graph_returns_no_gaps() {
    let g = GraphSidecar::new();
    let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
    assert!(candidates.is_empty());
}

// --- GcResult tests ---

#[test]
fn gc_result_serializes_all_fields() {
    let result = GcResult {
        nodes_evicted: 5,
        edges_removed: 12,
        aliases_removed: 8,
    };
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["nodes_evicted"], 5);
    assert_eq!(json["edges_removed"], 12);
    assert_eq!(json["aliases_removed"], 8);
}

#[test]
fn gc_result_zero_counts() {
    let result = GcResult {
        nodes_evicted: 0,
        edges_removed: 0,
        aliases_removed: 0,
    };
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["nodes_evicted"], 0);
    assert_eq!(json["edges_removed"], 0);
    assert_eq!(json["aliases_removed"], 0);
}

#[test]
fn gc_result_debug_impl() {
    let result = GcResult {
        nodes_evicted: 3,
        edges_removed: 7,
        aliases_removed: 2,
    };
    let debug = format!("{result:?}");
    assert!(debug.contains("nodes_evicted: 3"));
    assert!(debug.contains("edges_removed: 7"));
    assert!(debug.contains("aliases_removed: 2"));
}

#[test]
fn gc_result_clone() {
    let result = GcResult {
        nodes_evicted: 10,
        edges_removed: 20,
        aliases_removed: 5,
    };
    let cloned = result.clone();
    assert_eq!(cloned.nodes_evicted, result.nodes_evicted);
    assert_eq!(cloned.edges_removed, result.edges_removed);
    assert_eq!(cloned.aliases_removed, result.aliases_removed);
}

#[test]
fn invalidated_edge_stats_serializes() {
    let stats = InvalidatedEdgeStats {
        total_invalidated: 23000,
        total_valid: 113000,
        top_types: vec![
            InvalidatedEdgeType {
                rel_type: "RELATED_TO".into(),
                count: 15000,
            },
            InvalidatedEdgeType {
                rel_type: "co_occurs".into(),
                count: 5000,
            },
        ],
        top_nodes: vec![InvalidatedEdgeNode {
            node_id: uuid::Uuid::new_v4(),
            canonical_name: "Entity Resolution".into(),
            node_type: "concept".into(),
            invalidated_edge_count: 42,
        }],
    };
    let json = serde_json::to_string(&stats).unwrap();
    assert!(json.contains("23000"));
    assert!(json.contains("113000"));
    assert!(json.contains("RELATED_TO"));
    assert!(json.contains("co_occurs"));
    assert!(json.contains("Entity Resolution"));
    assert!(json.contains("42"));
}
