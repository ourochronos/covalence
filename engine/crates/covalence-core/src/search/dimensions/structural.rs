//! Structural search dimension — centrality-based node ranking.

use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::algorithms::pagerank;
use crate::search::SearchResult;

/// Structural importance search using PageRank.
///
/// Computes PageRank over the full in-memory graph and returns the
/// top-k nodes by score, normalized to `[0, 1]`.
pub struct StructuralDimension {
    graph: SharedGraph,
}

/// Default PageRank damping factor.
const DAMPING: f64 = 0.85;

/// Default PageRank iteration count.
const ITERATIONS: usize = 20;

impl StructuralDimension {
    /// Create a new structural search dimension.
    pub fn new(graph: SharedGraph) -> Self {
        Self { graph }
    }
}

impl SearchDimension for StructuralDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let sidecar = self.graph.read().await;

        if sidecar.node_count() == 0 {
            return Ok(Vec::new());
        }

        let scores = pagerank(sidecar.graph(), DAMPING, ITERATIONS);

        // Normalize to [0, 1].
        let max_score = scores.values().copied().fold(0.0_f64, f64::max);

        if max_score <= 0.0 {
            return Ok(Vec::new());
        }

        let mut scored: Vec<(Uuid, f64)> = scores
            .into_iter()
            .map(|(id, s)| (id, s / max_score))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(query.limit);

        let results = scored
            .into_iter()
            .enumerate()
            .map(|(i, (id, score))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "structural".to_string(),
                snippet: None,
                result_type: Some("node".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Structural
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_star_graph() -> (SharedGraph, Uuid, Vec<Uuid>) {
        let mut g = GraphSidecar::new();
        let center = Uuid::new_v4();
        g.add_node(NodeMeta {
            id: center,
            node_type: "entity".into(),
            canonical_name: "Center".into(),
            clearance_level: 0,
        })
        .unwrap();

        let mut leaves = Vec::new();
        for i in 0..4 {
            let leaf = Uuid::new_v4();
            g.add_node(NodeMeta {
                id: leaf,
                node_type: "entity".into(),
                canonical_name: format!("Leaf{i}"),
                clearance_level: 0,
            })
            .unwrap();
            g.add_edge(
                center,
                leaf,
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
            g.add_edge(
                leaf,
                center,
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
            leaves.push(leaf);
        }

        (Arc::new(RwLock::new(g)), center, leaves)
    }

    #[tokio::test]
    async fn structural_dimension_returns_ranked_nodes() {
        let (graph, center, leaves) = make_star_graph();
        let dim = StructuralDimension::new(graph);
        let query = SearchQuery {
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert_eq!(results.len(), 5); // center + 4 leaves
        // Top result should be the center (highest PageRank in star).
        assert_eq!(results[0].id, center);
        // All scores should be in [0, 1].
        for r in &results {
            assert!(r.score >= 0.0 && r.score <= 1.0);
        }
        // Leaves should all have the same score.
        let leaf_scores: Vec<f64> = results
            .iter()
            .filter(|r| leaves.contains(&r.id))
            .map(|r| r.score)
            .collect();
        assert_eq!(leaf_scores.len(), 4);
        for s in &leaf_scores[1..] {
            assert!((s - leaf_scores[0]).abs() < 1e-6);
        }
    }

    #[tokio::test]
    async fn structural_dimension_empty_graph() {
        let g = GraphSidecar::new();
        let graph = Arc::new(RwLock::new(g));
        let dim = StructuralDimension::new(graph);
        let query = SearchQuery::default();
        let results = dim.search(&query).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn structural_dimension_respects_limit() {
        let (graph, _, _) = make_star_graph();
        let dim = StructuralDimension::new(graph);
        let query = SearchQuery {
            limit: 2,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert_eq!(results.len(), 2);
    }
}
