//! Structural search dimension — centrality-based node ranking.
//!
//! Combines PageRank centrality with query-text relevance so that
//! structurally important nodes that are also related to the query
//! are ranked highest. When no query text is provided, falls back
//! to pure PageRank ordering.

use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery, extract_query_terms};
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::algorithms::pagerank;
use crate::search::SearchResult;

/// Structural importance search using PageRank with query relevance.
///
/// Computes PageRank over the full in-memory graph and boosts scores
/// for nodes whose canonical name matches the query text. When query
/// text is provided, nodes that do not match any query term are
/// penalized (but not removed) so that structurally important
/// query-relevant nodes float to the top.
pub struct StructuralDimension {
    graph: SharedGraph,
}

/// Default PageRank damping factor.
const DAMPING: f64 = 0.85;

/// Default PageRank iteration count.
const ITERATIONS: usize = 20;

/// Relevance multiplier for nodes matching the query text.
const RELEVANCE_BOOST: f64 = 2.0;

/// Penalty multiplier for non-matching nodes when a query is present.
const NON_MATCH_PENALTY: f64 = 0.1;

impl StructuralDimension {
    /// Create a new structural search dimension.
    pub fn new(graph: SharedGraph) -> Self {
        Self { graph }
    }
}

/// Check whether a node's canonical name matches any query term.
///
/// Performs case-insensitive substring matching in both directions:
/// the term may be a substring of the name, or the name may be a
/// substring of a term.
fn name_matches_query(name: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let name_lower = name.to_lowercase();
    terms
        .iter()
        .any(|term| name_lower.contains(term.as_str()) || term.contains(name_lower.as_str()))
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

        // Parse query terms for relevance filtering, removing
        // stopwords and short terms to avoid spurious matches.
        let terms = extract_query_terms(&query.text);
        let has_query = !terms.is_empty();

        let mut scored: Vec<(Uuid, f64)> = scores
            .into_iter()
            .map(|(id, s)| {
                let normalized = s / max_score;
                if !has_query {
                    return (id, normalized);
                }
                // Look up canonical name from the sidecar.
                let matches = sidecar
                    .get_node(id)
                    .is_some_and(|meta| name_matches_query(&meta.canonical_name, &terms));
                let boosted = if matches {
                    normalized * RELEVANCE_BOOST
                } else {
                    normalized * NON_MATCH_PENALTY
                };
                (id, boosted)
            })
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
    async fn structural_dimension_returns_ranked_nodes_no_query() {
        let (graph, center, leaves) = make_star_graph();
        let dim = StructuralDimension::new(graph);
        // No query text — pure PageRank ordering.
        let query = SearchQuery {
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert_eq!(results.len(), 5); // center + 4 leaves
        // Top result should be the center (highest PageRank).
        assert_eq!(results[0].id, center);
        // All scores should be in [0, 1] (no boost applied).
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
    async fn structural_dimension_boosts_matching_nodes() {
        let (graph, _center, leaves) = make_star_graph();
        let dim = StructuralDimension::new(graph);
        // Query "Leaf0" should boost that leaf's score.
        let query = SearchQuery {
            text: "Leaf0".to_string(),
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        assert_eq!(results.len(), 5);

        // The matching leaf should score higher than a
        // non-matching leaf.
        let leaf0 = leaves[0];
        let leaf1 = leaves[1];
        let leaf0_score = results
            .iter()
            .find(|r| r.id == leaf0)
            .map(|r| r.score)
            .unwrap_or(0.0);
        let leaf1_score = results
            .iter()
            .find(|r| r.id == leaf1)
            .map(|r| r.score)
            .unwrap_or(0.0);
        assert!(
            leaf0_score > leaf1_score,
            "matching leaf should score higher"
        );
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

    #[test]
    fn name_matches_query_basic() {
        let terms = vec!["rust".to_string(), "language".to_string()];
        assert!(name_matches_query("Rust", &terms));
        assert!(name_matches_query("rust programming", &terms));
        assert!(!name_matches_query("Python", &terms));
    }

    #[test]
    fn name_matches_query_empty_terms() {
        assert!(!name_matches_query("Rust", &[]));
    }

    #[test]
    fn name_matches_query_reverse_substring() {
        // Short canonical name is substring of a longer query term.
        let terms = vec!["rustlang".to_string()];
        assert!(name_matches_query("Rust", &terms));
    }

    #[tokio::test]
    async fn structural_dimension_filters_stopwords() {
        let (graph, _center, _leaves) = make_star_graph();
        let dim = StructuralDimension::new(graph);
        // "the" and "for" are stopwords; without filtering, "the"
        // would match "Center" substring and boost it. With
        // filtering, no terms survive, so no boost is applied.
        let query_stopwords_only = SearchQuery {
            text: "the for".to_string(),
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query_stopwords_only).await.unwrap();
        // All nodes should have the same relative ordering as pure
        // PageRank (no boost applied because terms were filtered).
        // Center still ranks #1 (pure PageRank) when only stopwords
        // are in the query.
        assert_eq!(results[0].id, _center);
    }

    #[tokio::test]
    async fn structural_dimension_filters_short_terms() {
        let (graph, center, _) = make_star_graph();
        let dim = StructuralDimension::new(graph);
        // "in" (2 chars) should be filtered out.
        let query = SearchQuery {
            text: "in".to_string(),
            limit: 10,
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        // Without short-term filtering, "in" would match "Center"
        // (no match). With filtering, falls back to pure PageRank.
        assert_eq!(results[0].id, center);
        // All scores should be in [0, 1] (no boost applied).
        for r in &results {
            assert!(r.score >= 0.0 && r.score <= 1.0);
        }
    }
}
