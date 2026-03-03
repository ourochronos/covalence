//! In-memory graph layer backed by petgraph.
//!
//! Phase 2a: CovalenceGraph struct definition only.
//! Phase 2b: load/reload logic, SharedGraph type alias, wired into AppState.

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::Edge;

/// A cheaply-cloneable, async-safe shared reference to the in-memory graph.
pub type SharedGraph = std::sync::Arc<tokio::sync::RwLock<CovalenceGraph>>;

/// An in-memory directed graph of Covalence nodes and edges.
///
/// Nodes are identified by their Covalence `Uuid`. Edges carry the
/// edge type string (e.g. "ORIGINATES", "CONFIRMS", "SUPERSEDES").
pub struct CovalenceGraph {
    /// The underlying petgraph directed graph.
    pub graph: DiGraph<Uuid, String>,
    /// Maps Covalence node UUIDs → petgraph NodeIndex for O(1) lookup.
    pub index: HashMap<Uuid, NodeIndex>,
}

impl CovalenceGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            index: HashMap::new(),
        }
    }

    /// Add a node (idempotent — no-op if already present).
    pub fn add_node(&mut self, id: Uuid) -> NodeIndex {
        if let Some(&idx) = self.index.get(&id) {
            return idx;
        }
        let idx = self.graph.add_node(id);
        self.index.insert(id, idx);
        idx
    }

    /// Add a directed edge between two node UUIDs.
    /// Nodes are created automatically if they don't exist.
    pub fn add_edge(&mut self, source: Uuid, target: Uuid, edge_type: String) {
        let s = self.add_node(source);
        let t = self.add_node(target);
        self.graph.add_edge(s, t, edge_type);
    }

    /// Build a graph from a slice of Edge records (all edges from DB).
    pub fn load(edges: &[Edge]) -> Self {
        let mut g = Self::new();
        for edge in edges {
            g.add_node(edge.source_node_id);
            g.add_node(edge.target_node_id);
            g.add_edge(
                edge.source_node_id,
                edge.target_node_id,
                edge.edge_type.as_label().to_string(),
            );
        }
        g
    }

    /// Return true if the graph contains a node with the given UUID.
    pub fn has_node(&self, id: &Uuid) -> bool {
        self.index.contains_key(id)
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

impl Default for CovalenceGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_graph() {
        let g = CovalenceGraph::new();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn test_add_nodes_and_edges() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn test_add_node_idempotent() {
        let mut g = CovalenceGraph::new();
        let id = Uuid::new_v4();
        let idx1 = g.add_node(id);
        let idx2 = g.add_node(id);
        assert_eq!(idx1, idx2);
        assert_eq!(g.node_count(), 1);
    }
}
