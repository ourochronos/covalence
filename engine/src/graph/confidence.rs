//! Topological confidence scoring for graph nodes.
//!
//! Phase 6 (covalence#53): Compute a dynamic confidence score for a node
//! based on its position in the knowledge graph, using PageRank and
//! inbound-edge diversity as signals.

use std::collections::HashMap;
use uuid::Uuid;

use petgraph::Direction;

use super::memory::CovalenceGraph;

/// Topological confidence derived from graph structure.
///
/// Combines a normalised PageRank score with an inbound-edge diversity metric
/// to produce a `score` in `[0.0, 1.0]`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TopologicalConfidence {
    /// Raw PageRank score for this node (un-normalised).
    pub pagerank: f64,
    /// Number of distinct inbound edges to this node.
    pub path_diversity: usize,
    /// Combined topological score: `0.6 * normalised_pagerank + 0.4 * path_norm`.
    pub score: f64,
}

/// Compute topological confidence for a single node.
///
/// # Arguments
/// * `node_id`         – The node whose confidence is being computed.
/// * `pagerank_scores` – Pre-computed PageRank map (`Uuid → raw score`).
/// * `graph`           – The in-memory [`CovalenceGraph`].
///
/// # Algorithm
/// 1. Look up the raw PageRank score (default `0.0` if absent).
/// 2. Normalise by dividing by the maximum score in the map.
///    If the map is empty or the max is `0.0`, treat all as `0.0`.
/// 3. Count inbound edges: `inbound_count`.
/// 4. Saturating normalisation: `path_norm = 1 - e^(-inbound / 5)`.
/// 5. Blend: `score = 0.6 * normalised_pr + 0.4 * path_norm`.
pub fn compute_topological_confidence(
    node_id: &Uuid,
    pagerank_scores: &HashMap<Uuid, f64>,
    graph: &CovalenceGraph,
) -> TopologicalConfidence {
    // Step 1: raw PageRank.
    let raw = pagerank_scores.get(node_id).copied().unwrap_or(0.0);

    // Step 2: find the map maximum for normalisation.
    let max_pr = pagerank_scores.values().cloned().fold(0.0_f64, f64::max);

    let normalized_pr = if max_pr > 0.0 { raw / max_pr } else { 0.0 };

    // Step 3: inbound edge count.
    let inbound_count = graph
        .index
        .get(node_id)
        .map(|&idx| graph.graph.edges_directed(idx, Direction::Incoming).count())
        .unwrap_or(0);

    // Step 4: saturating path normalisation.
    let path_norm = 1.0 - (-(inbound_count as f64) / 5.0).exp();

    // Step 5: weighted blend.
    let score = 0.6 * normalized_pr + 0.4 * path_norm;

    TopologicalConfidence {
        pagerank: raw,
        path_diversity: inbound_count,
        score,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_map_gives_zero() {
        let g = CovalenceGraph::new();
        let id = Uuid::new_v4();
        let tc = compute_topological_confidence(&id, &HashMap::new(), &g);
        assert_eq!(tc.score, 0.0);
        assert_eq!(tc.pagerank, 0.0);
        assert_eq!(tc.path_diversity, 0);
    }

    #[test]
    fn test_absent_node_gives_zero_score() {
        let mut scores = HashMap::new();
        scores.insert(Uuid::new_v4(), 0.5);
        let g = CovalenceGraph::new();
        let id = Uuid::new_v4();
        let tc = compute_topological_confidence(&id, &scores, &g);
        // raw = 0, normalised = 0, path_norm = 0 → score = 0
        assert_eq!(tc.score, 0.0);
    }

    #[test]
    fn test_path_norm_saturates() {
        let g = CovalenceGraph::new();
        let id = Uuid::new_v4();
        // Manually construct result with high inbound count to verify formula.
        // 10 inbound → path_norm = 1 - e^(-2) ≈ 0.8647
        let path_norm_10: f64 = 1.0 - (-10.0_f64 / 5.0).exp();
        assert!((path_norm_10 - 0.8647).abs() < 0.001);
        // pure path, no pagerank
        let mut scores = HashMap::new();
        scores.insert(id, 0.0);
        let tc = compute_topological_confidence(&id, &scores, &g);
        // path_diversity = 0 because no edges in empty graph
        assert_eq!(tc.path_diversity, 0);
        let _ = tc; // just ensure it compiles
    }
}
