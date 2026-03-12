//! Graph algorithms — PageRank, TrustRank, spreading activation, structural importance.

use std::collections::HashMap;

use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::EdgeRef;
use uuid::Uuid;

use super::sidecar::{EdgeMeta, NodeMeta};

/// Compute PageRank scores for all nodes in the graph.
///
/// Uses the power iteration method with the given damping factor and
/// iteration count. Returns a map of node UUID to PageRank score.
pub fn pagerank(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    damping: f64,
    iterations: usize,
) -> HashMap<Uuid, f64> {
    let n = graph.node_count();
    if n == 0 {
        return HashMap::new();
    }

    let n_f64 = n as f64;
    let initial = 1.0 / n_f64;
    let mut scores: HashMap<NodeIndex, f64> =
        graph.node_indices().map(|idx| (idx, initial)).collect();

    for _ in 0..iterations {
        let mut new_scores: HashMap<NodeIndex, f64> = graph
            .node_indices()
            .map(|idx| (idx, (1.0 - damping) / n_f64))
            .collect();

        // Distribute dangling node mass
        let dangling_sum: f64 = graph
            .node_indices()
            .filter(|&idx| graph.edges(idx).next().is_none())
            .map(|idx| scores[&idx])
            .sum();

        for idx in graph.node_indices() {
            if let Some(s) = new_scores.get_mut(&idx) {
                *s += damping * dangling_sum / n_f64;
            }
        }

        // Distribute rank through edges
        for idx in graph.node_indices() {
            let out_degree = graph.edges(idx).count();
            if out_degree > 0 {
                let share = damping * scores[&idx] / out_degree as f64;
                for edge in graph.edges(idx) {
                    if let Some(s) = new_scores.get_mut(&edge.target()) {
                        *s += share;
                    }
                }
            }
        }

        scores = new_scores;
    }

    // Map NodeIndex -> Uuid
    scores
        .into_iter()
        .map(|(idx, score)| (graph[idx].id, score))
        .collect()
}

/// Compute Personalized PageRank from a set of seed nodes.
///
/// The teleportation step biases towards seed nodes instead of uniform
/// distribution. Seed nodes receive equal teleportation probability.
pub fn personalized_pagerank(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    seeds: &[Uuid],
    damping: f64,
    iterations: usize,
) -> HashMap<Uuid, f64> {
    let n = graph.node_count();
    if n == 0 || seeds.is_empty() {
        return HashMap::new();
    }

    // Build seed set of NodeIndex
    let seed_indices: Vec<NodeIndex> = seeds
        .iter()
        .filter_map(|id| graph.node_indices().find(|&idx| graph[idx].id == *id))
        .collect();

    if seed_indices.is_empty() {
        return HashMap::new();
    }

    let n_f64 = n as f64;
    let seed_weight = 1.0 / seed_indices.len() as f64;
    let initial = 1.0 / n_f64;
    let mut scores: HashMap<NodeIndex, f64> =
        graph.node_indices().map(|idx| (idx, initial)).collect();

    for _ in 0..iterations {
        // Teleportation favors seed nodes
        let mut new_scores: HashMap<NodeIndex, f64> = graph
            .node_indices()
            .map(|idx| {
                let teleport = if seed_indices.contains(&idx) {
                    (1.0 - damping) * seed_weight
                } else {
                    0.0
                };
                (idx, teleport)
            })
            .collect();

        // Dangling nodes
        let dangling_sum: f64 = graph
            .node_indices()
            .filter(|&idx| graph.edges(idx).next().is_none())
            .map(|idx| scores[&idx])
            .sum();

        for &sidx in &seed_indices {
            if let Some(s) = new_scores.get_mut(&sidx) {
                *s += damping * dangling_sum * seed_weight;
            }
        }

        // Edge distribution
        for idx in graph.node_indices() {
            let out_degree = graph.edges(idx).count();
            if out_degree > 0 {
                let share = damping * scores[&idx] / out_degree as f64;
                for edge in graph.edges(idx) {
                    if let Some(s) = new_scores.get_mut(&edge.target()) {
                        *s += share;
                    }
                }
            }
        }

        scores = new_scores;
    }

    scores
        .into_iter()
        .map(|(idx, score)| (graph[idx].id, score))
        .collect()
}

/// Compute global trust scores from a set of verified seed nodes.
///
/// Trust flows through edges, weighted by edge confidence. Uses a modified
/// PageRank where teleportation biases towards seed nodes with their
/// assigned trust weights.
pub fn trust_rank(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    seed_nodes: &[(Uuid, f64)],
    damping: f64,
    iterations: usize,
) -> HashMap<Uuid, f64> {
    let n = graph.node_count();
    if n == 0 || seed_nodes.is_empty() {
        return HashMap::new();
    }

    // Build seed map: NodeIndex -> trust weight
    let mut seed_map: HashMap<NodeIndex, f64> = HashMap::new();
    for (id, weight) in seed_nodes {
        if let Some(idx) = graph.node_indices().find(|&idx| graph[idx].id == *id) {
            seed_map.insert(idx, *weight);
        }
    }

    if seed_map.is_empty() {
        return HashMap::new();
    }

    // Normalize seed weights
    let total_seed_weight: f64 = seed_map.values().sum();
    if total_seed_weight <= 0.0 {
        tracing::warn!(
            "trust_rank: seed weights sum to {total_seed_weight}, \
             returning empty scores"
        );
        return HashMap::new();
    }
    let seed_normalized: HashMap<NodeIndex, f64> = seed_map
        .iter()
        .map(|(&idx, &w)| (idx, w / total_seed_weight))
        .collect();

    let initial = 1.0 / n as f64;
    let mut scores: HashMap<NodeIndex, f64> =
        graph.node_indices().map(|idx| (idx, initial)).collect();

    for _ in 0..iterations {
        // Teleportation to seeds proportional to their trust weight
        let mut new_scores: HashMap<NodeIndex, f64> = graph
            .node_indices()
            .map(|idx| {
                let teleport = seed_normalized.get(&idx).copied().unwrap_or(0.0) * (1.0 - damping);
                (idx, teleport)
            })
            .collect();

        // Dangling node mass goes to seeds
        let dangling_sum: f64 = graph
            .node_indices()
            .filter(|&idx| graph.edges(idx).next().is_none())
            .map(|idx| scores[&idx])
            .sum();

        for (&sidx, &sw) in &seed_normalized {
            if let Some(s) = new_scores.get_mut(&sidx) {
                *s += damping * dangling_sum * sw;
            }
        }

        // Trust flows through edges weighted by confidence
        for idx in graph.node_indices() {
            let edges: Vec<_> = graph.edges(idx).collect();
            if edges.is_empty() {
                continue;
            }
            let total_confidence: f64 = edges
                .iter()
                .map(|e| graph[e.id()].effective_confidence())
                .sum();
            if total_confidence <= 0.0 {
                continue;
            }
            for edge in &edges {
                let edge_share = damping * scores[&idx] * graph[edge.id()].effective_confidence()
                    / total_confidence;
                if let Some(s) = new_scores.get_mut(&edge.target()) {
                    *s += edge_share;
                }
            }
        }

        scores = new_scores;
    }

    scores
        .into_iter()
        .map(|(idx, score)| (graph[idx].id, score))
        .collect()
}

/// ACT-R inspired spreading activation for query expansion.
///
/// Activation flows from seed nodes through edges, decaying with distance.
/// Only nodes above `threshold` are returned.
pub fn spreading_activation(
    graph: &StableDiGraph<NodeMeta, EdgeMeta>,
    seeds: &[(Uuid, f64)],
    decay: f64,
    threshold: f64,
) -> HashMap<Uuid, f64> {
    if seeds.is_empty() {
        return HashMap::new();
    }

    let mut activation: HashMap<NodeIndex, f64> = HashMap::new();

    // Initialize seeds
    for (id, energy) in seeds {
        if let Some(idx) = graph.node_indices().find(|&idx| graph[idx].id == *id) {
            *activation.entry(idx).or_insert(0.0) += energy;
        }
    }

    // Iteratively spread activation until no new activations above threshold
    let mut frontier: Vec<(NodeIndex, f64)> = activation.iter().map(|(&k, &v)| (k, v)).collect();

    while !frontier.is_empty() {
        let mut next_frontier = Vec::new();

        for (idx, energy) in &frontier {
            let out_edges: Vec<_> = graph.edges(*idx).collect();
            if out_edges.is_empty() {
                continue;
            }

            for edge in &out_edges {
                let spread = energy * decay * graph[edge.id()].effective_weight();
                if spread < threshold {
                    continue;
                }

                let target = edge.target();
                let current = activation.get(&target).copied().unwrap_or(0.0);
                if spread > current {
                    activation.insert(target, spread);
                    next_frontier.push((target, spread));
                }
            }
        }

        frontier = next_frontier;
    }

    activation
        .into_iter()
        .filter(|&(_, v)| v >= threshold)
        .map(|(idx, score)| (graph[idx].id, score))
        .collect()
}

/// Compute structural importance (betweenness centrality) for all nodes.
///
/// Nodes critical to graph connectivity score higher. Used by BMR
/// forgetting decisions — high-importance nodes are archived, not pruned.
///
/// Uses Brandes' algorithm for O(V*E) betweenness centrality.
pub fn structural_importance(graph: &StableDiGraph<NodeMeta, EdgeMeta>) -> HashMap<Uuid, f64> {
    let n = graph.node_count();
    if n == 0 {
        return HashMap::new();
    }

    let mut centrality: HashMap<NodeIndex, f64> =
        graph.node_indices().map(|idx| (idx, 0.0)).collect();

    // Brandes' algorithm
    for s in graph.node_indices() {
        let mut stack: Vec<NodeIndex> = Vec::new();
        let mut predecessors: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
        let mut sigma: HashMap<NodeIndex, f64> =
            graph.node_indices().map(|idx| (idx, 0.0)).collect();
        if let Some(s_sigma) = sigma.get_mut(&s) {
            *s_sigma = 1.0;
        }
        let mut dist: HashMap<NodeIndex, i64> = graph.node_indices().map(|idx| (idx, -1)).collect();
        if let Some(s_dist) = dist.get_mut(&s) {
            *s_dist = 0;
        }

        let mut queue = std::collections::VecDeque::new();
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            for edge in graph.edges(v) {
                let w = edge.target();
                let dist_v = dist[&v];
                let sigma_v = sigma[&v];
                if dist[&w] < 0 {
                    queue.push_back(w);
                    if let Some(d) = dist.get_mut(&w) {
                        *d = dist_v + 1;
                    }
                }
                if dist[&w] == dist_v + 1 {
                    if let Some(s) = sigma.get_mut(&w) {
                        *s += sigma_v;
                    }
                    predecessors.entry(w).or_default().push(v);
                }
            }
        }

        let mut delta: HashMap<NodeIndex, f64> =
            graph.node_indices().map(|idx| (idx, 0.0)).collect();
        while let Some(w) = stack.pop() {
            let sigma_w = sigma[&w];
            let delta_w = delta[&w];
            // Guard against division by zero (should not happen since
            // only BFS-reachable nodes with sigma > 0 are on the stack,
            // but defend against degenerate graph structures).
            if sigma_w > 0.0 {
                if let Some(preds) = predecessors.get(&w) {
                    for &v in preds {
                        let d = (sigma[&v] / sigma_w) * (1.0 + delta_w);
                        if let Some(dv) = delta.get_mut(&v) {
                            *dv += d;
                        }
                    }
                }
            }
            if w != s {
                if let Some(c) = centrality.get_mut(&w) {
                    *c += delta_w;
                }
            }
        }
    }

    // Normalize by (n-1)*(n-2) for directed graphs
    let norm = if n > 2 {
        1.0 / ((n - 1) * (n - 2)) as f64
    } else {
        1.0
    };

    centrality
        .into_iter()
        .map(|(idx, score)| (graph[idx].id, score * norm))
        .collect()
}

/// Compute topological confidence from PageRank scores and path diversity.
///
/// `confidence(node) = alpha * normalized_pagerank + beta * path_diversity`
///
/// Where alpha = 0.6, beta = 0.4 (tunable, from covalence).
pub fn topological_confidence(
    pagerank_scores: &HashMap<Uuid, f64>,
    path_diversity: &HashMap<Uuid, f64>,
) -> HashMap<Uuid, f64> {
    const ALPHA: f64 = 0.6;
    const BETA: f64 = 0.4;

    // Normalize PageRank scores to [0, 1]
    let max_pr = pagerank_scores.values().copied().fold(0.0_f64, f64::max);
    let max_pd = path_diversity.values().copied().fold(0.0_f64, f64::max);

    let all_ids: std::collections::HashSet<&Uuid> = pagerank_scores
        .keys()
        .chain(path_diversity.keys())
        .collect();

    all_ids
        .into_iter()
        .map(|id| {
            let pr = if max_pr > 0.0 {
                pagerank_scores.get(id).copied().unwrap_or(0.0) / max_pr
            } else {
                0.0
            };
            let pd = if max_pd > 0.0 {
                path_diversity.get(id).copied().unwrap_or(0.0) / max_pd
            } else {
                0.0
            };
            (*id, ALPHA * pr + BETA * pd)
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

    /// Build: A <-> B <-> C (bidirectional)
    fn triangle_graph() -> (GraphSidecar, Uuid, Uuid, Uuid) {
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
        (g, a, b, c)
    }

    #[test]
    fn pagerank_uniform_triangle() {
        let (g, a, b, c) = triangle_graph();
        let scores = pagerank(g.graph(), 0.85, 50);
        // All nodes should have roughly equal PageRank in a symmetric graph
        let sa = scores[&a];
        let sb = scores[&b];
        let sc = scores[&c];
        assert!((sa - sb).abs() < 0.01);
        assert!((sb - sc).abs() < 0.01);
    }

    #[test]
    fn personalized_pagerank_biases_seeds() {
        let (g, a, b, _c) = triangle_graph();
        let scores = personalized_pagerank(g.graph(), &[a], 0.85, 50);
        // Seed node A should have higher score than others
        assert!(scores[&a] > scores[&b]);
    }

    #[test]
    fn trust_rank_biases_seeds() {
        let (g, a, b, c) = triangle_graph();
        let scores = trust_rank(g.graph(), &[(a, 1.0)], 0.85, 50);
        assert!(scores.contains_key(&a));
        assert!(scores.contains_key(&b));
        assert!(scores.contains_key(&c));
        // Seed node should have highest trust
        assert!(scores[&a] >= scores[&b]);
    }

    #[test]
    fn spreading_activation_basic() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        let c = add_node(&mut g, "C");
        add_edge(&mut g, a, b);
        add_edge(&mut g, b, c);

        let result = spreading_activation(g.graph(), &[(a, 1.0)], 0.7, 0.01);
        assert!(result.contains_key(&a));
        assert!(result.contains_key(&b));
        // C should be reachable but with lower activation
        if let Some(&ac) = result.get(&c) {
            assert!(ac < result[&b]);
        }
    }

    #[test]
    fn structural_importance_hub_scores_higher() {
        // Star graph: center connected to 4 leaves
        let mut g = GraphSidecar::new();
        let center = add_node(&mut g, "Center");
        let mut leaves = Vec::new();
        for i in 0..4 {
            let leaf = add_node(&mut g, &format!("Leaf{i}"));
            add_edge(&mut g, center, leaf);
            add_edge(&mut g, leaf, center);
            leaves.push(leaf);
        }
        // Connect leaf0 -> leaf1 through center
        let scores = structural_importance(g.graph());
        // Center should have highest betweenness
        let center_score = scores[&center];
        for leaf in &leaves {
            assert!(center_score >= scores[leaf]);
        }
    }

    #[test]
    fn topological_confidence_blending() {
        let id = Uuid::new_v4();
        let mut pr = HashMap::new();
        pr.insert(id, 1.0);
        let mut pd = HashMap::new();
        pd.insert(id, 0.5);

        let tc = topological_confidence(&pr, &pd);
        // alpha=0.6 * 1.0 + beta=0.4 * 1.0 = 1.0 (both normalized to max)
        assert!((tc[&id] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn pagerank_empty_graph() {
        let g: StableDiGraph<NodeMeta, EdgeMeta> = StableDiGraph::new();
        let scores = pagerank(&g, 0.85, 10);
        assert!(scores.is_empty());
    }

    #[test]
    fn trust_rank_zero_weight_seeds() {
        let (g, a, _b, _c) = triangle_graph();
        let seeds = vec![(a, 0.0)];
        let scores = trust_rank(g.graph(), &seeds, 0.85, 10);
        // Zero-weight seeds should return empty scores
        assert!(scores.is_empty());
    }

    #[test]
    fn trust_rank_negative_weight_seeds() {
        let (g, a, _b, _c) = triangle_graph();
        let seeds = vec![(a, -1.0)];
        let scores = trust_rank(g.graph(), &seeds, 0.85, 10);
        // Negative total weight should return empty scores
        assert!(scores.is_empty());
    }
}
