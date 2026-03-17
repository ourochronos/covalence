//! Filtered graph views for clearance-based access control.
//!
//! Uses petgraph's `NodeFiltered` for zero-copy views that mask nodes
//! and edges below a given clearance level.

use std::collections::HashSet;

use petgraph::stable_graph::{EdgeIndex, StableDiGraph};
use petgraph::visit::{EdgeFiltered, EdgeRef, NodeFiltered};

use super::sidecar::{EdgeMeta, NodeMeta};

/// Create a filtered view showing only nodes at or above the given clearance level.
///
/// Returns a zero-copy `NodeFiltered` view. Nodes below `min_clearance`
/// and all their edges are hidden from traversal.
pub fn filtered_view(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    min_clearance: i32,
) -> NodeFiltered<
    &StableDiGraph<NodeMeta, EdgeMeta>,
    impl Fn(petgraph::stable_graph::NodeIndex) -> bool + '_,
> {
    NodeFiltered::from_fn(graph, move |idx| {
        graph[idx].clearance_level >= min_clearance
    })
}

/// Wrapper around a `HashSet<EdgeIndex>` that implements `Fn(EdgeReference) -> bool`
/// for any lifetime, avoiding higher-ranked lifetime issues.
#[derive(Clone)]
pub struct EdgeIndexFilter {
    allowed: HashSet<EdgeIndex>,
}

impl<E> petgraph::visit::FilterEdge<petgraph::stable_graph::EdgeReference<'_, E>>
    for EdgeIndexFilter
{
    fn include_edge(&self, edge: petgraph::stable_graph::EdgeReference<'_, E>) -> bool {
        self.allowed.contains(&edge.id())
    }
}

/// Create an edge-filtered view showing only edges of the specified types.
///
/// Pre-computes the set of matching edge indices. Edges not in `edge_types`
/// are hidden from traversal.
pub fn filtered_edge_view<'a>(
    graph: &'a StableDiGraph<NodeMeta, EdgeMeta>,
    edge_types: &[String],
) -> EdgeFiltered<&'a StableDiGraph<NodeMeta, EdgeMeta>, EdgeIndexFilter> {
    let allowed: HashSet<EdgeIndex> = graph
        .edge_indices()
        .filter(|&eidx| edge_types.iter().any(|t| t == &graph[eidx].rel_type))
        .collect();

    EdgeFiltered(graph, EdgeIndexFilter { allowed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};
    use petgraph::visit::IntoNodeReferences;
    use uuid::Uuid;

    fn add_node(g: &mut GraphSidecar, name: &str, clearance: i32) -> Uuid {
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: name.into(),
            clearance_level: clearance,
        })
        .unwrap();
        id
    }

    fn add_edge(g: &mut GraphSidecar, src: Uuid, tgt: Uuid, rel: &str) {
        g.add_edge(
            src,
            tgt,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: rel.into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn filtered_view_hides_low_clearance() {
        let mut g = GraphSidecar::new();
        let _public = add_node(&mut g, "Public", 2);
        let _trusted = add_node(&mut g, "Trusted", 1);
        let _local = add_node(&mut g, "Local", 0);

        let view = filtered_view(g.graph(), 1);
        let visible: Vec<_> = view.node_references().collect();
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn filtered_view_clearance_zero_shows_all() {
        let mut g = GraphSidecar::new();
        add_node(&mut g, "A", 0);
        add_node(&mut g, "B", 1);
        add_node(&mut g, "C", 2);

        let view = filtered_view(g.graph(), 0);
        let visible: Vec<_> = view.node_references().collect();
        assert_eq!(visible.len(), 3);
    }

    #[test]
    fn edge_filtered_view_limits_types() {
        use petgraph::visit::IntoEdgeReferences;

        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A", 0);
        let b = add_node(&mut g, "B", 0);
        let c = add_node(&mut g, "C", 0);
        add_edge(&mut g, a, b, "causes");
        add_edge(&mut g, b, c, "related_to");

        let types = vec!["causes".to_string()];
        let view = filtered_edge_view(g.graph(), &types);
        let visible_edges: Vec<_> = (&view).edge_references().collect();
        assert_eq!(visible_edges.len(), 1);
    }
}
