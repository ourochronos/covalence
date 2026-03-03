//! Search service — orchestrates the four-dimensional cascade (SPEC §7.2).
//!
//! Step 1 (parallel): Lexical + Vector via tokio::try_join!
//! Step 2 (sequential): Graph from candidate anchors
//! Step 3 (sequential): Structural from same anchor set (covalence#52)
//! Step 4: Score fusion (weighted dimensional + confidence + freshness)
//!
//! ## Search Modes
//!
//! * [`SearchMode::Standard`] (default) — flat search over all node types,
//!   current behaviour unchanged.
//! * [`SearchMode::Hierarchical`] — two-phase retrieval:
//!   1. Search articles only to surface the best organised summaries.
//!   2. Expand the top-N articles by following provenance edges
//!      (ORIGINATES / COMPILED_FROM / CONFIRMS) and pulling in their linked
//!      source nodes at a discounted score (`article_score * 0.8`).
//!
//!   The combined result set — articles first, then expanded sources — is
//!   truncated to `req.limit`.  Expanded sources carry an `expanded_from`
//!   field that identifies the parent article.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::worker::llm::LlmClient;

use crate::graph::algorithms::pagerank;
use crate::graph::{TopologicalConfidence, compute_topological_confidence};
use crate::models::SearchIntent;
use crate::search::dimension::{DimensionAdaptor, DimensionQuery};
use crate::search::graph::GraphAdaptor;
use crate::search::lexical::LexicalAdaptor;
use crate::search::structural::StructuralAdaptor;
use crate::search::vector::VectorAdaptor;

// ─── Search Mode ──────────────────────────────────────────────────────────────

/// Controls how the search engine retrieves and returns results.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Default, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Flat search over all node types (current behaviour). This is the
    /// default when `mode` is absent from the request.
    #[default]
    Standard,
    /// Two-phase hierarchical retrieval: articles first, then their linked
    /// sources expanded via provenance edges.
    Hierarchical,
    /// Article-free live synthesis (covalence#59): runs the 4-D search over
    /// raw sources, feeds the top-N results to the LLM, and returns a
    /// synthesised answer with inline provenance citations.
    ///
    /// Requires `COVALENCE_LIVE_SYNTHESIS=true`.
    Synthesis,
}

// ─── Search Strategy ──────────────────────────────────────────────────────────

/// Adaptive fusion strategy — controls the relative weighting of the four
/// search dimensions for different query types.
///
/// When [`SearchRequest::weights`] is explicitly provided it always takes
/// precedence; `strategy` only applies when no explicit weights are given.
#[derive(Debug, Clone, Deserialize, Serialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    /// Balanced across all dimensions: vector=0.55, lexical=0.20, graph=0.10,
    /// structural=0.15.
    #[default]
    Balanced,
    /// Lexical-heavy for factual/specific lookups: vector=0.30, lexical=0.45,
    /// graph=0.10, structural=0.15.  Prioritises exact-term matches.
    Precise,
    /// Vector-heavy for conceptual/broad queries: vector=0.65, lexical=0.10,
    /// graph=0.10, structural=0.15.  Prioritises semantic similarity.
    Exploratory,
    /// Graph-heavy for relationship queries: vector=0.25, lexical=0.10,
    /// graph=0.45, structural=0.20.  Prioritises graph-neighbourhood signals.
    Graph,
    /// Structural-heavy for topology queries: vector=0.25, lexical=0.10,
    /// graph=0.10, structural=0.55.  Discovers nodes with similar graph
    /// structure even without semantic or lexical overlap.
    Structural,
}

// ─── Request / Response types ─────────────────────────────────────────────────

/// Request body for POST /search.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    #[schema(value_type = Option<Vec<f32>>)]
    pub embedding: Option<Vec<f32>>,
    #[serde(default)]
    pub intent: Option<SearchIntent>,
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub session_id: Option<Uuid>,
    #[serde(default)]
    pub node_types: Option<Vec<String>>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub weights: Option<WeightsInput>,
    /// Search mode — defaults to [`SearchMode::Standard`].
    #[serde(default)]
    pub mode: Option<SearchMode>,
    /// Recency bias factor (0.0–1.0). Higher values favor newer content.
    /// At 0.0 (default), freshness gets 5% weight (current behavior).
    /// At 1.0, freshness gets 40% weight (strongly favor recent).
    #[serde(default)]
    pub recency_bias: Option<f64>,
    /// Optional domain-path filter. When set, only nodes whose `domain_path`
    /// array shares at least one element with this list are returned.
    /// Nodes with an empty or NULL `domain_path` are excluded when filtering.
    #[serde(default)]
    pub domain_path: Option<Vec<String>>,
    /// Search strategy — adjusts dimension weights for different query types.
    /// Overrides `weights` when set to a non-`Balanced` value.
    /// When both `weights` and `strategy` are provided, `weights` wins.
    #[serde(default)]
    pub strategy: Option<SearchStrategy>,
    /// Maximum graph traversal hops (1-3, default 1). Higher values discover
    /// structurally distant but related nodes via edge chains.
    ///
    /// When absent, the effective default is determined by `strategy`:
    /// - [`SearchStrategy::Graph`] → 2 hops
    /// - All other strategies → 1 hop
    ///
    /// Explicit `max_hops` always wins over the strategy-derived default.
    #[serde(default)]
    pub max_hops: Option<u32>,
    /// Include only nodes whose `created_at` is strictly after this timestamp.
    /// When `None`, no lower-bound date filter is applied.
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub after: Option<DateTime<Utc>>,
    /// Include only nodes whose `created_at` is strictly before this timestamp.
    /// When `None`, no upper-bound date filter is applied.
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub before: Option<DateTime<Utc>>,
    /// Minimum score threshold (covalence#33). Results with a final score
    /// strictly below this value are filtered out before returning. When
    /// `None` (the default), all results are returned regardless of score.
    /// Useful for precision queries: set to 0.6 to avoid weak matches.
    #[serde(default)]
    pub min_score: Option<f64>,
}

fn default_limit() -> usize {
    10
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct WeightsInput {
    pub vector: Option<f32>,
    pub lexical: Option<f32>,
    pub graph: Option<f32>,
    /// Weight for the structural similarity dimension (covalence#52).
    /// When omitted, defaults to `0.0` when explicit weights are provided,
    /// or the strategy preset when using a strategy.
    pub structural: Option<f32>,
}

/// A single search result with scores breakdown.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SearchResult {
    #[schema(value_type = String)]
    pub node_id: Uuid,
    pub score: f64,
    pub vector_score: Option<f64>,
    pub lexical_score: Option<f64>,
    pub graph_score: Option<f64>,
    /// Structural similarity score from the graph-embedding dimension.
    /// `None` when the structural adaptor did not run or produced no result
    /// for this node (e.g. feature flag off, or no embedding in DB).
    pub structural_score: Option<f64>,
    pub confidence: f64,
    /// Trustworthiness score derived from source reliability.
    ///
    /// For source nodes this is the node's own `reliability` field.
    /// For article nodes this is the average `reliability` of all linked
    /// source nodes (via ORIGINATES / COMPILED_FROM / CONFIRMS edges).
    /// Defaults to 0.5 when no linked sources carry a reliability value.
    pub trust_score: Option<f64>,
    pub node_type: String,
    pub title: Option<String>,
    pub content_preview: String,
    pub domain_path: Option<Vec<String>>,
    /// For results returned by hierarchical expansion: the UUID of the parent
    /// article that caused this source to be included.  `None` for directly-
    /// matched results (articles or standard-mode results).
    #[schema(value_type = Option<String>)]
    pub expanded_from: Option<Uuid>,
    /// Number of graph hops from the nearest anchor node.
    /// `None` if this result was not discovered via graph traversal.
    pub graph_hops: Option<u32>,
    /// When this node was created. Populated from the `created_at` column on
    /// the `nodes` table; used by the `after`/`before` temporal filters.
    #[schema(value_type = Option<String>)]
    pub created_at: Option<DateTime<Utc>>,
    /// Topological confidence score derived from graph structure (PageRank +
    /// inbound-edge diversity).  `None` when the feature flag
    /// `COVALENCE_TOPOLOGICAL_CONFIDENCE` is not enabled.
    pub topological_score: Option<f64>,
}

/// Metadata about the search execution.
#[derive(Debug, Serialize)]
pub struct SearchMeta {
    pub total_results: usize,
    pub lexical_backend: String,
    pub dimensions_used: Vec<String>,
    pub elapsed_ms: u64,
    /// The effective fusion strategy used for this request (for transparency).
    /// One of `"balanced"`, `"precise"`, `"exploratory"`, `"graph"`,
    /// `"structural"`, or `"custom"` (when caller-supplied weights override).
    pub strategy: String,
}

// ─── Synthesis response types ─────────────────────────────────────────────────

/// A single source that was consumed by the live-synthesis pipeline.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SynthesisSource {
    /// UUID of the source node.
    #[schema(value_type = String)]
    pub node_id: Uuid,
    /// Source title, if available.
    pub title: Option<String>,
    /// Source reliability score in [0.0, 1.0].
    pub reliability: f64,
    /// 4-D relevance score that caused this source to be selected.
    pub relevance_score: f64,
}

/// Response body returned by [`SearchService::search_synthesis`].
///
/// Delivered as `POST /search` with `"mode": "synthesis"`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SynthesisResponse {
    /// Always `"synthesis"`.
    pub mode: String,
    /// The original query string.
    pub query: String,
    /// LLM-produced synthesis with inline source citations.
    pub synthesis: String,
    /// Sources ranked by reliability that were fed to the LLM.
    pub sources_used: Vec<SynthesisSource>,
    /// Wall-clock time for the full synthesis pipeline in milliseconds.
    pub elapsed_ms: u64,
    /// Number of sources consumed by the LLM.
    pub source_count: usize,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct SearchService {
    pool: PgPool,
    vector: VectorAdaptor,
    lexical: LexicalAdaptor,
    graph: GraphAdaptor,
    structural: StructuralAdaptor,
    /// Whether to blend topological confidence into scoring.
    /// Enabled via `COVALENCE_TOPOLOGICAL_CONFIDENCE=true`.
    topological_enabled: bool,
    /// Shared in-memory graph used for topological confidence computation.
    shared_graph: Option<crate::graph::SharedGraph>,
    /// LLM client used by the live-synthesis pipeline.
    llm: Option<Arc<dyn LlmClient>>,
    /// Whether the live-synthesis mode is enabled.
    /// Controlled by `COVALENCE_LIVE_SYNTHESIS=true`.
    synthesis_enabled: bool,
}

impl SearchService {
    pub fn new(pool: PgPool) -> Self {
        let topological_enabled = std::env::var("COVALENCE_TOPOLOGICAL_CONFIDENCE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let synthesis_enabled = std::env::var("COVALENCE_LIVE_SYNTHESIS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        Self {
            pool,
            vector: VectorAdaptor::new(),
            lexical: LexicalAdaptor::new(),
            graph: GraphAdaptor::new(),
            structural: StructuralAdaptor::new(),
            topological_enabled,
            shared_graph: None,
            llm: None,
            synthesis_enabled,
        }
    }

    /// Attach a shared in-memory graph for topological confidence scoring.
    pub fn with_graph(mut self, graph: crate::graph::SharedGraph) -> Self {
        self.shared_graph = Some(graph);
        self
    }

    /// Attach an LLM client for the live-synthesis pipeline.
    pub fn with_llm(mut self, llm: Arc<dyn LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// Check which backends are available. Call once at startup.
    pub async fn init(&self) {
        let v = self.vector.check_availability(&self.pool).await;
        let l = self.lexical.check_availability(&self.pool).await;
        let g = self.graph.check_availability(&self.pool).await;
        let s = self.structural.check_availability(&self.pool).await;
        tracing::info!(
            vector = v,
            lexical = l,
            graph = g,
            structural = s,
            "search dimensions initialized"
        );
    }

    pub async fn search(
        &self,
        req: SearchRequest,
    ) -> anyhow::Result<(Vec<SearchResult>, SearchMeta)> {
        match req.mode.clone().unwrap_or_default() {
            SearchMode::Standard => self.search_standard(req).await,
            SearchMode::Hierarchical => self.search_hierarchical(req).await,
            // Synthesis returns a different response shape.  The route handler
            // intercepts synthesis requests before calling this method; this
            // branch is a defensive safety net only.
            SearchMode::Synthesis => anyhow::bail!(
                "synthesis mode uses a different response shape; \
                 call search_synthesis() instead of search()"
            ),
        }
    }

    // ── Standard (flat) search ────────────────────────────────────────────────

    async fn search_standard(
        &self,
        req: SearchRequest,
    ) -> anyhow::Result<(Vec<SearchResult>, SearchMeta)> {
        let start = std::time::Instant::now();
        let candidate_limit = req.limit * 5;

        // Resolve effective max_hops: explicit request value wins; otherwise
        // derive from strategy (Graph → 2, everything else → 1).
        let effective_max_hops = req.max_hops.unwrap_or({
            if matches!(req.strategy, Some(SearchStrategy::Graph)) {
                2
            } else {
                1
            }
        });

        let dim_query = DimensionQuery {
            text: req.query.clone(),
            embedding: req.embedding,
            intent: req.intent,
            session_id: req.session_id,
            node_types: req.node_types,
            max_hops: Some(effective_max_hops),
        };

        let (w_vec, w_lex, w_graph, w_struct) = resolve_weights(&req.weights, &req.strategy);
        let effective_strategy = strategy_label(&req.weights, &req.strategy);
        let recency_bias = req.recency_bias.unwrap_or(0.0).clamp(0.0, 1.0);

        let (mut vec_results, mut lex_results) = tokio::try_join!(
            self.vector
                .search(&self.pool, &dim_query, None, candidate_limit),
            self.lexical
                .search(&self.pool, &dim_query, None, candidate_limit),
        )?;

        self.vector.normalize_scores(&mut vec_results);
        self.lexical.normalize_scores(&mut lex_results);

        let mut candidate_set: Vec<Uuid> = vec_results.iter().map(|r| r.node_id).collect();
        for r in &lex_results {
            if !candidate_set.contains(&r.node_id) {
                candidate_set.push(r.node_id);
            }
        }

        let mut graph_results = if !candidate_set.is_empty() {
            self.graph
                .search(
                    &self.pool,
                    &dim_query,
                    Some(&candidate_set),
                    candidate_limit,
                )
                .await?
        } else {
            vec![]
        };
        self.graph.normalize_scores(&mut graph_results);

        // Step 3: Structural dimension — same anchor set as graph.
        let mut structural_results = if !candidate_set.is_empty() {
            self.structural
                .search(
                    &self.pool,
                    &dim_query,
                    Some(&candidate_set),
                    candidate_limit,
                )
                .await?
        } else {
            vec![]
        };
        self.structural.normalize_scores(&mut structural_results);

        // Build a hop-count map from graph results for SearchResult transparency.
        let graph_hops_map: HashMap<Uuid, u32> = graph_results
            .iter()
            .filter_map(|r| r.hop.map(|h| (r.node_id, h)))
            .collect();

        let dims_used = collect_dims_used(
            &vec_results,
            &lex_results,
            &graph_results,
            &structural_results,
        );

        #[allow(clippy::type_complexity)]
        let mut node_scores: HashMap<Uuid, DimScores> = HashMap::new();
        for r in &vec_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .0 = Some(r.normalized_score);
        }
        for r in &lex_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .1 = Some(r.normalized_score);
        }
        for r in &graph_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .2 = Some(r.normalized_score);
        }
        for r in &structural_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .3 = Some(r.normalized_score);
        }

        let node_ids: Vec<Uuid> = node_scores.keys().cloned().collect();
        if node_ids.is_empty() {
            return Ok((
                vec![],
                empty_meta(
                    self.lexical.bm25_available(),
                    dims_used,
                    &start,
                    effective_strategy,
                ),
            ));
        }

        let node_map = fetch_node_map(&self.pool, &node_ids).await?;

        // Topological confidence (feature-flagged). Compute PageRank once,
        // release the graph lock, then build per-node scores from in-memory data.
        let topo_map: Option<HashMap<Uuid, TopologicalConfidence>> = if self.topological_enabled {
            if let Some(shared) = &self.shared_graph {
                let graph = shared.read().await;
                let pr_scores = pagerank(&graph, 0.85, 20);
                // Capture inbound counts and release lock.
                let topo: HashMap<Uuid, TopologicalConfidence> = node_ids
                    .iter()
                    .map(|id| {
                        let tc = compute_topological_confidence(id, &pr_scores, &graph);
                        (*id, tc)
                    })
                    .collect();
                drop(graph);
                Some(topo)
            } else {
                None
            }
        } else {
            None
        };

        let mut results = build_results(
            &node_scores,
            &node_map,
            w_vec,
            w_lex,
            w_graph,
            w_struct,
            None,
            recency_bias,
            &graph_hops_map,
            &req.query,
            topo_map.as_ref(),
        );
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(min) = req.min_score {
            results.retain(|r| r.score >= min);
        }
        if let Some(ref filter_paths) = req.domain_path {
            results.retain(|r| {
                r.domain_path
                    .as_ref()
                    .is_some_and(|dp| dp.iter().any(|d| filter_paths.contains(d)))
            });
        }
        // Temporal post-filters: exclude nodes outside the requested date range.
        if let Some(after) = req.after {
            results.retain(|r| r.created_at.is_none_or(|ca| ca > after));
        }
        if let Some(before) = req.before {
            results.retain(|r| r.created_at.is_none_or(|ca| ca < before));
        }
        results.truncate(req.limit);

        let meta = SearchMeta {
            total_results: results.len(),
            lexical_backend: lexical_backend_name(self.lexical.bm25_available()),
            dimensions_used: dims_used,
            elapsed_ms: start.elapsed().as_millis() as u64,
            strategy: effective_strategy,
        };
        Ok((results, meta))
    }

    // ── Hierarchical search ───────────────────────────────────────────────────

    /// Two-phase hierarchical retrieval.
    ///
    /// Phase 1 — search articles only.
    /// Phase 2 — expand the top-`EXPAND_N` articles via provenance edges.
    /// Phase 3 — fetch source nodes and attach at discounted score.
    async fn search_hierarchical(
        &self,
        req: SearchRequest,
    ) -> anyhow::Result<(Vec<SearchResult>, SearchMeta)> {
        const EXPAND_N: usize = 3;
        const SOURCE_SCORE_FACTOR: f64 = 0.8;

        let start = std::time::Instant::now();
        let candidate_limit = req.limit * 5;

        // Resolve effective max_hops: explicit request value wins; otherwise
        // derive from strategy (Graph → 2, everything else → 1).
        let effective_max_hops = req.max_hops.unwrap_or({
            if matches!(req.strategy, Some(SearchStrategy::Graph)) {
                2
            } else {
                1
            }
        });

        // Phase 1: restrict dimension queries to articles only.
        let article_dim_query = DimensionQuery {
            text: req.query.clone(),
            embedding: req.embedding.clone(),
            intent: req.intent,
            session_id: req.session_id,
            node_types: Some(vec!["article".into()]),
            max_hops: Some(effective_max_hops),
        };

        let (w_vec, w_lex, w_graph, w_struct) = resolve_weights(&req.weights, &req.strategy);
        let effective_strategy = strategy_label(&req.weights, &req.strategy);
        let recency_bias = req.recency_bias.unwrap_or(0.0).clamp(0.0, 1.0);

        let (mut vec_results, mut lex_results) = tokio::try_join!(
            self.vector
                .search(&self.pool, &article_dim_query, None, candidate_limit),
            self.lexical
                .search(&self.pool, &article_dim_query, None, candidate_limit),
        )?;

        self.vector.normalize_scores(&mut vec_results);
        self.lexical.normalize_scores(&mut lex_results);

        let mut candidate_set: Vec<Uuid> = vec_results.iter().map(|r| r.node_id).collect();
        for r in &lex_results {
            if !candidate_set.contains(&r.node_id) {
                candidate_set.push(r.node_id);
            }
        }

        let mut graph_results = if !candidate_set.is_empty() {
            self.graph
                .search(
                    &self.pool,
                    &article_dim_query,
                    Some(&candidate_set),
                    candidate_limit,
                )
                .await?
        } else {
            vec![]
        };
        self.graph.normalize_scores(&mut graph_results);

        // Structural dimension — same anchor set as graph.
        let mut structural_results = if !candidate_set.is_empty() {
            self.structural
                .search(
                    &self.pool,
                    &article_dim_query,
                    Some(&candidate_set),
                    candidate_limit,
                )
                .await?
        } else {
            vec![]
        };
        self.structural.normalize_scores(&mut structural_results);

        // Build a hop-count map from graph results for SearchResult transparency.
        let graph_hops_map: HashMap<Uuid, u32> = graph_results
            .iter()
            .filter_map(|r| r.hop.map(|h| (r.node_id, h)))
            .collect();

        let dims_used = collect_dims_used(
            &vec_results,
            &lex_results,
            &graph_results,
            &structural_results,
        );

        // Build article score map.
        let mut article_scores: HashMap<Uuid, DimScores> = HashMap::new();
        for r in &vec_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .0 = Some(r.normalized_score);
        }
        for r in &lex_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .1 = Some(r.normalized_score);
        }
        for r in &graph_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .2 = Some(r.normalized_score);
        }
        for r in &structural_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .3 = Some(r.normalized_score);
        }

        if article_scores.is_empty() {
            return Ok((
                vec![],
                empty_meta(
                    self.lexical.bm25_available(),
                    dims_used,
                    &start,
                    effective_strategy,
                ),
            ));
        }

        let article_ids: Vec<Uuid> = article_scores.keys().cloned().collect();
        let article_node_map = fetch_node_map(&self.pool, &article_ids).await?;

        // Topological confidence for hierarchical mode (feature-flagged).
        let article_topo_map: Option<HashMap<Uuid, TopologicalConfidence>> =
            if self.topological_enabled {
                if let Some(shared) = &self.shared_graph {
                    let graph = shared.read().await;
                    let pr_scores = pagerank(&graph, 0.85, 20);
                    let topo: HashMap<Uuid, TopologicalConfidence> = article_ids
                        .iter()
                        .map(|id| {
                            let tc = compute_topological_confidence(id, &pr_scores, &graph);
                            (*id, tc)
                        })
                        .collect();
                    drop(graph);
                    Some(topo)
                } else {
                    None
                }
            } else {
                None
            };

        // Build article results and score them.
        // Post-filter to articles only: the dimension adaptors do not apply
        // the node_types restriction in DimensionQuery, so non-article nodes
        // may have slipped through when they share content with articles.
        let mut article_results = build_results(
            &article_scores,
            &article_node_map,
            w_vec,
            w_lex,
            w_graph,
            w_struct,
            None,
            recency_bias,
            &graph_hops_map,
            &req.query,
            article_topo_map.as_ref(),
        );
        article_results.retain(|r| r.node_type == "article");
        article_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Phase 2: collect the top-N article IDs for source expansion.
        let top_articles: Vec<(Uuid, f64)> = article_results
            .iter()
            .take(EXPAND_N)
            .map(|r| (r.node_id, r.score))
            .collect();

        let top_article_ids: Vec<Uuid> = top_articles.iter().map(|(id, _)| *id).collect();

        // Fetch linked source node IDs for those articles via provenance edges.
        // Edge direction: source_node → article (ORIGINATES means the source
        // originated the article, so source is `source_node_id`).
        let provenance_rows = if !top_article_ids.is_empty() {
            sqlx::query_as::<_, (Uuid, Uuid)>(
                "SELECT e.target_node_id  AS article_id,
                        e.source_node_id  AS source_id
                 FROM   covalence.edges e
                 JOIN   covalence.nodes s ON s.id = e.source_node_id
                 WHERE  e.target_node_id = ANY($1)
                   AND  e.edge_type IN ('ORIGINATES', 'COMPILED_FROM', 'CONFIRMS')
                   AND  s.node_type = 'source'
                   AND  s.status   = 'active'",
            )
            .bind(&top_article_ids)
            .fetch_all(&self.pool)
            .await?
        } else {
            vec![]
        };

        // Build a map: source_id → parent_article_id (and the article score).
        // Keep only sources not already in the article result set.
        let article_id_set: std::collections::HashSet<Uuid> =
            article_results.iter().map(|r| r.node_id).collect();

        // parent_score_map: article_id → final score (used for derivation)
        let parent_score_map: HashMap<Uuid, f64> = top_articles.iter().cloned().collect();

        // source_id → first parent article it was found under
        let mut source_to_parent: HashMap<Uuid, Uuid> = HashMap::new();
        for (article_id, source_id) in &provenance_rows {
            if !article_id_set.contains(source_id) {
                source_to_parent.entry(*source_id).or_insert(*article_id);
            }
        }

        // Phase 3: fetch source node metadata and build expanded results.
        let source_ids: Vec<Uuid> = source_to_parent.keys().cloned().collect();
        let mut expanded_results: Vec<SearchResult> = if !source_ids.is_empty() {
            let source_node_map = fetch_node_map(&self.pool, &source_ids).await?;

            source_to_parent
                .iter()
                .filter_map(|(source_id, parent_article_id)| {
                    let node = source_node_map.get(source_id)?;
                    let (
                        _,
                        node_type,
                        title,
                        preview,
                        confidence,
                        modified_at,
                        trust,
                        domain_path,
                        created_at,
                    ) = node;

                    let parent_score = *parent_score_map.get(parent_article_id).unwrap_or(&0.5);
                    let derived_score = parent_score * SOURCE_SCORE_FACTOR;

                    // Apply the same freshness tiebreaker as standard mode,
                    // honouring the caller's recency_bias setting.
                    let days = (chrono::Utc::now() - modified_at).num_seconds() as f64 / 86400.0;
                    let freshness = (-0.01 * days).exp();
                    let bias = recency_bias;
                    let freshness_weight = 0.05 + bias * 0.35;
                    let dim_weight = 1.0 - freshness_weight - 0.10;
                    let final_score = derived_score * dim_weight
                        + trust * 0.05
                        + confidence * 0.05
                        + freshness * freshness_weight;

                    Some(SearchResult {
                        node_id: *source_id,
                        score: final_score,
                        vector_score: None,
                        lexical_score: None,
                        graph_score: None,
                        structural_score: None,
                        confidence: *confidence,
                        trust_score: Some(*trust),
                        node_type: node_type.clone(),
                        title: title.clone(),
                        content_preview: preview.clone(),
                        domain_path: domain_path.clone(),
                        expanded_from: Some(*parent_article_id),
                        graph_hops: None, // expanded via provenance, not graph traversal
                        created_at: Some(*created_at),
                        topological_score: None,
                    })
                })
                .collect()
        } else {
            vec![]
        };

        // Sort expanded sources by their derived score descending.
        expanded_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // min_score filter (covalence#33) — applied to both article and expanded results.
        if let Some(min) = req.min_score {
            article_results.retain(|r| r.score >= min);
            expanded_results.retain(|r| r.score >= min);
        }

        // Apply domain_path filter to both article and expanded results.
        if let Some(ref filter_paths) = req.domain_path {
            article_results.retain(|r| {
                r.domain_path
                    .as_ref()
                    .is_some_and(|dp| dp.iter().any(|d| filter_paths.contains(d)))
            });
            expanded_results.retain(|r| {
                r.domain_path
                    .as_ref()
                    .is_some_and(|dp| dp.iter().any(|d| filter_paths.contains(d)))
            });
        }
        // Temporal post-filters applied to both article and expanded results.
        if let Some(after) = req.after {
            article_results.retain(|r| r.created_at.is_none_or(|ca| ca > after));
            expanded_results.retain(|r| r.created_at.is_none_or(|ca| ca > after));
        }
        if let Some(before) = req.before {
            article_results.retain(|r| r.created_at.is_none_or(|ca| ca < before));
            expanded_results.retain(|r| r.created_at.is_none_or(|ca| ca < before));
        }

        // Combine: articles first (already sorted), then expanded sources.
        // Reserve at least 30% of slots for expanded sources when available,
        // so the expansion isn't crowded out by a large article corpus.
        let max_article_slots = if expanded_results.is_empty() {
            req.limit
        } else {
            // 70% articles, 30% sources — but use all slots for articles if
            // there aren't enough sources to fill the reserved portion.
            let reserved_source = (req.limit * 3) / 10;
            let reserved_source = reserved_source.min(expanded_results.len());
            req.limit.saturating_sub(reserved_source)
        };
        let article_slots = max_article_slots.min(article_results.len());
        article_results.truncate(article_slots);

        let source_slots = req.limit.saturating_sub(article_slots);
        expanded_results.truncate(source_slots);

        let mut combined = article_results;
        combined.extend(expanded_results);

        let meta = SearchMeta {
            total_results: combined.len(),
            lexical_backend: lexical_backend_name(self.lexical.bm25_available()),
            dimensions_used: dims_used,
            elapsed_ms: start.elapsed().as_millis() as u64,
            strategy: effective_strategy,
        };
        Ok((combined, meta))
    }

    // ── Live synthesis (POST /search with mode=synthesis) ────────────────────

    /// Article-free live synthesis pipeline (covalence#59).
    ///
    /// 1. Runs the 4-D search restricted to `node_type = 'source'` (top 10).
    /// 2. Fetches the full content of each returned source from the DB.
    /// 3. Ranks sources by reliability (descending) then relevance (descending).
    /// 4. Builds a structured LLM prompt that embeds each source with its
    ///    reliability score and instructs the model to cite by number.
    /// 5. Calls the LLM and returns the synthesis alongside provenance metadata.
    ///
    /// Returns [`anyhow::Error`] when:
    /// - `COVALENCE_LIVE_SYNTHESIS` is not set to `"true"` or `"1"`.
    /// - No LLM client is attached (call [`SearchService::with_llm`] first).
    pub async fn search_synthesis(
        &self,
        req: SearchRequest,
    ) -> anyhow::Result<SynthesisResponse> {
        if !self.synthesis_enabled {
            anyhow::bail!(
                "live synthesis is disabled; set COVALENCE_LIVE_SYNTHESIS=true to enable"
            );
        }

        let llm = self
            .llm
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("LLM client not attached; call with_llm() on SearchService"))?;

        let start = std::time::Instant::now();

        // Clone the query string before req is partially consumed.
        let query = req.query.clone();

        // Build a source-only search request — reuse all caller preferences.
        let source_req = SearchRequest {
            query: req.query,
            embedding: req.embedding,
            intent: req.intent,
            session_id: req.session_id,
            // Override: sources only, capped at 10.
            node_types: Some(vec!["source".into()]),
            limit: 10,
            weights: req.weights,
            mode: None, // avoid recursive Synthesis dispatch
            recency_bias: req.recency_bias,
            domain_path: req.domain_path,
            strategy: req.strategy,
            max_hops: req.max_hops,
            after: req.after,
            before: req.before,
            min_score: req.min_score,
        };

        // Step 1 — run standard 4-D search over sources.
        let (results, _meta) = self.search_standard(source_req).await?;

        // Keep only source nodes (the dimension adaptors may include non-source
        // nodes despite the node_types hint, so we post-filter to be safe).
        let source_results: Vec<&SearchResult> =
            results.iter().filter(|r| r.node_type == "source").collect();

        if source_results.is_empty() {
            return Ok(SynthesisResponse {
                mode: "synthesis".into(),
                query,
                synthesis: "No relevant sources were found to synthesize from.".into(),
                sources_used: vec![],
                elapsed_ms: start.elapsed().as_millis() as u64,
                source_count: 0,
            });
        }

        // Step 2 — fetch full source content.
        let source_ids: Vec<Uuid> = source_results.iter().map(|r| r.node_id).collect();
        let content_rows = sqlx::query_as::<_, (Uuid, Option<String>, Option<String>, Option<f64>)>(
            "SELECT id, title, content, reliability::float8
             FROM   covalence.nodes
             WHERE  id = ANY($1)
               AND  status = 'active'",
        )
        .bind(&source_ids)
        .fetch_all(&self.pool)
        .await?;

        // Build a map: node_id → (title, content, reliability).
        let content_map: HashMap<Uuid, (Option<String>, Option<String>, f64)> = content_rows
            .into_iter()
            .map(|(id, title, content, reliability)| {
                (id, (title, content, reliability.unwrap_or(0.5)))
            })
            .collect();

        // Step 3 — rank by reliability descending, then relevance descending.
        let mut ranked: Vec<(f64, f64, Uuid)> = source_results
            .iter()
            .map(|r| {
                let reliability = content_map
                    .get(&r.node_id)
                    .map(|(_, _, rel)| *rel)
                    .unwrap_or(0.5);
                (reliability, r.score, r.node_id)
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
        });

        // Step 4 — build the LLM prompt.
        let mut prompt = format!(
            "Given the following sources (ranked by relevance and reliability), \
             synthesize a comprehensive answer to the query: \"{query}\"\n\n\
             Sources (in order of reliability):\n"
        );

        let mut sources_used: Vec<SynthesisSource> = Vec::new();
        for (i, (reliability, relevance, node_id)) in ranked.iter().enumerate() {
            if let Some((title, content, _)) = content_map.get(node_id) {
                let title_str = title.as_deref().unwrap_or("(untitled)");
                let content_str = content.as_deref().unwrap_or("(no content)");
                // Truncate very long sources to keep the prompt manageable.
                let truncated = if content_str.len() > 2000 {
                    &content_str[..2000]
                } else {
                    content_str
                };
                prompt.push_str(&format!(
                    "[Source {n} - reliability: {reliability:.2}] Title: {title_str}\n{truncated}\n\n",
                    n = i + 1,
                ));
                sources_used.push(SynthesisSource {
                    node_id: *node_id,
                    title: title.clone(),
                    reliability: *reliability,
                    relevance_score: *relevance,
                });
            }
        }
        prompt.push_str("Synthesize a clear, accurate answer. Cite sources by number.\n");

        // Step 5 — call the LLM.
        let synthesis = llm.complete(&prompt, 1500).await?;

        let source_count = sources_used.len();
        Ok(SynthesisResponse {
            mode: "synthesis".into(),
            query,
            synthesis,
            sources_used,
            elapsed_ms: start.elapsed().as_millis() as u64,
            source_count,
        })
    }

    // ── Debug search (POST /search/debug) ────────────────────────────────────

    /// Run the full 4-D search pipeline and return raw per-dimension scores
    /// alongside the final fused results.  Used by `POST /search/debug`.
    ///
    /// Mirrors `search_standard` but captures `DimensionResult` snapshots before
    /// they are collapsed into the fused `SearchResult` list, so callers can
    /// inspect the contribution of each dimension.
    pub async fn search_debug(
        &self,
        req: SearchRequest,
    ) -> anyhow::Result<SearchDebugResponse> {
        let start = std::time::Instant::now();
        let candidate_limit = req.limit * 5;

        // Resolve effective max_hops (same logic as search_standard).
        let effective_max_hops = req.max_hops.unwrap_or({
            if matches!(req.strategy, Some(SearchStrategy::Graph)) {
                2
            } else {
                1
            }
        });

        // Destructure request fields we need after dim_query consumes some.
        let query_str = req.query.clone();
        let min_score = req.min_score;
        let domain_path = req.domain_path.clone();
        let after = req.after;
        let before = req.before;
        let limit = req.limit;
        let recency_bias = req.recency_bias.unwrap_or(0.0).clamp(0.0, 1.0);

        let (w_vec, w_lex, w_graph, w_struct) = resolve_weights(&req.weights, &req.strategy);
        let effective_strategy = strategy_label(&req.weights, &req.strategy);

        let dim_query = DimensionQuery {
            text: req.query,
            embedding: req.embedding,
            intent: req.intent,
            session_id: req.session_id,
            node_types: req.node_types,
            max_hops: Some(effective_max_hops),
        };

        // ── Availability checks ────────────────────────────────────────────────
        let (vec_available, lex_available) = tokio::join!(
            self.vector.check_availability(&self.pool),
            self.lexical.check_availability(&self.pool),
        );
        let (graph_available, struct_available) = tokio::join!(
            self.graph.check_availability(&self.pool),
            self.structural.check_availability(&self.pool),
        );

        // ── Step 1 (parallel): Vector + Lexical ───────────────────────────────
        let (mut vec_results, mut lex_results) = tokio::try_join!(
            self.vector
                .search(&self.pool, &dim_query, None, candidate_limit),
            self.lexical
                .search(&self.pool, &dim_query, None, candidate_limit),
        )?;

        self.vector.normalize_scores(&mut vec_results);
        self.lexical.normalize_scores(&mut lex_results);

        // ── Step 2: Build candidate set ───────────────────────────────────────
        let mut candidate_set: Vec<Uuid> = vec_results.iter().map(|r| r.node_id).collect();
        for r in &lex_results {
            if !candidate_set.contains(&r.node_id) {
                candidate_set.push(r.node_id);
            }
        }

        // ── Step 3: Graph ─────────────────────────────────────────────────────
        let mut graph_results = if !candidate_set.is_empty() {
            self.graph
                .search(
                    &self.pool,
                    &dim_query,
                    Some(&candidate_set),
                    candidate_limit,
                )
                .await?
        } else {
            vec![]
        };
        self.graph.normalize_scores(&mut graph_results);

        // ── Step 4: Structural ────────────────────────────────────────────────
        let mut structural_results = if !candidate_set.is_empty() {
            self.structural
                .search(
                    &self.pool,
                    &dim_query,
                    Some(&candidate_set),
                    candidate_limit,
                )
                .await?
        } else {
            vec![]
        };
        self.structural.normalize_scores(&mut structural_results);

        // ── Capture raw-score snapshots (before building node_scores map) ─────
        let vec_debug = DimensionDebugInfo {
            available: vec_available,
            results_count: vec_results.len(),
            raw_scores: vec_results
                .iter()
                .map(|r| RawScoreEntry {
                    node_id: r.node_id,
                    raw_score: r.raw_score,
                    normalized_score: r.normalized_score,
                })
                .collect(),
        };
        let lex_debug = DimensionDebugInfo {
            available: lex_available,
            results_count: lex_results.len(),
            raw_scores: lex_results
                .iter()
                .map(|r| RawScoreEntry {
                    node_id: r.node_id,
                    raw_score: r.raw_score,
                    normalized_score: r.normalized_score,
                })
                .collect(),
        };
        let graph_debug = DimensionDebugInfo {
            available: graph_available,
            results_count: graph_results.len(),
            raw_scores: graph_results
                .iter()
                .map(|r| RawScoreEntry {
                    node_id: r.node_id,
                    raw_score: r.raw_score,
                    normalized_score: r.normalized_score,
                })
                .collect(),
        };
        let struct_debug = DimensionDebugInfo {
            available: struct_available,
            results_count: structural_results.len(),
            raw_scores: structural_results
                .iter()
                .map(|r| RawScoreEntry {
                    node_id: r.node_id,
                    raw_score: r.raw_score,
                    normalized_score: r.normalized_score,
                })
                .collect(),
        };

        // ── Build hop-count and node-score maps ───────────────────────────────
        let graph_hops_map: HashMap<Uuid, u32> = graph_results
            .iter()
            .filter_map(|r| r.hop.map(|h| (r.node_id, h)))
            .collect();

        let dims_used = collect_dims_used(
            &vec_results,
            &lex_results,
            &graph_results,
            &structural_results,
        );

        let mut node_scores: HashMap<Uuid, DimScores> = HashMap::new();
        for r in &vec_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .0 = Some(r.normalized_score);
        }
        for r in &lex_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .1 = Some(r.normalized_score);
        }
        for r in &graph_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .2 = Some(r.normalized_score);
        }
        for r in &structural_results {
            node_scores
                .entry(r.node_id)
                .or_insert((None, None, None, None))
                .3 = Some(r.normalized_score);
        }

        // ── Fuse and build final results ──────────────────────────────────────
        let final_results = if node_scores.is_empty() {
            vec![]
        } else {
            let node_ids: Vec<Uuid> = node_scores.keys().cloned().collect();
            let node_map = fetch_node_map(&self.pool, &node_ids).await?;

            let topo_map: Option<HashMap<Uuid, TopologicalConfidence>> =
                if self.topological_enabled {
                    if let Some(shared) = &self.shared_graph {
                        let graph = shared.read().await;
                        let pr_scores = pagerank(&graph, 0.85, 20);
                        let topo: HashMap<Uuid, TopologicalConfidence> = node_ids
                            .iter()
                            .map(|id| {
                                let tc = compute_topological_confidence(id, &pr_scores, &graph);
                                (*id, tc)
                            })
                            .collect();
                        drop(graph);
                        Some(topo)
                    } else {
                        None
                    }
                } else {
                    None
                };

            let mut results = build_results(
                &node_scores,
                &node_map,
                w_vec,
                w_lex,
                w_graph,
                w_struct,
                None,
                recency_bias,
                &graph_hops_map,
                &query_str,
                topo_map.as_ref(),
            );
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(min) = min_score {
                results.retain(|r| r.score >= min);
            }
            if let Some(ref filter_paths) = domain_path {
                results.retain(|r| {
                    r.domain_path
                        .as_ref()
                        .is_some_and(|dp| dp.iter().any(|d| filter_paths.contains(d)))
                });
            }
            if let Some(after) = after {
                results.retain(|r| r.created_at.is_none_or(|ca| ca > after));
            }
            if let Some(before) = before {
                results.retain(|r| r.created_at.is_none_or(|ca| ca < before));
            }
            results.truncate(limit);
            results
        };

        let _ = dims_used; // captured for potential future use in the response

        Ok(SearchDebugResponse {
            query: query_str,
            strategy_selected: effective_strategy,
            dimensions: DimensionsDebug {
                vector: vec_debug,
                lexical: lex_debug,
                graph: graph_debug,
                structural: struct_debug,
            },
            fusion_weights: FusionWeights {
                vector: w_vec,
                lexical: w_lex,
                graph: w_graph,
                structural: w_struct,
            },
            final_results,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
}

// ─── Type aliases ─────────────────────────────────────────────────────────────

/// Per-node dimensional scores `(vector, lexical, graph, structural)`.
type DimScores = (Option<f64>, Option<f64>, Option<f64>, Option<f64>);

// ─── Shared helpers ───────────────────────────────────────────────────────────

// ─── Debug endpoint types ─────────────────────────────────────────────────────

/// Raw score entry for a single node in one search dimension.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RawScoreEntry {
    #[schema(value_type = String)]
    pub node_id: Uuid,
    /// Unnormalized score from the dimension (distance, ts_rank, edge weight, etc.)
    pub raw_score: f64,
    /// Normalized score in \[0.0, 1.0\] after intra-dimension normalization.
    pub normalized_score: f64,
}

/// Per-dimension debug snapshot captured before score fusion.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DimensionDebugInfo {
    /// Whether this dimension's backend is available in this deployment.
    pub available: bool,
    /// Number of candidate results this dimension returned before fusion.
    pub results_count: usize,
    /// Raw and normalized scores for each result.
    pub raw_scores: Vec<RawScoreEntry>,
}

/// All four dimension snapshots.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DimensionsDebug {
    pub vector: DimensionDebugInfo,
    pub lexical: DimensionDebugInfo,
    pub graph: DimensionDebugInfo,
    pub structural: DimensionDebugInfo,
}

/// Effective dimension fusion weights used for this request.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FusionWeights {
    pub vector: f32,
    pub lexical: f32,
    pub graph: f32,
    pub structural: f32,
}

/// Full debug response from `POST /search/debug`.
///
/// Contains the same fused results as `/search` plus a per-dimension breakdown
/// of raw scores before fusion — useful for eval, tuning, and debugging.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SearchDebugResponse {
    /// The original query string.
    pub query: String,
    /// The effective fusion strategy label (`"balanced"`, `"precise"`, …).
    pub strategy_selected: String,
    /// Per-dimension availability and raw score breakdown.
    pub dimensions: DimensionsDebug,
    /// Effective dimension weights used for fusion.
    pub fusion_weights: FusionWeights,
    /// Final fused results (identical to what `/search` would return).
    pub final_results: Vec<SearchResult>,
    /// Wall-clock time for the full debug pipeline in milliseconds.
    pub elapsed_ms: u64,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Parse and normalise weights from the request (or use defaults).
///
/// Priority order (highest wins):
/// 1. Explicit `weights` field (caller override).
/// 2. `strategy` field (preset dimension ratios).
/// 3. [`SearchStrategy::Balanced`] defaults.
pub fn resolve_weights(
    w: &Option<WeightsInput>,
    strategy: &Option<SearchStrategy>,
) -> (f32, f32, f32, f32) {
    let (v, l, g, s) = match w {
        // Explicit caller weights always take precedence.
        // structural defaults to 0.0 so callers who don't specify it get no
        // structural contribution (backward-compatible with 3-dim requests).
        Some(wi) => (
            wi.vector.unwrap_or(0.55),
            wi.lexical.unwrap_or(0.20),
            wi.graph.unwrap_or(0.10),
            wi.structural.unwrap_or(0.0),
        ),
        None => match strategy {
            Some(SearchStrategy::Precise) => (0.30, 0.45, 0.10, 0.15),
            Some(SearchStrategy::Exploratory) => (0.65, 0.10, 0.10, 0.15),
            Some(SearchStrategy::Graph) => (0.25, 0.10, 0.45, 0.20),
            Some(SearchStrategy::Structural) => (0.25, 0.10, 0.10, 0.55),
            // Balanced (or absent) → defaults.
            Some(SearchStrategy::Balanced) | None => (0.55, 0.20, 0.10, 0.15),
        },
    };
    let sum = v + l + g + s;
    if sum > 0.0 {
        (v / sum, l / sum, g / sum, s / sum)
    } else {
        (0.50, 0.30, 0.15, 0.05)
    }
}

/// Return the human-readable label for the effective fusion strategy.
fn strategy_label(w: &Option<WeightsInput>, strategy: &Option<SearchStrategy>) -> String {
    if w.is_some() {
        return "custom".to_string();
    }
    match strategy {
        Some(SearchStrategy::Precise) => "precise".to_string(),
        Some(SearchStrategy::Exploratory) => "exploratory".to_string(),
        Some(SearchStrategy::Graph) => "graph".to_string(),
        Some(SearchStrategy::Structural) => "structural".to_string(),
        Some(SearchStrategy::Balanced) | None => "balanced".to_string(),
    }
}

/// Collect dimension names that produced non-empty result sets.
fn collect_dims_used(
    vec_results: &[impl Sized],
    lex_results: &[impl Sized],
    graph_results: &[impl Sized],
    structural_results: &[impl Sized],
) -> Vec<String> {
    let mut dims = vec![];
    if !vec_results.is_empty() {
        dims.push("vector".to_string());
    }
    if !lex_results.is_empty() {
        dims.push("lexical".to_string());
    }
    if !graph_results.is_empty() {
        dims.push("graph".to_string());
    }
    if !structural_results.is_empty() {
        dims.push("structural".to_string());
    }
    dims
}

fn lexical_backend_name(bm25: bool) -> String {
    if bm25 { "bm25" } else { "ts_rank" }.to_string()
}

fn empty_meta(
    bm25: bool,
    dims_used: Vec<String>,
    start: &std::time::Instant,
    strategy: String,
) -> SearchMeta {
    SearchMeta {
        total_results: 0,
        lexical_backend: lexical_backend_name(bm25),
        dimensions_used: dims_used,
        elapsed_ms: start.elapsed().as_millis() as u64,
        strategy,
    }
}

// ─── Node metadata fetch & result construction ───────────────────────────────

type NodeRow = (
    Uuid,
    String,
    Option<String>,
    String,
    f64,
    DateTime<Utc>,
    f64,
    Option<Vec<String>>,
    DateTime<Utc>,
);

/// Bulk-fetch node metadata (including trust scores) for the given IDs.
async fn fetch_node_map(
    pool: &PgPool,
    node_ids: &[Uuid],
) -> anyhow::Result<HashMap<Uuid, NodeRow>> {
    let rows = sqlx::query_as::<_, NodeRow>(
        "WITH article_trust AS (
             SELECT e.target_node_id                      AS node_id,
                    AVG(COALESCE(s.reliability, 0.5))     AS avg_reliability
             FROM   covalence.edges e
             JOIN   covalence.nodes s ON s.id = e.source_node_id
             WHERE  e.target_node_id = ANY($1)
               AND  e.edge_type IN ('ORIGINATES', 'COMPILED_FROM', 'CONFIRMS')
             GROUP BY e.target_node_id
         )
         SELECT n.id,
                n.node_type,
                n.title,
                LEFT(n.content, 200)                      AS preview,
                COALESCE(n.confidence, 0.5)::float8       AS confidence,
                n.modified_at,
                CASE
                    WHEN n.node_type = 'source'
                        THEN COALESCE(n.reliability, 0.5)
                    ELSE COALESCE(at.avg_reliability, 0.5)
                END::float8                               AS trust_score,
                n.domain_path,
                n.created_at
         FROM   covalence.nodes n
         LEFT JOIN article_trust at ON at.node_id = n.id
         WHERE  n.id = ANY($1)
           AND  n.status = 'active'",
    )
    .bind(node_ids)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|n| (n.0, n)).collect())
}

/// Assemble [`SearchResult`] values from dimension score maps and node metadata.
///
/// `query`: the original search query string — used to compute title match
/// bonuses (covalence#33). Words longer than 3 chars that appear in the node
/// title each contribute up to `0.20` bonus spread across matched fraction.
///
/// `expanded_from_override`: when `Some(parent_id)`, all results get that
/// parent — used by the hierarchical expander.  Pass `None` for normal use.
///
/// `graph_hops_map`: maps node ID → hop distance discovered via graph traversal.
/// Nodes absent from this map receive `graph_hops: None`.
#[allow(clippy::too_many_arguments)]
fn build_results(
    node_scores: &HashMap<Uuid, DimScores>,
    node_map: &HashMap<Uuid, NodeRow>,
    w_vec: f32,
    w_lex: f32,
    w_graph: f32,
    w_struct: f32,
    expanded_from_override: Option<Uuid>,
    recency_bias: f64,
    graph_hops_map: &HashMap<Uuid, u32>,
    query: &str,
    topo_scores: Option<&HashMap<Uuid, TopologicalConfidence>>,
) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for (node_id, (vs, ls, gs, ss)) in node_scores {
        let Some(node) = node_map.get(node_id) else {
            continue;
        };
        let (_, node_type, title, preview, confidence, modified_at, trust, domain_path, created_at) =
            node;

        let mut weighted_sum = 0.0f64;
        if let Some(v) = vs {
            weighted_sum += v * w_vec as f64;
        }
        if let Some(l) = ls {
            weighted_sum += l * w_lex as f64;
        }
        if let Some(g) = gs {
            weighted_sum += g * w_graph as f64;
        }
        if let Some(s) = ss {
            weighted_sum += s * w_struct as f64;
        }

        let days = (chrono::Utc::now() - modified_at).num_seconds() as f64 / 86400.0;
        let freshness = (-0.01 * days).exp();

        let bias = recency_bias.clamp(0.0, 1.0);
        // freshness_weight scales from 0.05 (bias=0) to 0.40 (bias=1)
        let freshness_weight = 0.05 + bias * 0.35;
        // dim_weight absorbs the difference (trust and confidence stay at 0.05 each)
        let dim_weight = 1.0 - freshness_weight - 0.10; // 0.10 = trust + confidence

        // Topological confidence blending (feature-flagged).
        let (effective_confidence, topo_score_out) = if let Some(topo_map) = topo_scores {
            let topo = topo_map.get(node_id);
            let topo_sc = topo.map(|t| t.score).unwrap_or(0.0);
            let blended = 0.6 * confidence + 0.4 * topo_sc;
            (blended, Some(topo_sc))
        } else {
            (*confidence, None)
        };

        let base_score = weighted_sum * dim_weight
            + trust * 0.05
            + effective_confidence * 0.05
            + freshness * freshness_weight;

        // Title match bonus (covalence#33): query words >3 chars that appear
        // in the title boost relevance by up to 0.20 (proportional to the
        // fraction of significant query words matched).
        let title_bonus = if let Some(title) = title {
            let query_words: Vec<&str> = query.split_whitespace().filter(|w| w.len() > 3).collect();
            if query_words.is_empty() {
                0.0
            } else {
                let title_lower = title.to_lowercase();
                let matched = query_words
                    .iter()
                    .filter(|w| title_lower.contains(&w.to_lowercase()))
                    .count();
                0.20 * (matched as f64 / query_words.len() as f64)
            }
        } else {
            0.0
        };
        let final_score = base_score + title_bonus;

        results.push(SearchResult {
            node_id: *node_id,
            score: final_score,
            vector_score: *vs,
            lexical_score: *ls,
            graph_score: *gs,
            structural_score: *ss,
            confidence: *confidence,
            trust_score: Some(*trust),
            node_type: node_type.clone(),
            title: title.clone(),
            content_preview: preview.clone(),
            domain_path: domain_path.clone(),
            expanded_from: expanded_from_override,
            graph_hops: graph_hops_map.get(node_id).copied(),
            created_at: Some(*created_at),
            topological_score: topo_score_out,
        });
    }
    results
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── SearchMode parsing ────────────────────────────────────────────────────

    #[test]
    fn test_synthesis_mode_deserializes_from_json() {
        let json = r#"{"query": "what is Covalence?", "mode": "synthesis"}"#;
        let req: SearchRequest = serde_json::from_str(json).expect("should deserialise");
        assert_eq!(req.mode, Some(SearchMode::Synthesis));
    }

    #[test]
    fn test_standard_mode_still_deserializes() {
        let json = r#"{"query": "test", "mode": "standard"}"#;
        let req: SearchRequest = serde_json::from_str(json).expect("should deserialise");
        assert_eq!(req.mode, Some(SearchMode::Standard));
    }

    #[test]
    fn test_hierarchical_mode_still_deserializes() {
        let json = r#"{"query": "test", "mode": "hierarchical"}"#;
        let req: SearchRequest = serde_json::from_str(json).expect("should deserialise");
        assert_eq!(req.mode, Some(SearchMode::Hierarchical));
    }

    #[test]
    fn test_missing_mode_defaults_to_standard() {
        let json = r#"{"query": "test"}"#;
        let req: SearchRequest = serde_json::from_str(json).expect("should deserialise");
        // None here — default is applied by SearchService::search(), not during parse.
        assert_eq!(req.mode, None);
        assert_eq!(req.mode.unwrap_or_default(), SearchMode::Standard);
    }

    #[test]
    fn test_synthesis_mode_serializes_to_lowercase() {
        let mode = SearchMode::Synthesis;
        let serialized = serde_json::to_string(&mode).expect("should serialise");
        assert_eq!(serialized, r#""synthesis""#);
    }

    #[test]
    fn test_all_modes_roundtrip() {
        for (label, expected) in [
            ("standard", SearchMode::Standard),
            ("hierarchical", SearchMode::Hierarchical),
            ("synthesis", SearchMode::Synthesis),
        ] {
            let serialized = serde_json::to_string(&expected).unwrap();
            assert_eq!(serialized, format!(r#""{label}""#));
            let deserialized: SearchMode = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, expected);
        }
    }

    // ── Feature-flag behaviour ────────────────────────────────────────────────

    /// The live-synthesis feature flag must be **opt-in** — absent or any value
    /// other than `"true"` / `"1"` keeps synthesis disabled.
    #[test]
    fn test_synthesis_feature_flag_disabled_by_default() {
        // Save and unset the env var so this test is hermetic.
        let saved = std::env::var("COVALENCE_LIVE_SYNTHESIS").ok();
        // SAFETY: single-threaded test; safe to mutate env.
        unsafe { std::env::remove_var("COVALENCE_LIVE_SYNTHESIS") };

        let enabled = std::env::var("COVALENCE_LIVE_SYNTHESIS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        assert!(!enabled, "synthesis should be disabled when env var is absent");

        // Restore.
        if let Some(val) = saved {
            unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", val) };
        }
    }

    #[test]
    fn test_synthesis_feature_flag_enabled_by_true() {
        let saved = std::env::var("COVALENCE_LIVE_SYNTHESIS").ok();
        unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", "true") };

        let enabled = std::env::var("COVALENCE_LIVE_SYNTHESIS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        assert!(enabled, "synthesis should be enabled when env var is 'true'");

        // Restore.
        unsafe { std::env::remove_var("COVALENCE_LIVE_SYNTHESIS") };
        if let Some(val) = saved {
            unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", val) };
        }
    }

    #[test]
    fn test_synthesis_feature_flag_enabled_by_one() {
        let saved = std::env::var("COVALENCE_LIVE_SYNTHESIS").ok();
        unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", "1") };

        let enabled = std::env::var("COVALENCE_LIVE_SYNTHESIS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        assert!(enabled, "synthesis should be enabled when env var is '1'");

        unsafe { std::env::remove_var("COVALENCE_LIVE_SYNTHESIS") };
        if let Some(val) = saved {
            unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", val) };
        }
    }

    #[test]
    fn test_synthesis_feature_flag_not_enabled_by_arbitrary_value() {
        let saved = std::env::var("COVALENCE_LIVE_SYNTHESIS").ok();
        unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", "yes") };

        let enabled = std::env::var("COVALENCE_LIVE_SYNTHESIS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        assert!(
            !enabled,
            "synthesis should NOT be enabled by arbitrary truthy strings"
        );

        unsafe { std::env::remove_var("COVALENCE_LIVE_SYNTHESIS") };
        if let Some(val) = saved {
            unsafe { std::env::set_var("COVALENCE_LIVE_SYNTHESIS", val) };
        }
    }

    // ── SynthesisResponse shape ───────────────────────────────────────────────

    #[test]
    fn test_synthesis_response_serializes_correctly() {
        let resp = SynthesisResponse {
            mode: "synthesis".into(),
            query: "what is Covalence?".into(),
            synthesis: "Covalence is a knowledge engine. [1]".into(),
            sources_used: vec![SynthesisSource {
                node_id: uuid::Uuid::nil(),
                title: Some("About Covalence".into()),
                reliability: 0.8,
                relevance_score: 0.92,
            }],
            elapsed_ms: 142,
            source_count: 1,
        };

        let json = serde_json::to_value(&resp).expect("should serialise");
        assert_eq!(json["mode"], "synthesis");
        assert_eq!(json["source_count"], 1);
        assert_eq!(json["sources_used"][0]["reliability"], 0.8);
    }
}
