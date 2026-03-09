//! Query expansion via graph context.
//!
//! Enriches queries with 1-hop graph relationships for entities
//! mentioned in the query. This improves recall for entity-centric
//! queries by embedding graph knowledge into the query vector.
//!
//! Example: "Tim Cook" -> "Tim Cook (CEO of Apple, announced Vision Pro)"
//! Cost: one graph lookup per entity (~1ms). No LLM call.

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
}
