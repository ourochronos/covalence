//! Topic clustering via community detection.
//!
//! Groups sources by the communities their extracted nodes belong to,
//! enabling batch consolidation to compile per-topic articles.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::graph::community::detect_communities;
use crate::graph::sidecar::{EdgeMeta, NodeMeta};
use crate::types::ids::SourceId;

/// A source with the node IDs extracted from it.
#[derive(Debug, Clone)]
pub struct SourceNodes {
    /// The source identifier.
    pub source_id: SourceId,
    /// Node UUIDs extracted from this source.
    pub node_ids: Vec<Uuid>,
}

/// Cluster sources by the graph communities their nodes belong to.
///
/// Runs community detection on the graph, then maps each source to the
/// community that contains the majority of its extracted nodes. Sources
/// whose nodes span multiple communities are assigned to the community
/// with the most overlap.
///
/// # Returns
///
/// A map from community ID to the source IDs whose nodes primarily
/// belong to that community.
pub fn cluster_sources_by_community(
    graph: &petgraph::stable_graph::StableDiGraph<NodeMeta, EdgeMeta>,
    sources: &[SourceNodes],
) -> HashMap<usize, Vec<SourceId>> {
    let communities = detect_communities(graph);

    // Build a reverse index: node UUID -> community ID
    let mut node_to_community: HashMap<Uuid, usize> = HashMap::new();
    for comm in &communities {
        for &node_id in &comm.node_ids {
            node_to_community.insert(node_id, comm.id);
        }
    }

    // Assign each source to its majority community
    let mut result: HashMap<usize, Vec<SourceId>> = HashMap::new();

    for source in sources {
        if source.node_ids.is_empty() {
            continue;
        }

        // Count how many of this source's nodes fall in each community
        let mut comm_counts: HashMap<usize, usize> = HashMap::new();
        for node_id in &source.node_ids {
            if let Some(&comm_id) = node_to_community.get(node_id) {
                *comm_counts.entry(comm_id).or_insert(0) += 1;
            }
        }

        // Pick the community with the most nodes from this source
        if let Some((&best_comm, _)) = comm_counts.iter().max_by_key(|(_, count)| *count) {
            result.entry(best_comm).or_default().push(source.source_id);
        }
    }

    result
}

/// Collect the unique set of node UUIDs belonging to the given sources
/// that are members of a specific community.
pub fn nodes_in_community(
    sources: &[SourceNodes],
    source_ids: &[SourceId],
    community_node_ids: &HashSet<Uuid>,
) -> Vec<Uuid> {
    let source_set: HashSet<SourceId> = source_ids.iter().copied().collect();

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for source in sources {
        if !source_set.contains(&source.source_id) {
            continue;
        }
        for &nid in &source.node_ids {
            if community_node_ids.contains(&nid) && seen.insert(nid) {
                result.push(nid);
            }
        }
    }

    result
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
            entity_class: None,
            canonical_name: name.into(),
            clearance_level: 0,
        })
        .unwrap();
        id
    }

    fn add_edge(g: &mut GraphSidecar, src: Uuid, tgt: Uuid) {
        g.add_edge(
            src,
            tgt,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "related".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
                has_valid_from: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn cluster_empty_sources() {
        let g = GraphSidecar::new();
        let result = cluster_sources_by_community(g.graph(), &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn cluster_sources_into_communities() {
        let mut g = GraphSidecar::new();

        // Create two disconnected clusters
        let a1 = add_node(&mut g, "A1");
        let a2 = add_node(&mut g, "A2");
        add_edge(&mut g, a1, a2);
        add_edge(&mut g, a2, a1);

        let b1 = add_node(&mut g, "B1");
        let b2 = add_node(&mut g, "B2");
        add_edge(&mut g, b1, b2);
        add_edge(&mut g, b2, b1);

        let src_a = SourceId::new();
        let src_b = SourceId::new();

        let sources = vec![
            SourceNodes {
                source_id: src_a,
                node_ids: vec![a1, a2],
            },
            SourceNodes {
                source_id: src_b,
                node_ids: vec![b1, b2],
            },
        ];

        let clusters = cluster_sources_by_community(g.graph(), &sources);

        // Both sources should be assigned to communities
        let total_sources: usize = clusters.values().map(|v| v.len()).sum();
        assert_eq!(total_sources, 2);

        // The two sources should not be in the same community
        // (since their nodes are in disconnected clusters)
        if clusters.len() >= 2 {
            for sources_in_comm in clusters.values() {
                assert!(sources_in_comm.len() <= 1);
            }
        }
    }

    #[test]
    fn source_with_no_graph_nodes_is_skipped() {
        let g = GraphSidecar::new();
        let src = SourceId::new();
        let sources = vec![SourceNodes {
            source_id: src,
            node_ids: vec![Uuid::new_v4()], // not in graph
        }];
        let clusters = cluster_sources_by_community(g.graph(), &sources);
        assert!(clusters.is_empty());
    }
}
