//! Community summary generation and storage.
//!
//! LLM-generated summaries for each community, stored as nodes
//! with `node_type = "community_summary"` and linked to community
//! members via `SUMMARIZES` edges.
//!
//! The [`generate_community_summaries`] function detects communities
//! from the graph sidecar and produces a [`CommunitySummaryNode`]
//! for each community, ready for insertion as a
//! `node_type = "community_summary"` node.

use std::collections::HashSet;

use petgraph::stable_graph::StableDiGraph;
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::community::{Community, detect_communities};
use crate::graph::sidecar::{EdgeMeta, NodeMeta};

/// Request to generate a community summary.
#[derive(Debug, Clone)]
pub struct CommunitySummaryInput {
    /// Community ID.
    pub community_id: usize,
    /// Core level of the community.
    pub core_level: usize,
    /// Entity names in this community.
    pub entity_names: Vec<String>,
    /// Key relationship descriptions.
    pub relationships: Vec<String>,
    /// Representative chunk content from community members.
    pub representative_chunks: Vec<String>,
}

/// Generated community summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitySummary {
    /// Community ID.
    pub community_id: usize,
    /// Generated title.
    pub title: String,
    /// Generated summary text.
    pub summary: String,
    /// Key findings/themes identified.
    pub key_themes: Vec<String>,
}

/// Trait for generating community summaries.
#[async_trait::async_trait]
pub trait SummaryGenerator: Send + Sync {
    /// Generate a summary for a community.
    async fn generate(
        &self,
        input: &CommunitySummaryInput,
    ) -> crate::error::Result<CommunitySummary>;
}

/// Simple concatenation-based summary generator (no LLM).
///
/// Produces summaries by joining entity names and relationship
/// descriptions. Useful for testing and as a baseline.
pub struct ConcatSummaryGenerator;

#[async_trait::async_trait]
impl SummaryGenerator for ConcatSummaryGenerator {
    async fn generate(
        &self,
        input: &CommunitySummaryInput,
    ) -> crate::error::Result<CommunitySummary> {
        let title = format!(
            "Community {} (core level {})",
            input.community_id, input.core_level
        );

        let mut summary = String::new();
        if !input.entity_names.is_empty() {
            summary.push_str("Key entities: ");
            summary.push_str(&input.entity_names.join(", "));
            summary.push_str(". ");
        }
        if !input.relationships.is_empty() {
            summary.push_str("Relationships: ");
            summary.push_str(&input.relationships.join("; "));
            summary.push_str(". ");
        }

        let key_themes = input.entity_names.clone();

        Ok(CommunitySummary {
            community_id: input.community_id,
            title,
            summary,
            key_themes,
        })
    }
}

/// A community summary node ready for insertion into the graph.
///
/// Represents a synthesized summary of a detected community,
/// stored as a node with `node_type = "community_summary"` to
/// enable the global search dimension to find content.
#[derive(Debug, Clone)]
pub struct CommunitySummaryNode {
    /// Unique identifier for this summary node.
    pub id: Uuid,
    /// The node type — always `"community_summary"`.
    pub node_type: String,
    /// Generated canonical name (title).
    pub canonical_name: String,
    /// Summary description combining entity names and relationships.
    pub description: String,
    /// UUIDs of member nodes in this community.
    pub member_node_ids: Vec<Uuid>,
    /// The k-core level of the community.
    pub core_level: usize,
    /// Key entity names for search indexing.
    pub key_entities: Vec<String>,
}

/// Generate community summary nodes from the graph sidecar.
///
/// Runs community detection (k-core decomposition) on the graph,
/// then for each detected community:
/// 1. Collects member node names and descriptions
/// 2. Collects relationship types between members
/// 3. Produces a text summary by concatenation
/// 4. Returns a [`CommunitySummaryNode`] ready for storage
///
/// This function does not require an LLM — it uses simple
/// concatenation to build summaries. For LLM-enhanced summaries,
/// use the [`SummaryGenerator`] trait.
///
/// # Arguments
///
/// * `graph` - The petgraph sidecar graph
///
/// # Returns
///
/// A vector of [`CommunitySummaryNode`] values, one per detected
/// community with at least 2 members.
pub fn generate_community_summaries(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
) -> Vec<CommunitySummaryNode> {
    let communities = detect_communities(graph);

    communities
        .iter()
        .filter(|c| c.node_ids.len() >= 2)
        .map(|community| build_summary_node(graph, community))
        .collect()
}

/// Build a single community summary node from detected community
/// data.
fn build_summary_node(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    community: &Community,
) -> CommunitySummaryNode {
    let member_set: HashSet<Uuid> = community.node_ids.iter().copied().collect();

    // Collect entity names and descriptions from member nodes
    let mut entity_names: Vec<String> = Vec::new();
    let descriptions: Vec<String> = Vec::new();

    for idx in graph.node_indices() {
        let meta = &graph[idx];
        if member_set.contains(&meta.id) {
            entity_names.push(meta.canonical_name.clone());
            // We don't have descriptions in NodeMeta, so we just
            // use canonical names. In a full implementation, we
            // would look up the node model from PG.
        }
    }

    // Collect relationship descriptions from internal edges
    let mut relationships: Vec<String> = Vec::new();
    let mut seen_edges: HashSet<Uuid> = HashSet::new();

    for idx in graph.node_indices() {
        let src_meta = &graph[idx];
        if !member_set.contains(&src_meta.id) {
            continue;
        }
        for edge_ref in graph.edges(idx) {
            let tgt_meta = &graph[edge_ref.target()];
            let edge_meta = edge_ref.weight();
            if member_set.contains(&tgt_meta.id) && seen_edges.insert(edge_meta.id) {
                relationships.push(format!(
                    "{} --[{}]--> {}",
                    src_meta.canonical_name, edge_meta.rel_type, tgt_meta.canonical_name,
                ));
            }
        }
    }

    // Build summary text
    let mut summary = String::new();
    if !entity_names.is_empty() {
        summary.push_str("Key entities: ");
        summary.push_str(&entity_names.join(", "));
        summary.push_str(". ");
    }
    if !relationships.is_empty() {
        summary.push_str("Relationships: ");
        summary.push_str(&relationships.join("; "));
        summary.push('.');
    }
    if !descriptions.is_empty() {
        summary.push(' ');
        summary.push_str(&descriptions.join(" "));
    }

    let title = format!(
        "Community {} (core level {})",
        community.id, community.core_level
    );

    CommunitySummaryNode {
        id: Uuid::new_v4(),
        node_type: "community_summary".to_string(),
        canonical_name: title,
        description: summary,
        member_node_ids: community.node_ids.clone(),
        core_level: community.core_level,
        key_entities: entity_names,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concat_summary_basic() {
        let generator = ConcatSummaryGenerator;
        let input = CommunitySummaryInput {
            community_id: 42,
            core_level: 2,
            entity_names: vec!["Alice".to_string(), "Bob".to_string()],
            relationships: vec!["Alice works with Bob".to_string()],
            representative_chunks: vec![],
        };

        let result = generator.generate(&input).await.unwrap();
        assert_eq!(result.community_id, 42);
        assert_eq!(result.title, "Community 42 (core level 2)");
        assert!(result.summary.contains("Alice"));
        assert!(result.summary.contains("Bob"));
        assert!(result.summary.contains("Alice works with Bob"));
        assert_eq!(result.key_themes.len(), 2);
        assert_eq!(result.key_themes[0], "Alice");
        assert_eq!(result.key_themes[1], "Bob");
    }

    #[tokio::test]
    async fn concat_summary_empty_entities() {
        let generator = ConcatSummaryGenerator;
        let input = CommunitySummaryInput {
            community_id: 1,
            core_level: 0,
            entity_names: vec![],
            relationships: vec![],
            representative_chunks: vec![],
        };

        let result = generator.generate(&input).await.unwrap();
        assert_eq!(result.community_id, 1);
        assert!(result.summary.is_empty());
        assert!(result.key_themes.is_empty());
    }

    #[tokio::test]
    async fn concat_summary_entities_only() {
        let generator = ConcatSummaryGenerator;
        let input = CommunitySummaryInput {
            community_id: 5,
            core_level: 1,
            entity_names: vec!["Rust".to_string()],
            relationships: vec![],
            representative_chunks: vec![],
        };

        let result = generator.generate(&input).await.unwrap();
        assert!(result.summary.starts_with("Key entities:"));
        assert!(!result.summary.contains("Relationships:"));
    }

    // --- Community summary generation tests ---

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

    fn add_edge_to(g: &mut GraphSidecar, src: Uuid, tgt: Uuid, rel_type: &str) {
        g.add_edge(
            src,
            tgt,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: rel_type.into(),
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
    fn generate_summaries_from_graph() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "Alice");
        let b = add_node(&mut g, "Bob");
        add_edge_to(&mut g, a, b, "works_with");
        add_edge_to(&mut g, b, a, "works_with");

        let summaries = generate_community_summaries(g.graph());

        // The pair should form at least one community
        assert!(
            !summaries.is_empty(),
            "Expected at least one community summary"
        );

        let summary = &summaries[0];
        assert_eq!(summary.node_type, "community_summary");
        assert!(summary.description.contains("Alice"));
        assert!(summary.description.contains("Bob"));
        assert!(summary.description.contains("works_with"));
        assert_eq!(summary.member_node_ids.len(), 2);
    }

    #[test]
    fn generate_summaries_empty_graph() {
        let g = GraphSidecar::new();
        let summaries = generate_community_summaries(g.graph());
        assert!(summaries.is_empty());
    }

    #[test]
    fn generate_summaries_singleton_excluded() {
        let mut g = GraphSidecar::new();
        let _a = add_node(&mut g, "Lonely");
        // Single node, no edges — should not produce summary
        let summaries = generate_community_summaries(g.graph());
        assert!(summaries.is_empty());
    }

    #[test]
    fn generate_summaries_multiple_communities() {
        let mut g = GraphSidecar::new();

        // Community 1
        let a = add_node(&mut g, "Alpha");
        let b = add_node(&mut g, "Beta");
        add_edge_to(&mut g, a, b, "related");
        add_edge_to(&mut g, b, a, "related");

        // Community 2 (disconnected)
        let x = add_node(&mut g, "Xray");
        let y = add_node(&mut g, "Yankee");
        add_edge_to(&mut g, x, y, "linked");
        add_edge_to(&mut g, y, x, "linked");

        let summaries = generate_community_summaries(g.graph());

        assert!(
            summaries.len() >= 2,
            "Expected at least 2 community summaries, \
             got {}",
            summaries.len()
        );

        // Each summary should have exactly 2 members
        for s in &summaries {
            assert_eq!(s.member_node_ids.len(), 2);
        }
    }
}
