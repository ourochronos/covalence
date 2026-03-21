//! Graph search dimension — BFS traversal from seed nodes with hop-decay.
//!
//! When explicit `seed_nodes` are provided in the query, traversal starts
//! from those nodes. Otherwise, seeds are auto-detected by matching the
//! query text against node canonical names in the in-memory graph sidecar
//! (case-insensitive substring match on each query term).

use std::collections::HashMap;

use uuid::Uuid;

use super::{DimensionKind, GraphView, SearchDimension, SearchQuery, extract_query_terms};
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::sidecar::{EdgeMeta, GraphSidecar};
use crate::graph::traversal::{bfs_neighborhood_pred, hop_decay_score};
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

/// Causal relationship types recognized by the `causal` graph view.
///
/// Edges with any of these `rel_type` values pass the causal filter
/// even when they lack an explicit `causal_level` annotation.
const CAUSAL_REL_TYPES: &[&str] = &[
    "CAUSED_BY",
    "ENABLED",
    "RESULTS_IN",
    "CONFIRMS",
    "CONTRADICTS",
];

/// Structural/entity relationship types recognized by the `entity`
/// (and `structural`) graph view.
const ENTITY_REL_TYPES: &[&str] = &[
    "calls",
    "uses_type",
    "contains",
    "implements",
    "extends",
    "PART_OF_COMPONENT",
];

/// Build an edge predicate for the given [`GraphView`].
///
/// The returned closure inspects an [`EdgeMeta`] reference and
/// returns `true` if the edge should be traversed. Bibliographic
/// and synthetic edges are always excluded regardless of view.
///
/// When `view_edges` is provided, uses ontology-driven edge sets.
/// Falls back to hardcoded constants otherwise.
fn view_predicate(
    view: &GraphView,
    view_edges: &Option<std::collections::HashMap<String, std::collections::HashSet<String>>>,
) -> Box<dyn Fn(&EdgeMeta) -> bool + Send + Sync> {
    // Clone the relevant edge set for the closure.
    let causal_set: std::collections::HashSet<String> = view_edges
        .as_ref()
        .and_then(|ve| ve.get("causal").cloned())
        .unwrap_or_else(|| CAUSAL_REL_TYPES.iter().map(|s| s.to_string()).collect());
    let entity_set: std::collections::HashSet<String> = view_edges
        .as_ref()
        .and_then(|ve| ve.get("entity").or(ve.get("structural")).cloned())
        .unwrap_or_else(|| ENTITY_REL_TYPES.iter().map(|s| s.to_string()).collect());
    let bib_set: std::collections::HashSet<String> =
        BIBLIOGRAPHIC_DENY.iter().map(|s| s.to_string()).collect();
    match view {
        GraphView::Causal => {
            let causal = causal_set;
            let bib = bib_set;
            Box::new(move |m: &EdgeMeta| {
                if m.is_synthetic {
                    return false;
                }
                if bib.iter().any(|d| d.eq_ignore_ascii_case(&m.rel_type)) {
                    return false;
                }
                m.causal_level.is_some()
                    || causal.iter().any(|r| r.eq_ignore_ascii_case(&m.rel_type))
            })
        }
        GraphView::Temporal => {
            let bib = bib_set;
            Box::new(move |m: &EdgeMeta| {
                if m.is_synthetic {
                    return false;
                }
                if bib.iter().any(|d| d.eq_ignore_ascii_case(&m.rel_type)) {
                    return false;
                }
                m.has_valid_from
            })
        }
        GraphView::Entity | GraphView::Structural => {
            let entity = entity_set;
            let bib = bib_set;
            Box::new(move |m: &EdgeMeta| {
                if m.is_synthetic {
                    return false;
                }
                if bib.iter().any(|d| d.eq_ignore_ascii_case(&m.rel_type)) {
                    return false;
                }
                entity.iter().any(|r| r.eq_ignore_ascii_case(&m.rel_type))
            })
        }
        GraphView::All => {
            let bib = bib_set;
            Box::new(move |m: &EdgeMeta| {
                if m.is_synthetic {
                    return false;
                }
                !bib.iter().any(|d| d.eq_ignore_ascii_case(&m.rel_type))
            })
        }
    }
}

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
    /// Configurable edge sets per view (from ontology).
    /// When set, these override the hardcoded constants.
    view_edges: Option<std::collections::HashMap<String, std::collections::HashSet<String>>>,
}

/// Default maximum BFS hops.
const MAX_HOPS: usize = 3;

/// Maximum number of auto-detected seed nodes.
const MAX_AUTO_SEEDS: usize = 10;

impl GraphDimension {
    /// Create a new graph search dimension.
    pub fn new(graph: SharedGraph) -> Self {
        Self {
            graph,
            view_edges: None,
        }
    }

    /// Set view → edge type mappings from the ontology.
    pub fn with_view_edges(
        mut self,
        view_edges: std::collections::HashMap<String, std::collections::HashSet<String>>,
    ) -> Self {
        self.view_edges = Some(view_edges);
        self
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

        // Build the edge predicate for the requested graph view.
        // When no view is set (or All), use the default
        // bibliographic-deny + skip-synthetic filter.
        let effective_view = query.graph_view.unwrap_or(GraphView::All);
        let pred = view_predicate(&effective_view, &self.view_edges);

        // Merge results from all seeds, keeping best score per node.
        let mut best: HashMap<Uuid, f64> = HashMap::new();

        // Compute the maximum semantic degree across all nodes for
        // normalization. The degree count respects the active view
        // predicate so that the bonus is view-consistent.
        let max_degree = sidecar
            .graph
            .node_indices()
            .map(|n| sidecar.graph.edges(n).filter(|e| pred(e.weight())).count())
            .max()
            .unwrap_or(1)
            .max(1) as f64;

        for &seed in &seeds {
            let neighbors = bfs_neighborhood_pred(&sidecar, seed, MAX_HOPS, &pred);
            for (node_id, hops) in neighbors {
                // Base score from hop decay.
                let base = hop_decay_score(1.0, hops);
                // Degree bonus: nodes with more connections
                // in this view get a small score boost (up to
                // 10% of base).
                let degree = sidecar
                    .node_index(node_id)
                    .map(|idx| {
                        sidecar
                            .graph
                            .edges(idx)
                            .filter(|e| pred(e.weight()))
                            .count()
                    })
                    .unwrap_or(0) as f64;
                let degree_bonus = 0.1 * base * (degree / max_degree);
                let score = base + degree_bonus;
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
                entity_class: None,
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
                has_valid_from: false,
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
                has_valid_from: false,
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
                entity_class: None,
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
                has_valid_from: false,
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
                has_valid_from: false,
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
            entity_class: None,
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
            entity_class: None,
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
            entity_class: None,
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
            entity_class: None,
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
            entity_class: None,
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
                entity_class: None,
                canonical_name: format!("node{i}"),
                clearance_level: 0,
            })
            .unwrap();
        }
        let seeds = find_seed_nodes(&g, "node");
        assert_eq!(seeds.len(), MAX_AUTO_SEEDS);
    }

    // --- Graph view tests ---

    /// Build a graph with mixed edge types for view testing.
    ///
    /// ```text
    /// A --[CAUSED_BY]--> B --[contains]--> C --[related]--> D
    ///                         (has_valid_from)
    /// ```
    fn make_view_graph() -> (SharedGraph, Uuid, Uuid, Uuid, Uuid) {
        use crate::types::causal::CausalLevel;

        let mut g = GraphSidecar::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let d = Uuid::new_v4();
        for (id, name) in [(a, "ViewA"), (b, "ViewB"), (c, "ViewC"), (d, "ViewD")] {
            g.add_node(NodeMeta {
                id,
                node_type: "entity".into(),
                entity_class: None,
                canonical_name: name.into(),
                clearance_level: 0,
            })
            .unwrap();
        }

        // A -> B: causal edge (rel_type = CAUSED_BY)
        g.add_edge(
            a,
            b,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "CAUSED_BY".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: Some(CausalLevel::Intervention),
                clearance_level: 0,
                is_synthetic: false,
                has_valid_from: false,
            },
        )
        .unwrap();

        // B -> C: structural edge (contains) with valid_from
        g.add_edge(
            b,
            c,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "contains".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
                has_valid_from: true,
            },
        )
        .unwrap();

        // C -> D: generic edge (no causal, no temporal, not
        // structural)
        g.add_edge(
            c,
            d,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "related".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
                has_valid_from: false,
            },
        )
        .unwrap();

        (Arc::new(RwLock::new(g)), a, b, c, d)
    }

    #[tokio::test]
    async fn graph_view_all_traverses_everything() {
        let (graph, a, b, c, d) = make_view_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            seed_nodes: vec![a],
            limit: 10,
            graph_view: Some(GraphView::All),
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        assert!(ids.contains(&b));
        assert!(ids.contains(&c));
        assert!(ids.contains(&d));
    }

    #[tokio::test]
    async fn graph_view_causal_only_causal_edges() {
        let (graph, a, b, _c, _d) = make_view_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            seed_nodes: vec![a],
            limit: 10,
            graph_view: Some(GraphView::Causal),
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        // Only B is reachable via the CAUSED_BY edge.
        assert!(ids.contains(&b));
        // C and D are behind non-causal edges.
        assert!(!ids.contains(&_c));
        assert!(!ids.contains(&_d));
    }

    #[tokio::test]
    async fn graph_view_temporal_only_valid_from_edges() {
        let (graph, a, _b, c, _d) = make_view_graph();
        let dim = GraphDimension::new(graph);
        // Start from B (which has the temporal edge B->C).
        // From A, only the causal edge A->B exists; the temporal
        // view won't traverse it. So start from B directly.
        let query = SearchQuery {
            seed_nodes: vec![_b],
            limit: 10,
            graph_view: Some(GraphView::Temporal),
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        // Only C is reachable from B via has_valid_from edge.
        assert!(ids.contains(&c));
        // A is behind a non-temporal edge (CAUSED_BY has
        // has_valid_from=false). D is behind a non-temporal
        // "related" edge.
        assert!(!ids.contains(&a));
        assert!(!ids.contains(&_d));
    }

    #[tokio::test]
    async fn graph_view_entity_only_structural_edges() {
        let (graph, _a, b, c, _d) = make_view_graph();
        let dim = GraphDimension::new(graph);
        // Start from B, which has "contains" -> C.
        let query = SearchQuery {
            seed_nodes: vec![b],
            limit: 10,
            graph_view: Some(GraphView::Entity),
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        // C is reachable via "contains" (structural).
        assert!(ids.contains(&c));
        // A is behind CAUSED_BY (not structural).
        assert!(!ids.contains(&_a));
        // D is behind "related" (not structural).
        assert!(!ids.contains(&_d));
    }

    #[tokio::test]
    async fn graph_view_structural_alias_for_entity() {
        let (graph, _a, b, c, _d) = make_view_graph();
        let dim = GraphDimension::new(graph);
        let query = SearchQuery {
            seed_nodes: vec![b],
            limit: 10,
            graph_view: Some(GraphView::Structural),
            ..SearchQuery::default()
        };
        let results = dim.search(&query).await.unwrap();
        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        assert!(ids.contains(&c));
        assert!(!ids.contains(&_a));
        assert!(!ids.contains(&_d));
    }

    #[tokio::test]
    async fn graph_view_none_same_as_all() {
        let (graph, a, _, _, _) = make_view_graph();
        let dim = GraphDimension::new(Arc::clone(&graph));
        // With graph_view = None (default)
        let q_none = SearchQuery {
            seed_nodes: vec![a],
            limit: 10,
            ..SearchQuery::default()
        };
        let r_none = dim.search(&q_none).await.unwrap();

        // With graph_view = All
        let dim2 = GraphDimension::new(graph);
        let q_all = SearchQuery {
            seed_nodes: vec![a],
            limit: 10,
            graph_view: Some(GraphView::All),
            ..SearchQuery::default()
        };
        let r_all = dim2.search(&q_all).await.unwrap();

        // Same result count (same traversal).
        assert_eq!(r_none.len(), r_all.len());
    }
}
