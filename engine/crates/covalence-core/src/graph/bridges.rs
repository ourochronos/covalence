//! Cross-domain bridge discovery.
//!
//! Identifies nodes that connect different communities (domains), enabling
//! cross-domain generalization discovery during deep consolidation.
//!
//! A bridge node is one that has edges into multiple communities. Higher
//! bridge scores indicate nodes that are critical connectors between
//! otherwise separate knowledge domains.

use std::collections::{HashMap, HashSet};

use petgraph::stable_graph::StableDiGraph;
use petgraph::visit::EdgeRef;
use uuid::Uuid;

use super::community::Community;
use super::sidecar::{EdgeMeta, NodeMeta};

/// A discovered cross-domain bridge.
#[derive(Debug, Clone)]
pub struct Bridge {
    /// The bridge node's UUID.
    pub node_id: Uuid,
    /// Community IDs this node connects.
    pub community_ids: Vec<usize>,
    /// Bridge score: higher means more critical connector.
    pub score: f64,
    /// Number of cross-community edges.
    pub cross_edges: usize,
}

/// Discover cross-domain bridges in the graph.
///
/// A bridge is a node with edges into 2+ communities. The bridge score
/// combines the number of communities connected, the fraction of cross-
/// community edges, and the node's betweenness centrality.
pub fn discover_bridges(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    communities: &[Community],
) -> Vec<Bridge> {
    if communities.len() < 2 {
        return Vec::new();
    }

    // Build node -> community map
    let mut node_community: HashMap<Uuid, usize> = HashMap::new();
    for comm in communities {
        for &nid in &comm.node_ids {
            node_community.insert(nid, comm.id);
        }
    }

    let importance = super::algorithms::structural_importance(graph);

    let mut bridges = Vec::new();

    for idx in graph.node_indices() {
        let node_id = graph[idx].id;
        let home_comm = match node_community.get(&node_id) {
            Some(&c) => c,
            None => continue,
        };

        // Collect communities reached by this node's edges
        let mut reached_communities: HashSet<usize> = HashSet::new();
        let mut total_edges = 0usize;
        let mut cross_edges = 0usize;

        // Outgoing edges
        for edge in graph.edges(idx) {
            total_edges += 1;
            let target_id = graph[edge.target()].id;
            if let Some(&target_comm) = node_community.get(&target_id) {
                if target_comm != home_comm {
                    reached_communities.insert(target_comm);
                    cross_edges += 1;
                }
            }
        }

        // Incoming edges
        for edge in graph.edges_directed(idx, petgraph::Direction::Incoming) {
            total_edges += 1;
            let source_id = graph[edge.source()].id;
            if let Some(&source_comm) = node_community.get(&source_id) {
                if source_comm != home_comm {
                    reached_communities.insert(source_comm);
                    cross_edges += 1;
                }
            }
        }

        if reached_communities.is_empty() {
            continue;
        }

        // Bridge score: communities_connected * cross_fraction * centrality
        let cross_fraction = if total_edges > 0 {
            cross_edges as f64 / total_edges as f64
        } else {
            0.0
        };

        let centrality = importance.get(&node_id).copied().unwrap_or(0.0);
        let communities_factor = reached_communities.len() as f64;

        let score = communities_factor * cross_fraction * (1.0 + centrality * 10.0);

        let mut community_ids: Vec<usize> = std::iter::once(home_comm)
            .chain(reached_communities)
            .collect();
        community_ids.sort_unstable();
        community_ids.dedup();

        bridges.push(Bridge {
            node_id,
            community_ids,
            score,
            cross_edges,
        });
    }

    // Sort by score descending
    bridges.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    bridges
}

/// Get the top-k bridges connecting specific community pairs.
pub fn bridges_between(
    bridges: &[Bridge],
    comm_a: usize,
    comm_b: usize,
    top_k: usize,
) -> Vec<&Bridge> {
    bridges
        .iter()
        .filter(|b| b.community_ids.contains(&comm_a) && b.community_ids.contains(&comm_b))
        .take(top_k)
        .collect()
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
    fn no_bridges_single_community() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        add_edge(&mut g, a, b);

        let comms = vec![Community {
            id: 0,
            node_ids: vec![a, b],
            label: None,
            coherence: 1.0,
            core_level: 0,
        }];

        let bridges = discover_bridges(g.graph(), &comms);
        assert!(bridges.is_empty());
    }

    #[test]
    fn bridge_connects_two_communities() {
        let mut g = GraphSidecar::new();
        // Community 0: A, B
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        add_edge(&mut g, a, b);
        add_edge(&mut g, b, a);

        // Community 1: C, D
        let c = add_node(&mut g, "C");
        let d = add_node(&mut g, "D");
        add_edge(&mut g, c, d);
        add_edge(&mut g, d, c);

        // Bridge: B -> C
        add_edge(&mut g, b, c);

        let comms = vec![
            Community {
                id: 0,
                node_ids: vec![a, b],
                label: None,
                coherence: 0.8,
                core_level: 0,
            },
            Community {
                id: 1,
                node_ids: vec![c, d],
                label: None,
                coherence: 0.8,
                core_level: 0,
            },
        ];

        let bridges = discover_bridges(g.graph(), &comms);
        assert!(!bridges.is_empty(), "Should find at least one bridge");

        // B should be a bridge (it connects community 0 to 1)
        let b_bridge = bridges.iter().find(|br| br.node_id == b);
        assert!(b_bridge.is_some(), "B should be identified as a bridge");
        assert!(b_bridge.unwrap().community_ids.contains(&0));
        assert!(b_bridge.unwrap().community_ids.contains(&1));
    }

    #[test]
    fn bridges_between_filters_correctly() {
        let bridges = vec![
            Bridge {
                node_id: Uuid::new_v4(),
                community_ids: vec![0, 1],
                score: 2.0,
                cross_edges: 2,
            },
            Bridge {
                node_id: Uuid::new_v4(),
                community_ids: vec![1, 2],
                score: 1.0,
                cross_edges: 1,
            },
            Bridge {
                node_id: Uuid::new_v4(),
                community_ids: vec![0, 2],
                score: 0.5,
                cross_edges: 1,
            },
        ];

        let between_01 = bridges_between(&bridges, 0, 1, 5);
        assert_eq!(between_01.len(), 1);
        assert_eq!(between_01[0].score, 2.0);

        let between_12 = bridges_between(&bridges, 1, 2, 5);
        assert_eq!(between_12.len(), 1);
    }

    #[test]
    fn bridge_score_higher_for_multi_community() {
        let mut g = GraphSidecar::new();
        // Three communities with a hub node
        let hub = add_node(&mut g, "Hub");

        let a = add_node(&mut g, "A");
        add_edge(&mut g, hub, a);
        add_edge(&mut g, a, hub);

        let b = add_node(&mut g, "B");
        add_edge(&mut g, hub, b);
        add_edge(&mut g, b, hub);

        let c = add_node(&mut g, "C");
        add_edge(&mut g, hub, c);
        add_edge(&mut g, c, hub);

        let comms = vec![
            Community {
                id: 0,
                node_ids: vec![hub],
                label: None,
                coherence: 0.5,
                core_level: 0,
            },
            Community {
                id: 1,
                node_ids: vec![a],
                label: None,
                coherence: 1.0,
                core_level: 0,
            },
            Community {
                id: 2,
                node_ids: vec![b],
                label: None,
                coherence: 1.0,
                core_level: 0,
            },
            Community {
                id: 3,
                node_ids: vec![c],
                label: None,
                coherence: 1.0,
                core_level: 0,
            },
        ];

        let bridges = discover_bridges(g.graph(), &comms);
        assert!(!bridges.is_empty());
        // Hub connects 3 external communities, should have high score
        let hub_bridge = bridges.iter().find(|br| br.node_id == hub);
        assert!(hub_bridge.is_some());
        assert!(
            hub_bridge.unwrap().community_ids.len() >= 3,
            "Hub should connect 3+ communities"
        );
    }
}
