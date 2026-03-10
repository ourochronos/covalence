//! Community detection via k-core decomposition (Matula-Beck algorithm).
//!
//! K-core decomposition replaces Louvain because it is deterministic,
//! runs in O(|E|) time, and produces a density-aware hierarchy where
//! higher k-cores represent denser, more cohesive subgraphs.
//!
//! Communities are ephemeral — computed during deep consolidation and
//! cached in sidecar memory. No PG table for v1.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::EdgeRef;
use uuid::Uuid;

use super::sidecar::{EdgeMeta, NodeMeta};

/// A detected community of related nodes.
#[derive(Debug, Clone)]
pub struct Community {
    /// Community identifier (sequential).
    pub id: usize,
    /// UUIDs of nodes in this community.
    pub node_ids: Vec<Uuid>,
    /// Optional label (generated post-detection via LLM or heuristic).
    pub label: Option<String>,
    /// Internal coherence: ratio of internal edge density to external.
    pub coherence: f64,
    /// The k-core level of this community (higher = denser).
    pub core_level: usize,
}

/// Compute the core number for every node using the Matula-Beck
/// algorithm.
///
/// Returns a mapping from `NodeIndex` to core number.
/// Core number k means the node is in the k-core but not the
/// (k+1)-core. The k-core is the maximal subgraph where every node
/// has at least k neighbors within it.
///
/// For directed graphs the effective degree is the combined count of
/// incoming and outgoing edges (weighted, rounded to the nearest
/// integer for bucket ordering).
pub fn compute_core_numbers(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
) -> HashMap<NodeIndex, usize> {
    let n = graph.node_count();
    if n == 0 {
        return HashMap::new();
    }

    // Effective degree: sum of edge weights for all incident edges
    // (both directions). We floor to usize for the peeling order.
    let mut degree: HashMap<NodeIndex, f64> = HashMap::with_capacity(n);
    for idx in graph.node_indices() {
        let out_w: f64 = graph.edges(idx).map(|e| graph[e.id()].weight).sum();
        let in_w: f64 = graph
            .edges_directed(idx, petgraph::Direction::Incoming)
            .map(|e| graph[e.id()].weight)
            .sum();
        degree.insert(idx, out_w + in_w);
    }

    // Min-heap keyed by current effective degree.
    let mut heap: BinaryHeap<Reverse<(usize, NodeIndex)>> = BinaryHeap::with_capacity(n);
    for (&idx, &deg) in &degree {
        heap.push(Reverse((deg as usize, idx)));
    }

    let mut removed: HashSet<NodeIndex> = HashSet::with_capacity(n);
    let mut core: HashMap<NodeIndex, usize> = HashMap::with_capacity(n);
    let mut current_k: usize = 0;

    while let Some(Reverse((deg_key, v))) = heap.pop() {
        if removed.contains(&v) {
            continue;
        }
        removed.insert(v);

        // Core number is max of current_k and the stored degree key.
        current_k = current_k.max(deg_key);
        core.insert(v, current_k);

        // Decrease effective degree of remaining neighbors.
        let neighbors: Vec<(NodeIndex, f64)> = graph
            .edges(v)
            .map(|e| (e.target(), graph[e.id()].weight))
            .chain(
                graph
                    .edges_directed(v, petgraph::Direction::Incoming)
                    .map(|e| (e.source(), graph[e.id()].weight)),
            )
            .collect();

        for (u, w) in neighbors {
            if removed.contains(&u) {
                continue;
            }
            if let Some(d) = degree.get_mut(&u) {
                *d = (*d - w).max(0.0);
                // Re-insert with updated key (stale entries filtered
                // by the `removed` check above).
                heap.push(Reverse((*d as usize, u)));
            }
        }
    }

    core
}

/// Detect communities by grouping nodes with the same k-core number
/// and splitting by connected component.
///
/// Nodes sharing a core number but in disconnected subgraphs become
/// separate communities. Communities with fewer than `min_size` nodes
/// are excluded. Communities are sorted by size (descending), with
/// IDs assigned after sorting. Higher `core_level` values indicate
/// denser, more cohesive subgraphs.
pub fn detect_communities(graph: &StableDiGraph<NodeMeta, EdgeMeta>) -> Vec<Community> {
    detect_communities_with_min_size(graph, 2)
}

/// Like [`detect_communities`] but with a configurable minimum
/// community size.
pub fn detect_communities_with_min_size(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    min_size: usize,
) -> Vec<Community> {
    if graph.node_count() == 0 {
        return Vec::new();
    }

    let core_numbers = compute_core_numbers(graph);

    // Group node indices by core number.
    let mut by_core: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
    for (&idx, &k) in &core_numbers {
        by_core.entry(k).or_default().push(idx);
    }

    // For each core level, split into connected components so that
    // disconnected subgraphs with the same density become separate
    // communities.
    let mut communities: Vec<Community> = Vec::new();
    for (core_level, indices) in &by_core {
        let idx_set: HashSet<NodeIndex> = indices.iter().copied().collect();
        let components = connected_components_within(graph, &idx_set);
        for component in components {
            if component.len() < min_size {
                continue;
            }
            let node_ids: Vec<Uuid> = component.iter().map(|&idx| graph[idx].id).collect();
            let component_set: HashSet<NodeIndex> = component.into_iter().collect();
            let coherence = compute_component_coherence(graph, &component_set);
            communities.push(Community {
                id: 0,
                node_ids,
                label: None,
                coherence,
                core_level: *core_level,
            });
        }
    }

    // Sort by size descending.
    communities.sort_by(|a, b| b.node_ids.len().cmp(&a.node_ids.len()));

    // Assign sequential IDs after sorting.
    for (i, comm) in communities.iter_mut().enumerate() {
        comm.id = i;
    }

    communities
}

/// Find connected components within a subset of graph nodes.
///
/// Returns a list of components, each being a set of `NodeIndex`
/// values that are reachable from each other (treating edges as
/// undirected) within the given subset.
fn connected_components_within(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    subset: &HashSet<NodeIndex>,
) -> Vec<Vec<NodeIndex>> {
    let mut visited: HashSet<NodeIndex> = HashSet::with_capacity(subset.len());
    let mut components = Vec::new();

    for &start in subset {
        if visited.contains(&start) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(v) = stack.pop() {
            if !visited.insert(v) {
                continue;
            }
            component.push(v);
            // Traverse outgoing edges.
            for edge in graph.edges(v) {
                if subset.contains(&edge.target()) && !visited.contains(&edge.target()) {
                    stack.push(edge.target());
                }
            }
            // Traverse incoming edges (undirected connectivity).
            for edge in graph.edges_directed(v, petgraph::Direction::Incoming) {
                if subset.contains(&edge.source()) && !visited.contains(&edge.source()) {
                    stack.push(edge.source());
                }
            }
        }
        components.push(component);
    }

    components
}

/// Compute coherence for a specific community component: ratio of
/// internal edge weight to total edge weight of those nodes.
fn compute_component_coherence(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    members: &HashSet<NodeIndex>,
) -> f64 {
    if members.len() <= 1 {
        return 1.0;
    }

    let mut internal_weight = 0.0;
    let mut total_weight = 0.0;

    for &idx in members {
        for edge in graph.edges(idx) {
            let w = graph[edge.id()].weight;
            total_weight += w;
            if members.contains(&edge.target()) {
                internal_weight += w;
            }
        }
    }

    if total_weight > 0.0 {
        internal_weight / total_weight
    } else {
        0.0
    }
}

/// Detect landmark nodes per community — nodes with highest
/// betweenness centrality.
///
/// Returns a map from community ID to the top-k landmark node UUIDs.
pub fn detect_landmarks(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    communities: &[Community],
    top_k: usize,
) -> HashMap<usize, Vec<Uuid>> {
    let importance = super::algorithms::structural_importance(graph);

    communities
        .iter()
        .map(|comm| {
            let mut scored: Vec<(Uuid, f64)> = comm
                .node_ids
                .iter()
                .map(|id| (*id, importance.get(id).copied().unwrap_or(0.0)))
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top_k);
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
            },
        )
        .unwrap();
    }

    #[test]
    fn detect_communities_single_node_filtered() {
        // A single isolated node is filtered out by the default
        // min_size=2 threshold.
        let mut g = GraphSidecar::new();
        let _a = add_node(&mut g, "A");
        let comms = detect_communities(g.graph());
        assert!(comms.is_empty());
    }

    #[test]
    fn detect_communities_single_node_min_size_1() {
        // With min_size=1, single nodes are included.
        let mut g = GraphSidecar::new();
        let _a = add_node(&mut g, "A");
        let comms = detect_communities_with_min_size(g.graph(), 1);
        assert_eq!(comms.len(), 1);
        assert_eq!(comms[0].node_ids.len(), 1);
        assert_eq!(comms[0].core_level, 0);
    }

    #[test]
    fn detect_communities_two_clusters() {
        // Two disconnected triangles should produce at least 2
        // communities (nodes within each triangle share the same
        // core number, and disconnected groups are separate).
        let mut g = GraphSidecar::new();
        let a1 = add_node(&mut g, "A1");
        let a2 = add_node(&mut g, "A2");
        let a3 = add_node(&mut g, "A3");
        add_edge(&mut g, a1, a2);
        add_edge(&mut g, a2, a1);
        add_edge(&mut g, a2, a3);
        add_edge(&mut g, a3, a2);
        add_edge(&mut g, a1, a3);
        add_edge(&mut g, a3, a1);

        let b1 = add_node(&mut g, "B1");
        let b2 = add_node(&mut g, "B2");
        let b3 = add_node(&mut g, "B3");
        add_edge(&mut g, b1, b2);
        add_edge(&mut g, b2, b1);
        add_edge(&mut g, b2, b3);
        add_edge(&mut g, b3, b2);
        add_edge(&mut g, b1, b3);
        add_edge(&mut g, b3, b1);

        let comms = detect_communities(g.graph());
        // Both triangles share the same core number but are
        // disconnected, so they should be split into separate
        // communities.
        assert!(
            comms.len() >= 2,
            "Expected >= 2 communities, got {}",
            comms.len()
        );
        // All 6 nodes should be accounted for.
        let total: usize = comms.iter().map(|c| c.node_ids.len()).sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn detect_landmarks_returns_top_k() {
        let mut g = GraphSidecar::new();
        let center = add_node(&mut g, "Center");
        let mut leaves = Vec::new();
        for i in 0..5 {
            let leaf = add_node(&mut g, &format!("Leaf{i}"));
            add_edge(&mut g, center, leaf);
            add_edge(&mut g, leaf, center);
            leaves.push(leaf);
        }

        let comms = vec![Community {
            id: 0,
            node_ids: std::iter::once(center)
                .chain(leaves.iter().copied())
                .collect(),
            label: None,
            coherence: 1.0,
            core_level: 1,
        }];

        let landmarks = detect_landmarks(g.graph(), &comms, 2);
        assert!(landmarks[&0].len() <= 2);
        // Center should be a landmark (highest betweenness)
        assert!(landmarks[&0].contains(&center));
    }

    #[test]
    fn detect_communities_empty_graph() {
        let g: StableDiGraph<NodeMeta, EdgeMeta> = StableDiGraph::new();
        let comms = detect_communities(&g);
        assert!(comms.is_empty());
    }

    #[test]
    fn core_numbers_triangle() {
        // Triangle: each node has 2 neighbors in each direction,
        // so combined degree = 4. After peeling, all nodes should
        // have core number 4 (bidirectional edges counted).
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        let c = add_node(&mut g, "C");
        add_edge(&mut g, a, b);
        add_edge(&mut g, b, a);
        add_edge(&mut g, b, c);
        add_edge(&mut g, c, b);
        add_edge(&mut g, a, c);
        add_edge(&mut g, c, a);

        let cores = compute_core_numbers(g.graph());
        // All nodes should have the same core number.
        let values: HashSet<usize> = cores.values().copied().collect();
        assert_eq!(values.len(), 1);
        // Each node has combined degree 4 (2 out + 2 in, weight 1
        // each). The minimum degree in the triangle stays uniform
        // through peeling, so core number = 4.
        assert!(cores.values().all(|&v| v == 4));
    }

    #[test]
    fn core_numbers_star() {
        // Star graph: center connects to 4 leaves (bidirectional).
        // Each leaf has combined degree 2 (1 out + 1 in).
        // Center has combined degree 8 (4 out + 4 in).
        // After peeling leaves (degree 2), center's effective degree
        // drops. All nodes end up with core number 2.
        let mut g = GraphSidecar::new();
        let center = add_node(&mut g, "Center");
        for i in 0..4 {
            let leaf = add_node(&mut g, &format!("L{i}"));
            add_edge(&mut g, center, leaf);
            add_edge(&mut g, leaf, center);
        }

        let cores = compute_core_numbers(g.graph());
        // Leaves have combined degree 2, so they peel at k=2.
        // After all leaves are removed, center has degree 0, but
        // current_k is already 2, so center also gets core 2.
        for (&_idx, &k) in &cores {
            assert_eq!(k, 2);
        }
    }

    #[test]
    fn core_numbers_empty() {
        let g: StableDiGraph<NodeMeta, EdgeMeta> = StableDiGraph::new();
        let cores = compute_core_numbers(&g);
        assert!(cores.is_empty());
    }

    #[test]
    fn communities_include_core_level() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        add_edge(&mut g, a, b);
        add_edge(&mut g, b, a);

        let comms = detect_communities(g.graph());
        assert!(!comms.is_empty());
        // Every community should have a core_level set.
        for comm in &comms {
            // core_level is a usize, so it's always >= 0.
            // For a bidirectional pair, combined degree = 2,
            // so core_level should be 2.
            assert_eq!(comm.core_level, 2);
        }
    }

    #[test]
    fn coherence_is_per_component() {
        // Two disconnected triangles at the same core level should
        // each have coherence 1.0 (all edges internal).
        let mut g = GraphSidecar::new();
        let a1 = add_node(&mut g, "A1");
        let a2 = add_node(&mut g, "A2");
        let a3 = add_node(&mut g, "A3");
        add_edge(&mut g, a1, a2);
        add_edge(&mut g, a2, a1);
        add_edge(&mut g, a2, a3);
        add_edge(&mut g, a3, a2);
        add_edge(&mut g, a1, a3);
        add_edge(&mut g, a3, a1);

        let b1 = add_node(&mut g, "B1");
        let b2 = add_node(&mut g, "B2");
        let b3 = add_node(&mut g, "B3");
        add_edge(&mut g, b1, b2);
        add_edge(&mut g, b2, b1);
        add_edge(&mut g, b2, b3);
        add_edge(&mut g, b3, b2);
        add_edge(&mut g, b1, b3);
        add_edge(&mut g, b3, b1);

        let comms = detect_communities(g.graph());
        assert_eq!(comms.len(), 2);
        // Both disconnected triangles have only internal edges,
        // so coherence should be 1.0 for each.
        for comm in &comms {
            assert!(
                (comm.coherence - 1.0).abs() < 1e-10,
                "expected coherence 1.0, got {}",
                comm.coherence
            );
        }
    }

    #[test]
    fn min_size_filters_small_communities() {
        // Graph with a triangle + an isolated pair.
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        let c = add_node(&mut g, "C");
        add_edge(&mut g, a, b);
        add_edge(&mut g, b, a);
        add_edge(&mut g, b, c);
        add_edge(&mut g, c, b);
        add_edge(&mut g, a, c);
        add_edge(&mut g, c, a);

        // Isolated node (no edges) — core level 0, size 1.
        let _d = add_node(&mut g, "D");

        // Default min_size=2 should exclude the isolated node.
        let comms = detect_communities(g.graph());
        let total: usize = comms.iter().map(|c| c.node_ids.len()).sum();
        assert_eq!(total, 3); // Only the triangle.

        // min_size=1 includes everything.
        let all = detect_communities_with_min_size(g.graph(), 1);
        let total_all: usize = all.iter().map(|c| c.node_ids.len()).sum();
        assert_eq!(total_all, 4);
    }
}
