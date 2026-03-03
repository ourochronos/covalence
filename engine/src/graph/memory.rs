//! In-memory graph layer backed by petgraph.
//!
//! Phase 2a: CovalenceGraph struct definition only.
//! Phase 2b: load/reload logic, SharedGraph type alias, wired into AppState.
//! Phase 7: intent-aware edge filtering via `intent_edge_types()` and
//!           upgraded `neighbors_filtered()` (covalence#54).

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::{Edge, EdgeType, SearchIntent};

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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

    /// Return the UUIDs of all direct outgoing neighbors of `id` (all edge types).
    pub fn neighbors(&self, id: &Uuid) -> Vec<Uuid> {
        let Some(&idx) = self.index.get(id) else {
            return vec![];
        };
        use petgraph::visit::EdgeRef;
        self.graph
            .edges(idx)
            .filter_map(|e| self.graph.node_weight(e.target()).copied())
            .collect()
    }

    /// Return `(neighbor_uuid, edge_type_label)` pairs for all outgoing edges
    /// of `id`, optionally restricted to the given edge type labels.
    ///
    /// * `edge_types = None`  → all outgoing edges (equivalent to `neighbors()`).
    /// * `edge_types = Some(types)` → only edges whose type label appears in `types`.
    ///
    /// This is the Phase 7 intent-aware variant (covalence#54).
    #[allow(dead_code)]
    pub fn neighbors_filtered(
        &self,
        id: &Uuid,
        edge_types: Option<&[String]>,
    ) -> Vec<(Uuid, String)> {
        let Some(&idx) = self.index.get(id) else {
            return vec![];
        };
        use petgraph::visit::EdgeRef;
        self.graph
            .edges(idx)
            .filter(|e| match edge_types {
                None => true,
                Some(types) => types.contains(e.weight()),
            })
            .filter_map(|e| {
                self.graph
                    .node_weight(e.target())
                    .copied()
                    .map(|neighbor_id| (neighbor_id, e.weight().clone()))
            })
            .collect()
    }
}

// ── Intent → edge-type mapping (Phase 7, covalence#54) ──────────────────────

/// Map a [`SearchIntent`] to the set of edge-type labels it should traverse.
///
/// Inspired by MAGMA's four orthogonal graph views:
///
/// | Intent   | Edge types                              |
/// |----------|-----------------------------------------|
/// | Factual  | CONFIRMS, ORIGINATES                    |
/// | Temporal | PRECEDES, FOLLOWS                       |
/// | Causal   | CAUSES, MOTIVATED_BY, IMPLEMENTS        |
/// | Entity   | INVOLVES                                |
///
/// To get "all edges" (no filter), pass `None` for the intent at call sites.
pub fn intent_edge_types(intent: &SearchIntent) -> Vec<String> {
    match intent {
        SearchIntent::Factual => vec![
            EdgeType::Confirms.as_label().to_string(),
            EdgeType::Originates.as_label().to_string(),
        ],
        SearchIntent::Temporal => vec![
            EdgeType::Precedes.as_label().to_string(),
            EdgeType::Follows.as_label().to_string(),
        ],
        SearchIntent::Causal => vec![
            EdgeType::Causes.as_label().to_string(),
            EdgeType::MotivatedBy.as_label().to_string(),
            EdgeType::Implements.as_label().to_string(),
        ],
        SearchIntent::Entity => vec![EdgeType::Involves.as_label().to_string()],
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
    use crate::models::SearchIntent;

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

    // ── neighbors_filtered tests (Phase 7) ───────────────────────────────────

    #[test]
    fn test_neighbors_filtered_single_type() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_edge(a, c, "CONFIRMS".to_string());

        let filter = vec!["ORIGINATES".to_string()];
        let neighbors: Vec<Uuid> = g
            .neighbors_filtered(&a, Some(&filter))
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(neighbors.len(), 1);
        assert!(neighbors.contains(&b));
        assert!(!neighbors.contains(&c));
    }

    #[test]
    fn test_neighbors_filtered_multi_type() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_edge(a, c, "CONFIRMS".to_string());
        g.add_edge(a, d, "PRECEDES".to_string());

        let filter = vec!["ORIGINATES".to_string(), "CONFIRMS".to_string()];
        let neighbors: Vec<Uuid> = g
            .neighbors_filtered(&a, Some(&filter))
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(neighbors.len(), 2);
        assert!(neighbors.contains(&b));
        assert!(neighbors.contains(&c));
        assert!(!neighbors.contains(&d));
    }

    #[test]
    fn test_neighbors_filtered_none_returns_all() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_edge(a, c, "CONFIRMS".to_string());

        let neighbors = g.neighbors_filtered(&a, None);
        assert_eq!(neighbors.len(), 2);
    }

    #[test]
    fn test_neighbors_filtered_returns_edge_types() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_edge(a, b, "CAUSES".to_string());

        let results = g.neighbors_filtered(&a, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, b);
        assert_eq!(results[0].1, "CAUSES");
    }

    #[test]
    fn test_neighbors_filtered_unknown_node() {
        let g = CovalenceGraph::new();
        let unknown = Uuid::new_v4();
        assert!(g.neighbors_filtered(&unknown, None).is_empty());
    }

    // ── Intent → edge-type mapping tests ─────────────────────────────────────

    #[test]
    fn test_factual_intent_traverses_confirms_originates() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4(); // CONFIRMS
        let c = Uuid::new_v4(); // ORIGINATES
        let d = Uuid::new_v4(); // PRECEDES — should be excluded
        g.add_edge(a, b, "CONFIRMS".to_string());
        g.add_edge(a, c, "ORIGINATES".to_string());
        g.add_edge(a, d, "PRECEDES".to_string());

        let types = intent_edge_types(&SearchIntent::Factual);
        let neighbors: Vec<Uuid> = g
            .neighbors_filtered(&a, Some(&types))
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(
            neighbors.len(),
            2,
            "factual intent must return CONFIRMS + ORIGINATES only"
        );
        assert!(neighbors.contains(&b));
        assert!(neighbors.contains(&c));
        assert!(
            !neighbors.contains(&d),
            "temporal edges must not appear under factual intent"
        );
    }

    #[test]
    fn test_temporal_intent_traverses_precedes_follows() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4(); // PRECEDES
        let c = Uuid::new_v4(); // FOLLOWS
        let d = Uuid::new_v4(); // CONFIRMS — should be excluded
        g.add_edge(a, b, "PRECEDES".to_string());
        g.add_edge(a, c, "FOLLOWS".to_string());
        g.add_edge(a, d, "CONFIRMS".to_string());

        let types = intent_edge_types(&SearchIntent::Temporal);
        let neighbors: Vec<Uuid> = g
            .neighbors_filtered(&a, Some(&types))
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(
            neighbors.len(),
            2,
            "temporal intent must return PRECEDES + FOLLOWS only"
        );
        assert!(neighbors.contains(&b));
        assert!(neighbors.contains(&c));
        assert!(
            !neighbors.contains(&d),
            "factual edges must not appear under temporal intent"
        );
    }

    #[test]
    fn test_causal_intent_edge_types() {
        let types = intent_edge_types(&SearchIntent::Causal);
        assert!(types.contains(&"CAUSES".to_string()));
        assert!(types.contains(&"MOTIVATED_BY".to_string()));
        assert!(types.contains(&"IMPLEMENTS".to_string()));
        assert!(!types.contains(&"CONFIRMS".to_string()));
    }

    #[test]
    fn test_entity_intent_edge_types() {
        let types = intent_edge_types(&SearchIntent::Entity);
        assert!(types.contains(&"INVOLVES".to_string()));
        assert_eq!(types.len(), 1);
    }
}
