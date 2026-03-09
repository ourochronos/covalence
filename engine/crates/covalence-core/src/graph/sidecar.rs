//! Graph sidecar — in-memory StableDiGraph mirroring PG node/edge data.
//!
//! The sidecar provides fast traversal and algorithm execution.
//! PG is the source of truth; the sidecar syncs via the outbox pattern.
//!
//! Uses `StableDiGraph` instead of `DiGraph` so that `NodeIndex` and
//! `EdgeIndex` values remain valid after removals (no swap-remove
//! invalidation), keeping the `HashMap<Uuid, NodeIndex>` lookup sound.

use std::collections::HashMap;
use std::sync::Arc;

use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableDiGraph};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::types::causal::CausalLevel;

use super::sync::OutboxEvent;

/// Metadata attached to graph nodes (lightweight — full data lives in PG).
#[derive(Debug, Clone)]
pub struct NodeMeta {
    /// Unique identifier for this node.
    pub id: Uuid,
    /// The type of this node (e.g. "entity", "concept").
    pub node_type: String,
    /// Human-readable canonical name.
    pub canonical_name: String,
    /// Clearance level controlling federation visibility.
    pub clearance_level: i32,
}

/// Metadata attached to graph edges in the petgraph sidecar.
#[derive(Debug, Clone)]
pub struct EdgeMeta {
    /// Unique identifier for this edge.
    pub id: Uuid,
    /// Relationship type (e.g. "causes", "related_to").
    pub rel_type: String,
    /// Edge weight for algorithm scoring.
    pub weight: f64,
    /// Confidence in this relationship.
    pub confidence: f64,
    /// Causal level per Pearl's hierarchy, if applicable.
    pub causal_level: Option<CausalLevel>,
    /// Clearance level controlling federation visibility.
    pub clearance_level: i32,
}

/// The in-memory graph sidecar.
///
/// Mirrors PG node/edge data into a `StableDiGraph` for fast traversal and
/// algorithm execution. Uses `StableDiGraph` so that `NodeIndex` and
/// `EdgeIndex` values remain stable across removals — no swap-remove
/// invalidation. The `index` map provides O(1) lookup from UUID to
/// petgraph's internal `NodeIndex`. The `edge_index` map provides O(1)
/// lookup from edge UUID to petgraph's internal `EdgeIndex`.
#[derive(Debug)]
pub struct GraphSidecar {
    /// The directed graph (stable — indices survive removal).
    pub(crate) graph: StableDiGraph<NodeMeta, EdgeMeta>,
    /// UUID to NodeIndex lookup.
    pub(crate) index: HashMap<Uuid, NodeIndex>,
    /// UUID to EdgeIndex lookup.
    pub(crate) edge_index: HashMap<Uuid, EdgeIndex>,
}

/// Thread-safe shared reference to the graph sidecar.
pub type SharedGraph = Arc<RwLock<GraphSidecar>>;

impl GraphSidecar {
    /// Create an empty graph sidecar.
    pub fn new() -> Self {
        Self {
            graph: StableDiGraph::new(),
            index: HashMap::new(),
            edge_index: HashMap::new(),
        }
    }

    /// Add a node to the graph. Returns an error if the node UUID already exists.
    pub fn add_node(&mut self, meta: NodeMeta) -> Result<NodeIndex> {
        if self.index.contains_key(&meta.id) {
            return Err(Error::Graph(format!("node already exists: {}", meta.id)));
        }
        let id = meta.id;
        let idx = self.graph.add_node(meta);
        self.index.insert(id, idx);
        Ok(idx)
    }

    /// Add an edge between two nodes identified by UUID.
    pub fn add_edge(&mut self, source: Uuid, target: Uuid, meta: EdgeMeta) -> Result<EdgeIndex> {
        let src_idx = self
            .index
            .get(&source)
            .copied()
            .ok_or_else(|| Error::NotFound {
                entity_type: "node",
                id: source.to_string(),
            })?;
        let tgt_idx = self
            .index
            .get(&target)
            .copied()
            .ok_or_else(|| Error::NotFound {
                entity_type: "node",
                id: target.to_string(),
            })?;
        let edge_id = meta.id;
        let eidx = self.graph.add_edge(src_idx, tgt_idx, meta);
        self.edge_index.insert(edge_id, eidx);
        Ok(eidx)
    }

    /// Remove a node by UUID. Also removes all connected edges and
    /// cleans up the `edge_index` for those edges.
    ///
    /// `StableDiGraph` preserves index stability on removal — no
    /// swap-remove fixups needed for other nodes or edges.
    pub fn remove_node(&mut self, id: Uuid) -> Result<NodeMeta> {
        let idx = self.index.remove(&id).ok_or(Error::NotFound {
            entity_type: "node",
            id: id.to_string(),
        })?;

        // Collect edge UUIDs for all edges connected to this node
        // (both outgoing and incoming) so we can remove them from
        // `edge_index` after petgraph drops them.
        let outgoing: Vec<Uuid> = self.graph.edges(idx).map(|e| e.weight().id).collect();
        let incoming: Vec<Uuid> = self
            .graph
            .edges_directed(idx, petgraph::Direction::Incoming)
            .map(|e| e.weight().id)
            .collect();
        for eid in outgoing.iter().chain(incoming.iter()) {
            self.edge_index.remove(eid);
        }

        let meta = self.graph.remove_node(idx).ok_or(Error::Graph(format!(
            "node index invalid after removal: {id}"
        )))?;

        Ok(meta)
    }

    /// Remove an edge by its UUID using O(1) index lookup.
    ///
    /// `StableDiGraph` preserves index stability on removal — no
    /// swap-remove fixups needed for remaining edges.
    pub fn remove_edge(&mut self, edge_id: Uuid) -> Result<EdgeMeta> {
        let eidx = self.edge_index.remove(&edge_id).ok_or(Error::NotFound {
            entity_type: "edge",
            id: edge_id.to_string(),
        })?;
        let meta = self
            .graph
            .remove_edge(eidx)
            .ok_or(Error::Graph(format!("edge index invalid: {edge_id}")))?;

        Ok(meta)
    }

    /// Get a reference to a node's metadata by UUID.
    pub fn get_node(&self, id: Uuid) -> Option<&NodeMeta> {
        self.index.get(&id).map(|&idx| &self.graph[idx])
    }

    /// Get a reference to an edge's metadata by UUID using O(1) lookup.
    pub fn get_edge(&self, edge_id: Uuid) -> Option<&EdgeMeta> {
        self.edge_index.get(&edge_id).map(|&eidx| &self.graph[eidx])
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Look up a `NodeIndex` by UUID.
    pub fn node_index(&self, id: Uuid) -> Option<NodeIndex> {
        self.index.get(&id).copied()
    }

    /// Get the inner StableDiGraph reference.
    pub fn graph(&self) -> &StableDiGraph<NodeMeta, EdgeMeta> {
        &self.graph
    }

    /// Apply an outbox event to update the graph.
    pub fn apply_event(&mut self, event: &OutboxEvent) {
        match (event.entity_type.as_str(), event.operation.as_str()) {
            ("node", "INSERT") | ("node", "UPDATE") => {
                if let Some(ref payload) = event.payload {
                    let _ = self.apply_node_upsert(event.entity_id, payload);
                }
            }
            ("node", "DELETE") => {
                let _ = self.remove_node(event.entity_id);
            }
            ("edge", "INSERT") | ("edge", "UPDATE") => {
                if let Some(ref payload) = event.payload {
                    let _ = self.apply_edge_upsert(event.entity_id, payload);
                }
            }
            ("edge", "DELETE") => {
                let _ = self.remove_edge(event.entity_id);
            }
            _ => {
                tracing::warn!(
                    entity_type = %event.entity_type,
                    operation = %event.operation,
                    "unknown outbox event type/operation"
                );
            }
        }
    }

    /// Insert or update a node from a JSON payload.
    fn apply_node_upsert(&mut self, entity_id: Uuid, payload: &serde_json::Value) -> Result<()> {
        let node_type = payload["node_type"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let canonical_name = payload["canonical_name"].as_str().unwrap_or("").to_string();
        let clearance_level = payload["clearance_level"].as_i64().unwrap_or(0) as i32;

        if let Some(&idx) = self.index.get(&entity_id) {
            let node = &mut self.graph[idx];
            node.node_type = node_type;
            node.canonical_name = canonical_name;
            node.clearance_level = clearance_level;
        } else {
            self.add_node(NodeMeta {
                id: entity_id,
                node_type,
                canonical_name,
                clearance_level,
            })?;
        }
        Ok(())
    }

    /// Insert or update an edge from a JSON payload.
    fn apply_edge_upsert(&mut self, entity_id: Uuid, payload: &serde_json::Value) -> Result<()> {
        let source_str = payload["source_node_id"]
            .as_str()
            .ok_or_else(|| Error::Graph("missing source_node_id".into()))?;
        let target_str = payload["target_node_id"]
            .as_str()
            .ok_or_else(|| Error::Graph("missing target_node_id".into()))?;

        let source: Uuid = source_str
            .parse()
            .map_err(|e| Error::Graph(format!("invalid source UUID: {e}")))?;
        let target: Uuid = target_str
            .parse()
            .map_err(|e| Error::Graph(format!("invalid target UUID: {e}")))?;

        let rel_type = payload["rel_type"]
            .as_str()
            .unwrap_or("related_to")
            .to_string();
        let weight = payload["weight"].as_f64().unwrap_or(1.0);
        let confidence = payload["confidence"].as_f64().unwrap_or(1.0);
        let clearance_level = payload["clearance_level"].as_i64().unwrap_or(0) as i32;
        let causal_level = payload["causal_level"]
            .as_str()
            .and_then(CausalLevel::from_str_opt);

        // Remove existing edge if updating
        let _ = self.remove_edge(entity_id);

        self.add_edge(
            source,
            target,
            EdgeMeta {
                id: entity_id,
                rel_type,
                weight,
                confidence,
                causal_level,
                clearance_level,
            },
        )?;
        Ok(())
    }
}

impl Default for GraphSidecar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(name: &str) -> NodeMeta {
        NodeMeta {
            id: Uuid::new_v4(),
            node_type: "entity".into(),
            canonical_name: name.into(),
            clearance_level: 0,
        }
    }

    fn make_edge(rel: &str) -> EdgeMeta {
        EdgeMeta {
            id: Uuid::new_v4(),
            rel_type: rel.into(),
            weight: 1.0,
            confidence: 0.9,
            causal_level: None,
            clearance_level: 0,
        }
    }

    #[test]
    fn add_and_get_node() {
        let mut g = GraphSidecar::new();
        let node = make_node("Alice");
        let id = node.id;
        g.add_node(node).unwrap();
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.get_node(id).unwrap().canonical_name, "Alice");
    }

    #[test]
    fn duplicate_node_is_error() {
        let mut g = GraphSidecar::new();
        let node = make_node("Alice");
        let id = node.id;
        g.add_node(node).unwrap();
        let dup = make_node("Bob");
        let mut dup = dup;
        dup.id = id;
        assert!(g.add_node(dup).is_err());
    }

    #[test]
    fn add_and_remove_edge() {
        let mut g = GraphSidecar::new();
        let a = make_node("A");
        let b = make_node("B");
        let a_id = a.id;
        let b_id = b.id;
        g.add_node(a).unwrap();
        g.add_node(b).unwrap();

        let edge = make_edge("causes");
        let edge_id = edge.id;
        g.add_edge(a_id, b_id, edge).unwrap();
        assert_eq!(g.edge_count(), 1);

        let removed = g.remove_edge(edge_id).unwrap();
        assert_eq!(removed.rel_type, "causes");
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn remove_node_removes_edges() {
        let mut g = GraphSidecar::new();
        let a = make_node("A");
        let b = make_node("B");
        let a_id = a.id;
        let b_id = b.id;
        g.add_node(a).unwrap();
        g.add_node(b).unwrap();
        g.add_edge(a_id, b_id, make_edge("related")).unwrap();
        assert_eq!(g.edge_count(), 1);

        g.remove_node(a_id).unwrap();
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn edge_index_lookup() {
        let mut g = GraphSidecar::new();
        let a = make_node("A");
        let b = make_node("B");
        let c = make_node("C");
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        g.add_node(a).ok();
        g.add_node(b).ok();
        g.add_node(c).ok();

        let e1 = make_edge("causes");
        let e1_id = e1.id;
        let e2 = make_edge("related_to");
        let e2_id = e2.id;
        let e3 = make_edge("depends_on");
        let e3_id = e3.id;

        g.add_edge(a_id, b_id, e1).ok();
        g.add_edge(b_id, c_id, e2).ok();
        g.add_edge(a_id, c_id, e3).ok();
        assert_eq!(g.edge_count(), 3);

        // O(1) lookup via edge_index
        let found = g.get_edge(e1_id);
        assert!(found.is_some());
        assert_eq!(found.map(|e| e.rel_type.as_str()), Some("causes"));

        let found2 = g.get_edge(e2_id);
        assert_eq!(found2.map(|e| e.rel_type.as_str()), Some("related_to"),);

        // Remove an edge and verify index is consistent
        g.remove_edge(e1_id).ok();
        assert!(g.get_edge(e1_id).is_none());
        assert_eq!(g.edge_count(), 2);

        // Remaining edges are still accessible
        assert!(g.get_edge(e2_id).is_some());
        assert!(g.get_edge(e3_id).is_some());

        // Remove a node and verify connected edges are cleaned
        g.remove_node(a_id).ok();
        assert!(g.get_edge(e3_id).is_none()); // a->c removed
        assert!(g.get_edge(e2_id).is_some()); // b->c still there
        assert_eq!(g.edge_count(), 1);

        // Non-existent edge returns None
        assert!(g.get_edge(Uuid::new_v4()).is_none());
    }

    #[test]
    fn apply_event_insert_node() {
        let mut g = GraphSidecar::new();
        let node_id = Uuid::new_v4();
        let event = OutboxEvent {
            seq_id: 1,
            entity_type: "node".into(),
            entity_id: node_id,
            operation: "INSERT".into(),
            payload: Some(serde_json::json!({
                "node_type": "concept",
                "canonical_name": "Rust",
                "clearance_level": 1
            })),
        };
        g.apply_event(&event);
        assert_eq!(g.node_count(), 1);
        let node = g.get_node(node_id).unwrap();
        assert_eq!(node.canonical_name, "Rust");
        assert_eq!(node.clearance_level, 1);
    }

    #[test]
    fn apply_event_delete_node() {
        let mut g = GraphSidecar::new();
        let node_id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id: node_id,
            node_type: "entity".into(),
            canonical_name: "ToDelete".into(),
            clearance_level: 0,
        })
        .unwrap();
        assert_eq!(g.node_count(), 1);

        let event = OutboxEvent {
            seq_id: 2,
            entity_type: "node".into(),
            entity_id: node_id,
            operation: "DELETE".into(),
            payload: None,
        };
        g.apply_event(&event);
        assert_eq!(g.node_count(), 0);
    }
}
