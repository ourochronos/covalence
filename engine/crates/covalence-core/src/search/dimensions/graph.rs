//! Graph search dimension — BFS traversal from seed nodes with hop-decay.

use std::collections::HashMap;

use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::traversal::{bfs_neighborhood, hop_decay_score};
use crate::search::SearchResult;

/// Graph-based search using BFS traversal from seed nodes.
///
/// For each seed node, runs BFS on the in-memory graph sidecar and
/// scores neighbors using hop-decay: `score = 1.0 * 0.7^hops`.
/// Duplicate nodes across seeds keep the best score.
pub struct GraphDimension {
    graph: SharedGraph,
}

/// Default maximum BFS hops.
const MAX_HOPS: usize = 3;

impl GraphDimension {
    /// Create a new graph search dimension.
    pub fn new(graph: SharedGraph) -> Self {
        Self { graph }
    }
}

impl SearchDimension for GraphDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        if query.seed_nodes.is_empty() {
            return Ok(Vec::new());
        }

        let sidecar = self.graph.read().await;

        // Merge results from all seeds, keeping best score per node.
        let mut best: HashMap<Uuid, f64> = HashMap::new();

        for &seed in &query.seed_nodes {
            let neighbors = bfs_neighborhood(&sidecar, seed, MAX_HOPS, None);
            for (node_id, hops) in neighbors {
                let score = hop_decay_score(1.0, hops);
                let entry = best.entry(node_id).or_insert(0.0);
                if score > *entry {
                    *entry = score;
                }
            }
        }

        // Sort by score descending and take top-k.
        let mut scored: Vec<(Uuid, f64)> = best.into_iter().collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(query.limit);

        let results = scored
            .into_iter()
            .enumerate()
            .map(|(i, (id, score))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "graph".to_string(),
                snippet: None,
                result_type: Some("node".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_shared_graph() -> (SharedGraph, Uuid, Uuid, Uuid) {
        let mut g = GraphSidecar::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        for (id, name) in [(a, "A"), (b, "B"), (c, "C")] {
            g.add_node(NodeMeta {
                id,
                node_type: "entity".into(),
                canonical_name: name.into(),
                clearance_level: 0,
            })
            .unwrap();
        }
        g.add_edge(
            a,
            b,
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
            b,
            c,
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
        (Arc::new(RwLock::new(g)), a, b, c)
    }

    #[tokio::test]
    async fn graph_dimension_finds_neighbors() {
        let (graph, a, b, c) = make_shared_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            seed_nodes: vec![a],
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        assert!(ids.contains(&b));
        assert!(ids.contains(&c));
        // B is 1 hop, C is 2 hops — B should score higher.
        let b_result = results.iter().find(|r| r.id == b).unwrap();
        let c_result = results.iter().find(|r| r.id == c).unwrap();
        assert!(b_result.score > c_result.score);
    }

    #[tokio::test]
    async fn graph_dimension_empty_seeds() {
        let (graph, _, _, _) = make_shared_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery::default();
        let results = dim.search(&query).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn graph_dimension_respects_limit() {
        let (graph, a, _, _) = make_shared_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            seed_nodes: vec![a],
            limit: 1,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
    }
}
