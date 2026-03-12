//! Query expansion via graph context.
//!
//! Enriches queries with 1-hop graph relationships for entities
//! mentioned in the query. This improves recall for entity-centric
//! queries by embedding graph knowledge into the query vector.
//!
//! Also provides spreading activation: given a set of seed node
//! IDs (e.g. top-K search results), collect their 1-hop neighbors
//! as additional candidate IDs weighted by edge confidence.
//!
//! Example: "Tim Cook" -> "Tim Cook (CEO of Apple, announced Vision Pro)"
//! Cost: one graph lookup per entity (~1ms). No LLM call.

use std::collections::HashMap;

use petgraph::visit::EdgeRef;
use uuid::Uuid;

use crate::graph::sidecar::SharedGraph;

/// Result of query expansion.
#[derive(Debug, Clone)]
pub struct ExpandedQuery {
    /// The original query text.
    pub original: String,
    /// The expanded query text (with graph context appended).
    pub expanded: String,
    /// Entity names that were matched and expanded.
    pub matched_entities: Vec<String>,
    /// Number of relationships added as context.
    pub relationships_added: usize,
}

/// Result of spreading activation from seed nodes.
#[derive(Debug, Clone)]
pub struct SpreadingResult {
    /// Node IDs discovered via spreading activation.
    pub expanded_ids: Vec<Uuid>,
    /// Activation weights per discovered ID (higher = closer
    /// to seed nodes, weighted by edge confidence).
    pub weights: HashMap<Uuid, f64>,
    /// Number of seed nodes that contributed.
    pub seeds_used: usize,
}

/// Default maximum neighbors to return per seed node.
const MAX_NEIGHBORS_PER_SEED: usize = 5;

/// Perform spreading activation from seed node IDs.
///
/// For each seed, collects 1-hop neighbors in the graph sidecar
/// weighted by edge confidence. Neighbors already in the seed
/// set are excluded. Returns the union of discovered neighbors
/// with their activation weights.
pub async fn spreading_activation(
    seed_ids: &[Uuid],
    graph: &SharedGraph,
    max_per_seed: Option<usize>,
) -> SpreadingResult {
    if seed_ids.is_empty() {
        return SpreadingResult {
            expanded_ids: Vec::new(),
            weights: HashMap::new(),
            seeds_used: 0,
        };
    }

    let max_n = max_per_seed.unwrap_or(MAX_NEIGHBORS_PER_SEED);
    let graph_read = graph.read().await;
    let seed_set: std::collections::HashSet<Uuid> = seed_ids.iter().copied().collect();

    let mut weights: HashMap<Uuid, f64> = HashMap::new();
    let mut seeds_used = 0usize;

    for &seed_id in seed_ids {
        let Some(idx) = graph_read.node_index(seed_id) else {
            continue;
        };
        seeds_used += 1;

        // Collect neighbors sorted by edge confidence descending.
        let mut neighbors: Vec<(Uuid, f64)> = Vec::new();

        for edge in graph_read.graph().edges(idx) {
            let target_meta = &graph_read.graph()[edge.target()];
            let edge_meta = &graph_read.graph()[edge.id()];
            if !seed_set.contains(&target_meta.id) {
                neighbors.push((target_meta.id, edge_meta.confidence));
            }
        }

        for edge in graph_read
            .graph()
            .edges_directed(idx, petgraph::Direction::Incoming)
        {
            let source_meta = &graph_read.graph()[edge.source()];
            let edge_meta = &graph_read.graph()[edge.id()];
            if !seed_set.contains(&source_meta.id) {
                neighbors.push((source_meta.id, edge_meta.confidence));
            }
        }

        // Filter out NaN/infinite confidences, then sort descending.
        neighbors.retain(|(_, conf)| conf.is_finite() && *conf >= 0.0);
        neighbors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        neighbors.truncate(max_n);

        for (nid, conf) in neighbors {
            let entry = weights.entry(nid).or_insert(0.0);
            *entry += conf;
        }
    }

    let expanded_ids: Vec<Uuid> = weights.keys().copied().collect();

    SpreadingResult {
        expanded_ids,
        weights,
        seeds_used,
    }
}

/// Maximum relationships to include per matched entity.
const MAX_RELS_PER_ENTITY: usize = 5;

/// Minimum entity name length to avoid false-positive matches.
const MIN_ENTITY_NAME_LEN: usize = 3;

/// Expand a query by appending 1-hop graph context for mentioned
/// entities.
///
/// Searches the graph sidecar for nodes whose canonical names
/// appear in the query text, then appends their relationship
/// context. Short entity names (fewer than 3 characters) are
/// skipped to avoid false positives.
pub async fn expand_query(query: &str, graph: &SharedGraph) -> ExpandedQuery {
    let graph_read = graph.read().await;
    let query_lower = query.to_lowercase();

    let mut matched_entities = Vec::new();
    let mut context_parts = Vec::new();

    // Find entities mentioned in the query
    for idx in graph_read.graph().node_indices() {
        let meta = &graph_read.graph()[idx];
        let name_lower = meta.canonical_name.to_lowercase();

        // Skip very short names (likely false positives)
        if name_lower.len() < MIN_ENTITY_NAME_LEN {
            continue;
        }

        if query_lower.contains(&name_lower) {
            matched_entities.push(meta.canonical_name.clone());

            // Collect 1-hop relationships
            let mut rels = Vec::new();
            use petgraph::visit::EdgeRef;

            // Outgoing edges
            for edge in graph_read.graph().edges(idx) {
                let target = &graph_read.graph()[edge.target()];
                let edge_meta = &graph_read.graph()[edge.id()];
                rels.push(format!("{} {}", edge_meta.rel_type, target.canonical_name));
            }

            // Incoming edges
            for edge in graph_read
                .graph()
                .edges_directed(idx, petgraph::Direction::Incoming)
            {
                let source = &graph_read.graph()[edge.source()];
                let edge_meta = &graph_read.graph()[edge.id()];
                rels.push(format!(
                    "{} of {}",
                    edge_meta.rel_type, source.canonical_name
                ));
            }

            if !rels.is_empty() {
                rels.truncate(MAX_RELS_PER_ENTITY);
                context_parts.push(format!("{} ({})", meta.canonical_name, rels.join(", ")));
            }
        }
    }

    let relationships_added = context_parts.len();

    let expanded = if context_parts.is_empty() {
        query.to_string()
    } else {
        format!("{} [Context: {}]", query, context_parts.join("; "))
    };

    ExpandedQuery {
        original: query.to_string(),
        expanded,
        matched_entities,
        relationships_added,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::GraphSidecar;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn empty_shared_graph() -> SharedGraph {
        Arc::new(RwLock::new(GraphSidecar::new()))
    }

    #[tokio::test]
    async fn expand_empty_graph() {
        let graph = empty_shared_graph();
        let result = expand_query("Tell me about Rust", &graph).await;

        assert_eq!(result.original, "Tell me about Rust");
        assert_eq!(result.expanded, "Tell me about Rust");
        assert!(result.matched_entities.is_empty());
        assert_eq!(result.relationships_added, 0);
    }

    #[tokio::test]
    async fn expand_no_match() {
        use crate::graph::sidecar::NodeMeta;
        use uuid::Uuid;

        let graph = empty_shared_graph();
        {
            let mut g = graph.write().await;
            g.add_node(NodeMeta {
                id: Uuid::new_v4(),
                node_type: "entity".into(),
                canonical_name: "Python".into(),
                clearance_level: 0,
            })
            .ok();
        }

        let result = expand_query("Tell me about Rust", &graph).await;

        assert_eq!(result.expanded, "Tell me about Rust");
        assert!(result.matched_entities.is_empty());
        assert_eq!(result.relationships_added, 0);
    }

    #[tokio::test]
    async fn expand_with_match_and_relationships() {
        use crate::graph::sidecar::{EdgeMeta, NodeMeta};
        use uuid::Uuid;

        let graph = empty_shared_graph();
        let rust_id = Uuid::new_v4();
        let tokio_id = Uuid::new_v4();

        {
            let mut g = graph.write().await;
            g.add_node(NodeMeta {
                id: rust_id,
                node_type: "entity".into(),
                canonical_name: "Rust".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_node(NodeMeta {
                id: tokio_id,
                node_type: "entity".into(),
                canonical_name: "Tokio".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_edge(
                rust_id,
                tokio_id,
                EdgeMeta {
                    id: Uuid::new_v4(),
                    rel_type: "has_library".into(),
                    weight: 1.0,
                    confidence: 0.9,
                    causal_level: None,
                    clearance_level: 0,
                    is_synthetic: false,
                },
            )
            .ok();
        }

        let result = expand_query("Tell me about Rust", &graph).await;

        assert_eq!(result.original, "Tell me about Rust");
        assert!(result.expanded.contains("[Context:"));
        assert!(result.expanded.contains("has_library Tokio"));
        assert_eq!(result.matched_entities, vec!["Rust"]);
        assert_eq!(result.relationships_added, 1);
    }

    #[tokio::test]
    async fn expand_skips_short_names() {
        use crate::graph::sidecar::NodeMeta;
        use uuid::Uuid;

        let graph = empty_shared_graph();
        {
            let mut g = graph.write().await;
            g.add_node(NodeMeta {
                id: Uuid::new_v4(),
                node_type: "entity".into(),
                canonical_name: "AI".into(),
                clearance_level: 0,
            })
            .ok();
        }

        let result = expand_query("Tell me about AI", &graph).await;

        // "AI" is only 2 chars, should be skipped
        assert!(result.matched_entities.is_empty());
        assert_eq!(result.expanded, "Tell me about AI");
    }

    #[tokio::test]
    async fn spreading_activation_empty_seeds() {
        let graph = empty_shared_graph();
        let result = spreading_activation(&[], &graph, None).await;
        assert!(result.expanded_ids.is_empty());
        assert_eq!(result.seeds_used, 0);
    }

    #[tokio::test]
    async fn spreading_activation_no_match_in_graph() {
        let graph = empty_shared_graph();
        let fake_id = Uuid::new_v4();
        let result = spreading_activation(&[fake_id], &graph, None).await;
        assert!(result.expanded_ids.is_empty());
        assert_eq!(result.seeds_used, 0);
    }

    #[tokio::test]
    async fn spreading_activation_finds_neighbors() {
        use crate::graph::sidecar::{EdgeMeta, NodeMeta};
        use uuid::Uuid;

        let graph = empty_shared_graph();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();
        let c_id = Uuid::new_v4();

        {
            let mut g = graph.write().await;
            g.add_node(NodeMeta {
                id: a_id,
                node_type: "entity".into(),
                canonical_name: "NodeA".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_node(NodeMeta {
                id: b_id,
                node_type: "entity".into(),
                canonical_name: "NodeB".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_node(NodeMeta {
                id: c_id,
                node_type: "entity".into(),
                canonical_name: "NodeC".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_edge(
                a_id,
                b_id,
                EdgeMeta {
                    id: Uuid::new_v4(),
                    rel_type: "related_to".into(),
                    weight: 1.0,
                    confidence: 0.8,
                    causal_level: None,
                    clearance_level: 0,
                    is_synthetic: false,
                },
            )
            .ok();
            g.add_edge(
                a_id,
                c_id,
                EdgeMeta {
                    id: Uuid::new_v4(),
                    rel_type: "related_to".into(),
                    weight: 1.0,
                    confidence: 0.6,
                    causal_level: None,
                    clearance_level: 0,
                    is_synthetic: false,
                },
            )
            .ok();
        }

        // Seed with node A — should find B and C as neighbors.
        let result = spreading_activation(&[a_id], &graph, None).await;
        assert_eq!(result.seeds_used, 1);
        assert_eq!(result.expanded_ids.len(), 2);
        assert!(result.weights.contains_key(&b_id));
        assert!(result.weights.contains_key(&c_id));
        assert!((result.weights[&b_id] - 0.8).abs() < 1e-10);
        assert!((result.weights[&c_id] - 0.6).abs() < 1e-10);
    }

    #[tokio::test]
    async fn spreading_excludes_seed_nodes() {
        use crate::graph::sidecar::{EdgeMeta, NodeMeta};
        use uuid::Uuid;

        let graph = empty_shared_graph();
        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();

        {
            let mut g = graph.write().await;
            g.add_node(NodeMeta {
                id: a_id,
                node_type: "entity".into(),
                canonical_name: "NodeA".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_node(NodeMeta {
                id: b_id,
                node_type: "entity".into(),
                canonical_name: "NodeB".into(),
                clearance_level: 0,
            })
            .ok();
            g.add_edge(
                a_id,
                b_id,
                EdgeMeta {
                    id: Uuid::new_v4(),
                    rel_type: "link".into(),
                    weight: 1.0,
                    confidence: 0.9,
                    causal_level: None,
                    clearance_level: 0,
                    is_synthetic: false,
                },
            )
            .ok();
        }

        // Both A and B are seeds — no new neighbors.
        let result = spreading_activation(&[a_id, b_id], &graph, None).await;
        assert!(result.expanded_ids.is_empty());
        assert_eq!(result.seeds_used, 2);
    }
}
