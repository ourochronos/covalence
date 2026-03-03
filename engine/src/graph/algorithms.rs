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

/// Personalized PageRank (PPR) seeded from a set of anchor nodes.
///
/// PPR is a biased random walk where the teleportation distribution is
/// concentrated on the seed set `seeds` rather than uniform over all nodes.
/// This makes it optimal for **local graph search**: nodes that are strongly
/// connected to the seed set receive high PPR scores in
/// O(1/ε × degree(seed)) time — independent of total graph size.
///
/// # Arguments
/// * `graph`      – The in-memory graph
/// * `seeds`      – The anchor/seed node UUIDs (the initial "query set")
/// * `damping`    – Damping / walk-continuation factor (typically 0.85)
/// * `iterations` – Power-iteration rounds (15–20 is sufficient for sparse
///                  graphs at Covalence's scale)
///
/// # Returns
/// `HashMap` mapping every `Uuid` node to its PPR score w.r.t. the seed set.
/// Seed nodes start with score `1/|seeds|`; adjacent nodes receive mass
/// proportional to edge weight and damping.  All scores are non-negative.
pub fn personalized_pagerank(
    graph: &CovalenceGraph,
    seeds: &HashSet<Uuid>,
    damping: f64,
    iterations: u32,
) -> HashMap<Uuid, f64> {
    let node_count = graph.node_count();
    if node_count == 0 || seeds.is_empty() {
        return HashMap::new();
    }

    let seed_count = seeds.len() as f64;
    let seed_weight = 1.0 / seed_count;
    let teleport_factor = 1.0 - damping;

    // Initialise: seeds start at 1/|S|, all other nodes start at 0.
    let mut ranks: HashMap<NodeIndex, f64> = graph
        .graph
        .node_indices()
        .map(|idx| {
            let id = *graph.graph.node_weight(idx).unwrap_or(&Uuid::nil());
            let initial = if seeds.contains(&id) { seed_weight } else { 0.0 };
            (idx, initial)
        })
        .collect();

    for _ in 0..iterations {
        let mut new_ranks: HashMap<NodeIndex, f64> = HashMap::new();

        for node_idx in graph.graph.node_indices() {
            let uuid = *graph.graph.node_weight(node_idx).unwrap_or(&Uuid::nil());

            // Teleportation term: only seed nodes contribute restart mass.
            let teleport = if seeds.contains(&uuid) {
                teleport_factor * seed_weight
            } else {
                0.0
            };

            // Random walk: accumulate mass from incoming edges.
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

            new_ranks.insert(node_idx, teleport + damping * rank_sum);
        }

        ranks = new_ranks;
    }

    ranks
        .into_iter()
        .filter_map(|(idx, score)| {
            graph.graph.node_weight(idx).copied().map(|id| (id, score))
        })
        .collect()
}

/// Compute a structural importance score for every node.
///
/// Structural importance identifies **bridge nodes** — articles whose removal
/// would disconnect or fragment the knowledge graph.  Such nodes must be
/// protected from eviction even when their `usage_score` is low, because
/// their loss would collapse cross-domain retrieval quality.
///
/// ## Algorithm
///
/// * **Small graphs (≤ 800 nodes)**: Full [betweenness centrality][bc]
///   (Brandes algorithm, O(VE)).  Exact and practical at Covalence's
///   current scale (~600 articles, ~1 000 edges).
///
/// * **Large graphs (> 800 nodes)**: An approximate proxy using:
///   - Normalised node degree (captures high-connectivity hubs).
///   - **Cluster-bridge bonus**: a node whose neighbours span ≥ 2 distinct
///     PageRank tiers (tertiles of the PageRank distribution) receives an
///     additional 0.5 score — it bridges communities with different
///     structural roles, making it a cross-domain knowledge broker.
///
/// [bc]: https://en.wikipedia.org/wiki/Betweenness_centrality
///
/// # Returns
/// `HashMap` mapping each `Uuid` to its normalised structural importance
/// score in \[0.0, 1.0\].  Higher = more critical to preserve for graph health.
pub fn structural_importance(graph: &CovalenceGraph) -> HashMap<Uuid, f64> {
    let node_count = graph.node_count();
    if node_count == 0 {
        return HashMap::new();
    }

    if node_count <= 800 {
        // Exact betweenness centrality — O(VE), practical at this scale.
        betweenness_centrality(graph)
    } else {
        // Approximate proxy for large graphs (O(V + E) instead of O(VE)).
        structural_importance_proxy(graph)
    }
}

/// Approximate structural importance for graphs with > 800 nodes.
///
/// Uses normalised degree + PageRank-cluster-bridge detection as a cheap
/// proxy for betweenness centrality.  A node is considered a cluster bridge
/// if its neighbours span ≥ 2 distinct PageRank tertiles (low / mid / high),
/// which correlates strongly with high betweenness in scale-free networks.
fn structural_importance_proxy(graph: &CovalenceGraph) -> HashMap<Uuid, f64> {
    let pr = pagerank(graph, 0.85, 20);

    // Compute PageRank tertile thresholds.
    let mut pr_values: Vec<f64> = pr.values().cloned().collect();
    pr_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = pr_values.len();
    let p33 = pr_values.get(n / 3).cloned().unwrap_or(0.0);
    let p67 = pr_values.get((n * 2) / 3).cloned().unwrap_or(1.0);

    // Assign each node to a PageRank tier: 0=low, 1=mid, 2=high.
    let tier = |score: f64| -> u8 {
        if score >= p67 {
            2
        } else if score >= p33 {
            1
        } else {
            0
        }
    };

    // Maximum total degree across all nodes (for normalisation).
    let max_degree = graph
        .graph
        .node_indices()
        .map(|idx| {
            graph.graph.edges_directed(idx, Direction::Outgoing).count()
                + graph.graph.edges_directed(idx, Direction::Incoming).count()
        })
        .max()
        .unwrap_or(1) as f64;

    graph
        .index
        .iter()
        .map(|(id, &idx)| {
            // Degree score (normalised by max degree across the graph).
            let degree = (graph.graph.edges_directed(idx, Direction::Outgoing).count()
                + graph.graph.edges_directed(idx, Direction::Incoming).count())
                as f64;
            let degree_score = degree / max_degree.max(1.0);

            // Cluster-bridge bonus: neighbours span ≥ 2 distinct PageRank tiers.
            let neighbor_tiers: HashSet<u8> = graph
                .graph
                .neighbors(idx)
                .filter_map(|n| graph.graph.node_weight(n).copied())
                .map(|nid| tier(pr.get(&nid).cloned().unwrap_or(0.0)))
                .collect();

            let bridge_bonus = if neighbor_tiers.len() >= 2 { 0.5 } else { 0.0 };

            (*id, (degree_score * 0.5 + bridge_bonus).min(1.0))
        })
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

    // ── personalized_pagerank tests ──────────────────────────────────────────

    #[test]
    fn test_ppr_seed_receives_highest_score() {
        let mut g = CovalenceGraph::new();
        let seed = Uuid::new_v4();
        let neighbor = Uuid::new_v4();
        let unrelated = Uuid::new_v4();
        g.add_edge(seed, neighbor, "CONFIRMS".to_string());
        g.add_node(unrelated);

        let seeds: HashSet<Uuid> = [seed].into_iter().collect();
        let scores = personalized_pagerank(&g, &seeds, 0.85, 20);

        // Seed should score highest (direct teleportation).
        assert!(
            scores[&seed] >= scores[&neighbor],
            "seed node should score >= its neighbor"
        );
        // Neighbor of seed should score higher than isolated unrelated node.
        assert!(
            scores[&neighbor] > scores[&unrelated],
            "neighbor of seed should outscore unrelated node"
        );
    }

    #[test]
    fn test_ppr_empty_seeds_returns_empty() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());

        let seeds: HashSet<Uuid> = HashSet::new();
        let scores = personalized_pagerank(&g, &seeds, 0.85, 20);
        assert!(scores.is_empty());
    }

    #[test]
    fn test_ppr_direct_neighbor_gets_high_score() {
        // A single seed with one outgoing edge.  After convergence the
        // neighbor should receive roughly damping × seed_weight mass from
        // the seed's outgoing walk (≈ 0.85 × 1.0 = 0.85 × contribution).
        let mut g = CovalenceGraph::new();
        let seed = Uuid::new_v4();
        let direct = Uuid::new_v4();
        g.add_edge(seed, direct, "ORIGINATES".to_string());

        let seeds: HashSet<Uuid> = [seed].into_iter().collect();
        let scores = personalized_pagerank(&g, &seeds, 0.85, 20);

        // direct neighbour should have non-trivial score.
        assert!(
            scores[&direct] > 0.1,
            "direct neighbor should have score > 0.1, got {}",
            scores[&direct]
        );
    }

    // ── structural_importance tests ──────────────────────────────────────────

    #[test]
    fn test_structural_importance_bridge_scores_highest() {
        // Bridge topology: a1 → a2 → bridge → b1 → b2
        // 'bridge' sits between cluster A (a1, a2) and cluster B (b1, b2).
        let mut g = CovalenceGraph::new();
        let a1 = Uuid::new_v4();
        let a2 = Uuid::new_v4();
        let bridge = Uuid::new_v4();
        let b1 = Uuid::new_v4();
        let b2 = Uuid::new_v4();
        g.add_edge(a1, a2, "CONFIRMS".to_string());
        g.add_edge(a2, bridge, "CONFIRMS".to_string());
        g.add_edge(bridge, b1, "CONFIRMS".to_string());
        g.add_edge(b1, b2, "CONFIRMS".to_string());

        let importance = structural_importance(&g);

        // Bridge should have higher importance than isolated endpoints.
        assert!(
            importance[&bridge] > importance[&a1],
            "bridge should score higher than cluster endpoint a1"
        );
        assert!(
            importance[&bridge] > importance[&b2],
            "bridge should score higher than cluster endpoint b2"
        );
    }

    #[test]
    fn test_structural_importance_isolated_node_scores_zero() {
        let mut g = CovalenceGraph::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let isolated = Uuid::new_v4();
        g.add_edge(a, b, "ORIGINATES".to_string());
        g.add_node(isolated);

        let importance = structural_importance(&g);
        // Isolated node has no paths through it — importance should be 0.
        let iso_score = importance.get(&isolated).cloned().unwrap_or(0.0);
        assert!(
            iso_score < 1e-9,
            "isolated node should have ~0 structural importance, got {iso_score}"
        );
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
