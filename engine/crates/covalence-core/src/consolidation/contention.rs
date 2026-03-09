//! Contention detection for batch consolidation.
//!
//! Identifies node pairs connected by contradictory or contentious edges,
//! flagging unresolved epistemic conflicts for human review or further
//! consolidation.

use petgraph::stable_graph::StableDiGraph;
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::sidecar::{EdgeMeta, NodeMeta};

/// Relationship types considered contentious.
const CONTENTION_REL_TYPES: &[&str] = &["contradicts", "contends", "CONTRADICTS", "CONTENDS"];

/// A detected contention between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contention {
    /// UUID of the first node in the contention.
    pub node_a: Uuid,
    /// UUID of the second node in the contention.
    pub node_b: Uuid,
    /// UUID of the edge representing the contention.
    pub edge_id: Uuid,
    /// The relationship type (e.g. "contradicts", "contends").
    pub rel_type: String,
    /// Confidence score of the contentious edge.
    pub confidence: f64,
}

/// Detect all contentions in the graph.
///
/// Scans all edges for relationship types that indicate contradiction
/// or contention (e.g. `CONTRADICTS`, `CONTENDS`) and returns them
/// as structured `Contention` records.
pub fn detect_contentions(graph: &StableDiGraph<NodeMeta, EdgeMeta>) -> Vec<Contention> {
    let mut contentions = Vec::new();

    for edge_ref in graph.edge_references() {
        let meta = edge_ref.weight();
        let rel_lower = meta.rel_type.to_lowercase();

        if CONTENTION_REL_TYPES
            .iter()
            .any(|&rt| rt.to_lowercase() == rel_lower)
        {
            let source_node = &graph[edge_ref.source()];
            let target_node = &graph[edge_ref.target()];

            contentions.push(Contention {
                node_a: source_node.id,
                node_b: target_node.id,
                edge_id: meta.id,
                rel_type: meta.rel_type.clone(),
                confidence: meta.confidence,
            });
        }
    }

    contentions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::GraphSidecar;

    fn add_node(g: &mut GraphSidecar, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            canonical_name: name.into(),
            clearance_level: 0,
        })
        .unwrap();
        id
    }

    #[test]
    fn no_contentions_in_normal_graph() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        g.add_edge(
            a,
            b,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "related_to".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
            },
        )
        .unwrap();

        let contentions = detect_contentions(g.graph());
        assert!(contentions.is_empty());
    }

    #[test]
    fn detects_contradicts_edge() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "ClaimA");
        let b = add_node(&mut g, "ClaimB");
        let edge_id = Uuid::new_v4();
        g.add_edge(
            a,
            b,
            EdgeMeta {
                id: edge_id,
                rel_type: "contradicts".into(),
                weight: 1.0,
                confidence: 0.85,
                causal_level: None,
                clearance_level: 0,
            },
        )
        .unwrap();

        let contentions = detect_contentions(g.graph());
        assert_eq!(contentions.len(), 1);
        assert_eq!(contentions[0].node_a, a);
        assert_eq!(contentions[0].node_b, b);
        assert_eq!(contentions[0].edge_id, edge_id);
        assert_eq!(contentions[0].rel_type, "contradicts");
        assert!((contentions[0].confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn detects_contends_edge() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "Theory1");
        let b = add_node(&mut g, "Theory2");
        g.add_edge(
            a,
            b,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "CONTENDS".into(),
                weight: 1.0,
                confidence: 0.7,
                causal_level: None,
                clearance_level: 0,
            },
        )
        .unwrap();

        let contentions = detect_contentions(g.graph());
        assert_eq!(contentions.len(), 1);
        assert_eq!(contentions[0].rel_type, "CONTENDS");
    }

    #[test]
    fn empty_graph_no_contentions() {
        let g = GraphSidecar::new();
        let contentions = detect_contentions(g.graph());
        assert!(contentions.is_empty());
    }
}
