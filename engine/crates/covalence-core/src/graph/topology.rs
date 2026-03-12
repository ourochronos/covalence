//! Domain topology map generation and landmark article identification.
//!
//! Builds a high-level map of knowledge domains from community detection,
//! PageRank, and bridge discovery. Each domain corresponds to a detected
//! community, enriched with landmark nodes and inter-domain connections.

use std::collections::HashMap;

use petgraph::stable_graph::StableDiGraph;
use uuid::Uuid;

use super::algorithms::{pagerank, structural_importance};
use super::bridges::{Bridge, discover_bridges};
use super::community::{Community, detect_communities, detect_landmarks};
use super::sidecar::{EdgeMeta, NodeMeta};

/// A domain in the topology map, derived from a community.
#[derive(Debug, Clone)]
pub struct Domain {
    /// Community identifier (sequential).
    pub community_id: usize,
    /// Optional label (from community detection).
    pub label: Option<String>,
    /// Number of nodes in this domain.
    pub node_count: usize,
    /// UUIDs of landmark nodes (top-3 by structural importance).
    pub landmark_ids: Vec<Uuid>,
    /// Internal coherence score from community detection.
    pub coherence: f64,
    /// Average PageRank of nodes in this domain.
    pub avg_pagerank: f64,
}

/// An inter-domain connection in the topology map.
#[derive(Debug, Clone)]
pub struct DomainLink {
    /// Source domain (community ID).
    pub source_domain: usize,
    /// Target domain (community ID).
    pub target_domain: usize,
    /// Number of bridge nodes connecting these domains.
    pub bridge_count: usize,
    /// UUID of the bridge node with the highest score.
    pub strongest_bridge: Uuid,
}

/// The complete domain topology map.
#[derive(Debug, Clone)]
pub struct TopologyMap {
    /// Domains (one per community).
    pub domains: Vec<Domain>,
    /// Inter-domain links derived from bridge nodes.
    pub links: Vec<DomainLink>,
    /// Total node count in the graph.
    pub total_nodes: usize,
    /// Total edge count in the graph.
    pub total_edges: usize,
}

/// Build a domain topology map from the in-memory graph.
///
/// 1. Runs community detection (Louvain).
/// 2. Computes PageRank scores.
/// 3. Discovers cross-domain bridges.
/// 4. For each community, computes average PageRank and identifies
///    top-3 landmark nodes by structural importance.
/// 5. For each pair of communities connected by bridges, creates a
///    `DomainLink` with the strongest bridge.
pub fn build_topology(graph: &StableDiGraph<NodeMeta, EdgeMeta>) -> TopologyMap {
    let total_nodes = graph.node_count();
    let total_edges = graph.edge_count();

    if total_nodes == 0 {
        return TopologyMap {
            domains: Vec::new(),
            links: Vec::new(),
            total_nodes: 0,
            total_edges: 0,
        };
    }

    // Step 1: Community detection + labeling
    let mut communities = detect_communities(graph);
    super::community::label_communities(graph, &mut communities);

    // Step 2: PageRank
    let pr_scores = pagerank(graph, 0.85, 50);

    // Step 3: Bridge discovery
    let bridges = discover_bridges(graph, &communities);

    // Step 4: Landmark detection (top 3 per community)
    let landmarks = detect_landmarks(graph, &communities, 3);

    // Build domains
    let domains: Vec<Domain> = communities
        .iter()
        .map(|comm| {
            let avg_pr = if comm.node_ids.is_empty() {
                0.0
            } else {
                let sum: f64 = comm
                    .node_ids
                    .iter()
                    .map(|id| pr_scores.get(id).copied().unwrap_or(0.0))
                    .sum();
                sum / comm.node_ids.len() as f64
            };

            Domain {
                community_id: comm.id,
                label: comm.label.clone(),
                node_count: comm.node_ids.len(),
                landmark_ids: landmarks.get(&comm.id).cloned().unwrap_or_default(),
                coherence: comm.coherence,
                avg_pagerank: avg_pr,
            }
        })
        .collect();

    // Step 5: Build domain links from bridges
    let links = build_domain_links(&bridges);

    TopologyMap {
        domains,
        links,
        total_nodes,
        total_edges,
    }
}

/// Build `DomainLink` entries from bridge nodes.
///
/// For each unique pair of communities connected by bridges, creates a
/// link with the count of bridges and the UUID of the strongest one.
fn build_domain_links(bridges: &[Bridge]) -> Vec<DomainLink> {
    // Key: (min_comm, max_comm) -> (count, strongest_bridge_id, strongest_score)
    let mut pair_map: HashMap<(usize, usize), (usize, Uuid, f64)> = HashMap::new();

    for bridge in bridges {
        // Each bridge can connect multiple communities; create links for each pair
        let comm_ids = &bridge.community_ids;
        for i in 0..comm_ids.len() {
            for j in (i + 1)..comm_ids.len() {
                let a = comm_ids[i].min(comm_ids[j]);
                let b = comm_ids[i].max(comm_ids[j]);
                let entry = pair_map
                    .entry((a, b))
                    .or_insert((0, bridge.node_id, bridge.score));
                entry.0 += 1;
                if bridge.score > entry.2 {
                    entry.1 = bridge.node_id;
                    entry.2 = bridge.score;
                }
            }
        }
    }

    pair_map
        .into_iter()
        .map(|((a, b), (count, strongest, _score))| DomainLink {
            source_domain: a,
            target_domain: b,
            bridge_count: count,
            strongest_bridge: strongest,
        })
        .collect()
}

/// Identify landmark articles per community.
///
/// For each community, finds the nodes with the highest structural
/// importance. These node UUIDs can be cross-referenced against the
/// `articles.source_node_ids` column to find landmark articles.
pub fn identify_landmark_articles(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    communities: &[Community],
) -> HashMap<usize, Vec<Uuid>> {
    let importance = structural_importance(graph);

    communities
        .iter()
        .map(|comm| {
            let mut scored: Vec<(Uuid, f64)> = comm
                .node_ids
                .iter()
                .map(|id| (*id, importance.get(id).copied().unwrap_or(0.0)))
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            // Return top landmark nodes — these are the nodes whose
            // associated articles are "landmark articles" for the domain.
            scored.truncate(5);
            (comm.id, scored.into_iter().map(|(id, _)| id).collect())
        })
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
            },
        )
        .unwrap();
    }

    #[test]
    fn topology_two_disconnected_clusters() {
        let mut g = GraphSidecar::new();

        // Cluster A: fully connected triangle
        let a1 = add_node(&mut g, "A1");
        let a2 = add_node(&mut g, "A2");
        let a3 = add_node(&mut g, "A3");
        add_edge(&mut g, a1, a2);
        add_edge(&mut g, a2, a1);
        add_edge(&mut g, a2, a3);
        add_edge(&mut g, a3, a2);
        add_edge(&mut g, a1, a3);
        add_edge(&mut g, a3, a1);

        // Cluster B: fully connected triangle
        let b1 = add_node(&mut g, "B1");
        let b2 = add_node(&mut g, "B2");
        let b3 = add_node(&mut g, "B3");
        add_edge(&mut g, b1, b2);
        add_edge(&mut g, b2, b1);
        add_edge(&mut g, b2, b3);
        add_edge(&mut g, b3, b2);
        add_edge(&mut g, b1, b3);
        add_edge(&mut g, b3, b1);

        let topo = build_topology(g.graph());

        // Should have at least 2 domains
        assert!(
            topo.domains.len() >= 2,
            "Expected >= 2 domains, got {}",
            topo.domains.len()
        );
        assert_eq!(topo.total_nodes, 6);
        assert_eq!(topo.total_edges, 12);

        // No links between disconnected clusters
        assert!(
            topo.links.is_empty(),
            "Disconnected clusters should have no domain links"
        );

        // Each domain should have landmarks
        for domain in &topo.domains {
            assert!(
                !domain.landmark_ids.is_empty(),
                "Domain {} should have landmarks",
                domain.community_id
            );
            assert!(domain.avg_pagerank > 0.0);
        }
    }

    #[test]
    fn topology_connected_clusters_have_links() {
        let mut g = GraphSidecar::new();

        // Cluster A
        let a1 = add_node(&mut g, "A1");
        let a2 = add_node(&mut g, "A2");
        let a3 = add_node(&mut g, "A3");
        add_edge(&mut g, a1, a2);
        add_edge(&mut g, a2, a1);
        add_edge(&mut g, a2, a3);
        add_edge(&mut g, a3, a2);
        add_edge(&mut g, a1, a3);
        add_edge(&mut g, a3, a1);

        // Cluster B
        let b1 = add_node(&mut g, "B1");
        let b2 = add_node(&mut g, "B2");
        let b3 = add_node(&mut g, "B3");
        add_edge(&mut g, b1, b2);
        add_edge(&mut g, b2, b1);
        add_edge(&mut g, b2, b3);
        add_edge(&mut g, b3, b2);
        add_edge(&mut g, b1, b3);
        add_edge(&mut g, b3, b1);

        // Bridge: a3 <-> b1
        add_edge(&mut g, a3, b1);
        add_edge(&mut g, b1, a3);

        let topo = build_topology(g.graph());

        // The bridge edge may or may not cause community detection to
        // merge the clusters (depends on modularity). If they are
        // separate, we should see links. If merged, that is also valid.
        if topo.domains.len() >= 2 {
            assert!(
                !topo.links.is_empty(),
                "Connected clusters should produce domain links"
            );
            for link in &topo.links {
                assert!(link.bridge_count > 0);
            }
        }
    }

    #[test]
    fn topology_empty_graph() {
        let g: StableDiGraph<NodeMeta, EdgeMeta> = StableDiGraph::new();
        let topo = build_topology(&g);
        assert!(topo.domains.is_empty());
        assert!(topo.links.is_empty());
        assert_eq!(topo.total_nodes, 0);
        assert_eq!(topo.total_edges, 0);
    }

    #[test]
    fn identify_landmarks_per_community() {
        let mut g = GraphSidecar::new();
        let center = add_node(&mut g, "Center");
        let mut leaves = Vec::new();
        for i in 0..5 {
            let leaf = add_node(&mut g, &format!("Leaf{i}"));
            add_edge(&mut g, center, leaf);
            add_edge(&mut g, leaf, center);
            leaves.push(leaf);
        }

        let communities = vec![Community {
            id: 0,
            node_ids: std::iter::once(center)
                .chain(leaves.iter().copied())
                .collect(),
            label: None,
            coherence: 1.0,
            core_level: 0,
        }];

        let landmarks = identify_landmark_articles(g.graph(), &communities);
        assert!(landmarks.contains_key(&0));
        // Center has highest betweenness, should be first landmark
        assert!(landmarks[&0].contains(&center));
    }
}
