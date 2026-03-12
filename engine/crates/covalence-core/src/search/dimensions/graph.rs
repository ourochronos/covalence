//! Graph search dimension — BFS traversal from seed nodes with hop-decay.
//!
//! When explicit `seed_nodes` are provided in the query, traversal starts
//! from those nodes. Otherwise, seeds are auto-detected by matching the
//! query text against node canonical names in the in-memory graph sidecar
//! (case-insensitive substring match on each query term).

use std::collections::HashMap;

use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery, extract_query_terms};
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::sidecar::GraphSidecar;
use crate::graph::traversal::{bfs_neighborhood_full, hop_decay_score};
use crate::search::SearchResult;

/// Edge types to skip during graph search traversal.
///
/// Bibliographic relationships dominate the edge distribution
/// (thousands of `authored`/`published_in` vs. hundreds of
/// architectural edges). Traversing them causes BFS to expand
/// into academic-paper neighborhoods rather than following
/// system-design structure.
const BIBLIOGRAPHIC_DENY: &[&str] = &[
    "authored",
    "published_in",
    "works_at",
    "evaluated_on",
    "trained_on",
    "uses_dataset",
    "created_by",
    "edited_by",
];

/// Graph-based search using BFS traversal from seed nodes.
///
/// For each seed node, runs BFS on the in-memory graph sidecar and
/// scores neighbors using hop-decay: `score = 1.0 * 0.7^hops`.
/// Duplicate nodes across seeds keep the best score.
///
/// When no explicit seed nodes are provided, the dimension
/// auto-detects seeds by matching query terms against node
/// canonical names (case-insensitive substring match).
pub struct GraphDimension {
    graph: SharedGraph,
}

/// Default maximum BFS hops.
const MAX_HOPS: usize = 3;

/// Maximum number of auto-detected seed nodes.
const MAX_AUTO_SEEDS: usize = 10;

impl GraphDimension {
    /// Create a new graph search dimension.
    pub fn new(graph: SharedGraph) -> Self {
        Self { graph }
    }
}

/// Find seed nodes by matching query text against canonical names.
///
/// Uses `extract_query_terms` to split the query, filter stopwords
/// and short terms, then checks each node's `canonical_name` for a
/// case-insensitive substring match. Returns up to `MAX_AUTO_SEEDS`
/// matching node UUIDs, ranked by match count.
fn find_seed_nodes(sidecar: &GraphSidecar, query_text: &str) -> Vec<Uuid> {
    if query_text.is_empty() {
        return Vec::new();
    }

    let terms = extract_query_terms(query_text);

    if terms.is_empty() {
        return Vec::new();
    }

    // Score each node by how many query terms match. Nodes matching
    // more terms are better seeds (e.g., "Search Service" matching
    // both "search" and "service" ranks above nodes matching only
    // "search").
    let mut scored: Vec<(Uuid, usize)> = Vec::new();

    for node_idx in sidecar.graph.node_indices() {
        let meta = &sidecar.graph[node_idx];
        let name_lower = meta.canonical_name.to_lowercase();

        let match_count = terms
            .iter()
            .filter(|term| name_lower.contains(term.as_str()) || term.contains(name_lower.as_str()))
            .count();

        if match_count > 0 {
            scored.push((meta.id, match_count));
        }
    }

    // Sort by match count descending (best matches first).
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.truncate(MAX_AUTO_SEEDS);
    scored.into_iter().map(|(id, _)| id).collect()
}

impl SearchDimension for GraphDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let sidecar = self.graph.read().await;

        // Use explicit seeds if provided, otherwise auto-detect.
        let seeds = if query.seed_nodes.is_empty() {
            find_seed_nodes(&sidecar, &query.text)
        } else {
            query.seed_nodes.clone()
        };

        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        // Merge results from all seeds, keeping best score per node.
        let mut best: HashMap<Uuid, f64> = HashMap::new();

        for &seed in &seeds {
            let neighbors = bfs_neighborhood_full(
                &sidecar,
                seed,
                MAX_HOPS,
                None,
                true,
                Some(BIBLIOGRAPHIC_DENY),
            );
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
                is_synthetic: false,
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
                is_synthetic: false,
            },
        )
        .unwrap();
        (Arc::new(RwLock::new(g)), a, b, c)
    }

    /// Build a graph with meaningful names for auto-seed testing.
    /// Rust -> Tokio -> Async
    fn make_named_graph() -> (SharedGraph, Uuid, Uuid, Uuid) {
        let mut g = GraphSidecar::new();
        let rust = Uuid::new_v4();
        let tokio = Uuid::new_v4();
        let async_id = Uuid::new_v4();
        for (id, name) in [
            (rust, "Rust"),
            (tokio, "Tokio"),
            (async_id, "Async Runtime"),
        ] {
            g.add_node(NodeMeta {
                id,
                node_type: "entity".into(),
                canonical_name: name.into(),
                clearance_level: 0,
            })
            .unwrap();
        }
        g.add_edge(
            rust,
            tokio,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "uses".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
            },
        )
        .unwrap();
        g.add_edge(
            tokio,
            async_id,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "implements".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
            },
        )
        .unwrap();
        (Arc::new(RwLock::new(g)), rust, tokio, async_id)
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
    async fn graph_dimension_empty_seeds_and_empty_text() {
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

    #[tokio::test]
    async fn auto_detect_seeds_from_query_text() {
        let (graph, _rust, tokio_id, async_id) = make_named_graph();
        let dim = GraphDimension::new(graph);
        // Query text "rust" should match the "Rust" node,
        // then BFS should find Tokio (1 hop) and Async (2 hops).
        let query = SearchQuery {
            text: "rust".to_string(),
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert!(!results.is_empty());
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        assert!(ids.contains(&tokio_id));
        assert!(ids.contains(&async_id));
    }

    #[tokio::test]
    async fn auto_detect_no_match_returns_empty() {
        let (graph, _, _, _) = make_named_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            text: "nonexistent".to_string(),
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn auto_detect_case_insensitive() {
        let (graph, _rust, tokio_id, _) = make_named_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            text: "RUST language".to_string(),
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        // "rust" in "RUST language" matches "Rust" node.
        assert!(ids.contains(&tokio_id));
    }

    #[test]
    fn find_seed_nodes_empty_query() {
        let g = GraphSidecar::new();
        assert!(find_seed_nodes(&g, "").is_empty());
    }

    #[test]
    fn find_seed_nodes_matches_substring() {
        let mut g = GraphSidecar::new();
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            canonical_name: "Knowledge Graph".into(),
            clearance_level: 0,
        })
        .unwrap();
        let seeds = find_seed_nodes(&g, "knowledge");
        assert_eq!(seeds, vec![id]);
    }

    #[test]
    fn find_seed_nodes_reverse_substring() {
        let mut g = GraphSidecar::new();
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            canonical_name: "ML".into(),
            clearance_level: 0,
        })
        .unwrap();
        // Short canonical names (< MIN_SEED_TERM_LEN) can still be
        // matched via the reverse check (term contains name).
        // The term "machine_learning" contains "ml"? No — we need
        // a term that actually contains the 2-char name as a
        // substring. Use a name >= MIN_SEED_TERM_LEN instead.
        let id2 = Uuid::new_v4();
        g.add_node(NodeMeta {
            id: id2,
            node_type: "entity".into(),
            canonical_name: "NER".into(),
            clearance_level: 0,
        })
        .unwrap();
        // "ner" (3 chars) passes min length and matches a term
        // that contains it.
        let seeds = find_seed_nodes(&g, "gliner model");
        assert_eq!(seeds, vec![id2]);
    }

    #[test]
    fn find_seed_nodes_filters_stopwords() {
        let mut g = GraphSidecar::new();
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            canonical_name: "Information Retrieval".into(),
            clearance_level: 0,
        })
        .unwrap();
        // "from" and "the" are stopwords; only "retrieval" should match.
        let seeds = find_seed_nodes(&g, "from the retrieval");
        assert_eq!(seeds, vec![id]);
        // Pure stopwords should return nothing.
        let seeds = find_seed_nodes(&g, "from the");
        assert!(seeds.is_empty());
    }

    #[test]
    fn find_seed_nodes_filters_short_terms() {
        let mut g = GraphSidecar::new();
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            canonical_name: "Mining Algorithm".into(),
            clearance_level: 0,
        })
        .unwrap();
        // "in" is too short (< MIN_SEED_TERM_LEN=3), shouldn't match
        // "Mining" via substring. "mining" (6 chars) should.
        let seeds = find_seed_nodes(&g, "in");
        assert!(seeds.is_empty());
        let seeds = find_seed_nodes(&g, "mining");
        assert_eq!(seeds, vec![id]);
    }

    #[test]
    fn find_seed_nodes_respects_max_limit() {
        let mut g = GraphSidecar::new();
        for i in 0..(MAX_AUTO_SEEDS + 5) {
            g.add_node(NodeMeta {
                id: Uuid::new_v4(),
                node_type: "entity".into(),
                canonical_name: format!("node{i}"),
                clearance_level: 0,
            })
            .unwrap();
        }
        let seeds = find_seed_nodes(&g, "node");
        assert_eq!(seeds.len(), MAX_AUTO_SEEDS);
    }
}
