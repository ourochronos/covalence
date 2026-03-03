//! Graph algorithms for the Covalence knowledge engine.
//!
//! Ported from valence-v2 with adaptations for `CovalenceGraph`:
//! - `GraphView` → `&CovalenceGraph`
//! - `NodeId` → `Uuid`

#![allow(dead_code)]
//! - `graph.get_node_id(idx)` → `g.graph.node_weight(idx).copied()`
//! - `graph.get_index(id)` → `g.index.get(&id).copied()`
//! - `graph.node_count()` → `g.node_count()`

use petgraph::Direction;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet, VecDeque};
use uuid::Uuid;

use super::memory::CovalenceGraph;

/// Compute PageRank scores for all nodes in the graph.
///
/// # Arguments
/// * `graph`      – The in-memory graph
/// * `damping`    – Damping factor (typically 0.85)
/// * `iterations` – Number of power-iteration rounds
///
/// # Returns
/// `HashMap` mapping each `Uuid` node to its PageRank score.
pub fn pagerank(graph: &CovalenceGraph, damping: f64, iterations: u32) -> HashMap<Uuid, f64> {
    let node_count = graph.node_count();
    if node_count == 0 {
        return HashMap::new();
    }

    let initial_rank = 1.0 / node_count as f64;
    let damping_term = (1.0 - damping) / node_count as f64;

    // Initialise per-index ranks.
    let mut ranks: HashMap<NodeIndex, f64> = graph
        .graph
        .node_indices()
        .map(|idx| (idx, initial_rank))
        .collect();

    for _ in 0..iterations {
        let mut new_ranks: HashMap<NodeIndex, f64> = HashMap::new();

        for node_idx in graph.graph.node_indices() {
            let mut rank_sum = 0.0;

            for edge in graph.graph.edges_directed(node_idx, Direction::Incoming) {
                let source_idx = edge.source();
                let source_rank = ranks.get(&source_idx).copied().unwrap_or(0.0);
                let out_degree = graph
                    .graph
                    .edges_directed(source_idx, Direction::Outgoing)
                    .count();

                if out_degree > 0 {
                    rank_sum += source_rank / out_degree as f64;
                }
            }

            new_ranks.insert(node_idx, damping_term + damping * rank_sum);
        }

        ranks = new_ranks;
    }

    // Translate NodeIndex → Uuid.
    ranks
        .into_iter()
        .filter_map(|(idx, rank)| graph.graph.node_weight(idx).copied().map(|id| (id, rank)))
        .collect()
}

/// Find all strongly connected components using Kosaraju's algorithm.
///
/// # Returns
/// A `Vec` of components; each component is a `Vec<Uuid>`.
pub fn connected_components(graph: &CovalenceGraph) -> Vec<Vec<Uuid>> {
    use petgraph::algo::kosaraju_scc;

    kosaraju_scc(&graph.graph)
        .into_iter()
        .map(|component| {
            component
                .into_iter()
                .filter_map(|idx| graph.graph.node_weight(idx).copied())
                .collect()
        })
        .collect()
}

/// Find the shortest (fewest-hops) path between two nodes using BFS.
///
/// # Returns
/// `Some(path)` — ordered `Vec<Uuid>` from `from` to `to` inclusive —
/// or `None` if no path exists or either node is absent.
pub fn shortest_path(graph: &CovalenceGraph, from: Uuid, to: Uuid) -> Option<Vec<Uuid>> {
    let from_idx = graph.index.get(&from).copied()?;
    let to_idx = graph.index.get(&to).copied()?;

    let mut queue = VecDeque::new();
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut parent: HashMap<NodeIndex, NodeIndex> = HashMap::new();

    queue.push_back(from_idx);
    visited.insert(from_idx);

    while let Some(current) = queue.pop_front() {
        if current == to_idx {
            // Reconstruct path by walking parent pointers.
            let mut path = vec![to_idx];
            let mut node = to_idx;
            while let Some(&prev) = parent.get(&node) {
                path.push(prev);
                node = prev;
            }
            path.reverse();
            return Some(
                path.into_iter()
                    .filter_map(|idx| graph.graph.node_weight(idx).copied())
                    .collect(),
            );
        }

        for neighbor in graph.graph.neighbors(current) {
            if !visited.contains(&neighbor) {
                visited.insert(neighbor);
                parent.insert(neighbor, current);
                queue.push_back(neighbor);
            }
        }
    }

    None
}

/// Compute betweenness centrality for every node.
///
/// Betweenness centrality measures how often a node lies on shortest paths
/// between all other pairs of nodes in the graph.
///
/// # Returns
/// `HashMap` mapping each `Uuid` to its normalised betweenness centrality.
pub fn betweenness_centrality(graph: &CovalenceGraph) -> HashMap<Uuid, f64> {
    let mut centrality: HashMap<NodeIndex, f64> =
        graph.graph.node_indices().map(|idx| (idx, 0.0)).collect();

    for source in graph.graph.node_indices() {
        // BFS to collect all shortest paths from `source`.
        let mut paths: HashMap<NodeIndex, Vec<Vec<NodeIndex>>> = graph
            .graph
            .node_indices()
            .map(|idx| (idx, Vec::new()))
            .collect();
        let mut dist: HashMap<NodeIndex, i32> =
            graph.graph.node_indices().map(|idx| (idx, -1)).collect();

        paths.insert(source, vec![vec![source]]);
        dist.insert(source, 0);

        let mut queue = VecDeque::new();
        queue.push_back(source);

        while let Some(current) = queue.pop_front() {
            let current_dist = dist[&current];

            for neighbor in graph.graph.neighbors(current) {
                if dist[&neighbor] == -1 {
                    dist.insert(neighbor, current_dist + 1);
                    queue.push_back(neighbor);
                }

                if dist[&neighbor] == current_dist + 1 {
                    let current_paths = paths[&current].clone();
                    for mut path in current_paths {
                        path.push(neighbor);
                        paths.get_mut(&neighbor).unwrap().push(path);
                    }
                }
            }
        }

        // Accumulate centrality contribution for each reachable target.
        for node_idx in graph.graph.node_indices() {
            if node_idx == source {
                continue;
            }

            let node_paths = &paths[&node_idx];
            if node_paths.is_empty() {
                continue;
            }

            let total_paths = node_paths.len() as f64;
            let mut intermediate_counts: HashMap<NodeIndex, usize> = HashMap::new();

            for path in node_paths {
                // Intermediates are every node except the first and last.
                for &intermediate in path.iter().skip(1).take(path.len().saturating_sub(2)) {
                    *intermediate_counts.entry(intermediate).or_insert(0) += 1;
                }
            }

            for (intermediate, count) in intermediate_counts {
                *centrality.get_mut(&intermediate).unwrap() += count as f64 / total_paths;
            }
        }
    }

    // Normalise by (n-1)(n-2) — the maximum number of ordered pairs.
    let node_count = graph.node_count();
    if node_count > 2 {
        let normalization = ((node_count - 1) * (node_count - 2)) as f64;
        for score in centrality.values_mut() {
            *score /= normalization;
        }
    }

    centrality
        .into_iter()
        .filter_map(|(idx, score)| graph.graph.node_weight(idx).copied().map(|id| (id, score)))
        .collect()
}

/// Compute PageRank scores restricted to a filtered subgraph.
///
/// Only edges whose type label appears in `edge_types` participate in the
/// PageRank walk.  Nodes that become isolated within the filtered subgraph
/// still receive the baseline damping score `(1 - d) / N`.
///
/// # Arguments
/// * `graph`      – The full in-memory graph
/// * `damping`    – Damping factor (typically 0.85)
/// * `iterations` – Number of power-iteration rounds
/// * `edge_types` – Allowlist of edge-type labels (e.g. `["CONFIRMS", "ORIGINATES"]`)
///
/// # Returns
/// `HashMap` mapping each `Uuid` node to its filtered PageRank score,
/// or an empty map when `edge_types` is empty or the graph has no nodes.
pub fn pagerank_filtered(
    graph: &CovalenceGraph,
    damping: f64,
    iterations: u32,
    edge_types: &[String],
) -> HashMap<Uuid, f64> {
    let node_count = graph.node_count();
    if node_count == 0 || edge_types.is_empty() {
        return HashMap::new();
    }

    let initial_rank = 1.0 / node_count as f64;
    let damping_term = (1.0 - damping) / node_count as f64;

    let mut ranks: HashMap<NodeIndex, f64> = graph
        .graph
        .node_indices()
        .map(|idx| (idx, initial_rank))
        .collect();

    for _ in 0..iterations {
        let mut new_ranks: HashMap<NodeIndex, f64> = HashMap::new();

        for node_idx in graph.graph.node_indices() {
            let mut rank_sum = 0.0;

            for edge in graph.graph.edges_directed(node_idx, Direction::Incoming) {
                // Only traverse edges whose type is in the allowlist.
                if !edge_types.contains(edge.weight()) {
                    continue;
                }

                let source_idx = edge.source();
                let source_rank = ranks.get(&source_idx).copied().unwrap_or(0.0);

                // Out-degree counts only filtered edges from the source.
                let out_degree = graph
                    .graph
                    .edges_directed(source_idx, Direction::Outgoing)
                    .filter(|e| edge_types.contains(e.weight()))
                    .count();

                if out_degree > 0 {
                    rank_sum += source_rank / out_degree as f64;
                }
            }

            new_ranks.insert(node_idx, damping_term + damping * rank_sum);
        }

        ranks = new_ranks;
    }

    ranks
        .into_iter()
        .filter_map(|(idx, rank)| graph.graph.node_weight(idx).copied().map(|id| (id, rank)))
        .collect()
}

/// Count distinct paths between two nodes up to `max_depth` hops (DFS).
///
/// Used for path-diversity metrics in confidence scoring.
pub fn count_distinct_paths(graph: &CovalenceGraph, from: Uuid, to: Uuid, max_depth: u32) -> usize {
    let Some(&from_idx) = graph.index.get(&from) else {
        return 0;
    };
    let Some(&to_idx) = graph.index.get(&to) else {
        return 0;
    };

    let mut path_count = 0usize;
    // Stack entries: (current_node, visited_set, depth_so_far)
    let mut stack: Vec<(NodeIndex, HashSet<NodeIndex>, u32)> = vec![(from_idx, HashSet::new(), 0)];

    while let Some((current, mut visited, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }

        if current == to_idx {
            path_count += 1;
            continue;
        }

        visited.insert(current);

        for neighbor in graph.graph.neighbors(current) {
            if !visited.contains(&neighbor) {
                stack.push((neighbor, visited.clone(), depth + 1));
            }
        }
    }

    path_count
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagerank_basic() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_edge(c, b, "ORIGINATES".to_string());
        let ranks = pagerank(&g, 0.85, 20);
        assert!(ranks[&b] > ranks[&a]);
        assert!(ranks[&b] > ranks[&c]);
    }

    #[test]
    fn test_pagerank_empty() {
        let g = CovalenceGraph::new();
        assert_eq!(pagerank(&g, 0.85, 10).len(), 0);
    }

    #[test]
    fn test_shortest_path_basic() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_edge(b, c, "ORIGINATES".to_string());
        let path = shortest_path(&g, a, c).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], a);
        assert_eq!(path[2], c);
    }

    #[test]
    fn test_shortest_path_no_path() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_node(a);
        g.add_node(b);
        assert!(shortest_path(&g, a, b).is_none());
    }

    #[test]
    fn test_connected_components_two_islands() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        g.add_edge(a, b, "X".to_string());
        g.add_edge(c, d, "X".to_string());
        let comps = connected_components(&g);
        assert!(comps.len() >= 2);
    }

    #[test]
    fn test_neighbors_filtered() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_edge(a, c, "CONFIRMS".to_string());

        // Single-type filter — only ORIGINATES should surface b
        let filter = vec!["ORIGINATES".to_string()];
        let neighbors: Vec<Uuid> = g
            .neighbors_filtered(&a, Some(&filter))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0], b);
    }

    // ── pagerank_filtered tests ───────────────────────────────────────────────

    #[test]
    fn test_pagerank_filtered_basic() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        // b receives two CONFIRMS inbound links; c receives one PRECEDES link.
        g.add_edge(a, b, "CONFIRMS".to_string());
        g.add_edge(c, b, "CONFIRMS".to_string());
        g.add_edge(a, c, "PRECEDES".to_string());

        // Filter to factual (CONFIRMS, ORIGINATES): the PRECEDES edge is ignored.
        let types = vec!["CONFIRMS".to_string(), "ORIGINATES".to_string()];
        let ranks = pagerank_filtered(&g, 0.85, 20, &types);

        // b should score higher than a and c in the factual subgraph.
        assert!(
            ranks[&b] > ranks[&a],
            "b should outrank a in factual subgraph"
        );
        assert!(
            ranks[&b] > ranks[&c],
            "b should outrank c in factual subgraph"
        );
    }

    #[test]
    fn test_pagerank_filtered_empty_types() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        // Empty edge_types → empty result
        let ranks = pagerank_filtered(&g, 0.85, 20, &[]);
        assert!(ranks.is_empty());
    }

    #[test]
    fn test_pagerank_filtered_no_matching_edges() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_edge(a, b, "PRECEDES".to_string());

        // Filter to CONFIRMS — no edges match, so all nodes get the flat
        // baseline damping score (uniform distribution).
        let types = vec!["CONFIRMS".to_string()];
        let ranks = pagerank_filtered(&g, 0.85, 20, &types);
        // Both nodes should have equal rank (no structural advantage).
        let ra = ranks[&a];
        let rb = ranks[&b];
        let diff = (ra - rb).abs();
        assert!(diff < 1e-9, "all ranks should be equal when no edges match");
    }
}
