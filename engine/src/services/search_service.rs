//! Search service — orchestrates the three-dimensional cascade (SPEC §7.2).
//!
//! Step 1 (parallel): Lexical + Vector via tokio::try_join!
//! Step 2 (sequential): Graph from candidate anchors
//! Step 3: Score fusion via ScoreFusion
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

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::SearchIntent;
use crate::search::dimension::{DimensionAdaptor, DimensionQuery};
use crate::search::graph::GraphAdaptor;
use crate::search::lexical::LexicalAdaptor;
use crate::search::vector::VectorAdaptor;

// ─── Search Mode ──────────────────────────────────────────────────────────────

/// Controls how the search engine retrieves and returns results.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Flat search over all node types (current behaviour). This is the
    /// default when `mode` is absent from the request.
    #[default]
    Standard,
    /// Two-phase hierarchical retrieval: articles first, then their linked
    /// sources expanded via provenance edges.
    Hierarchical,
}

// ─── Request / Response types ─────────────────────────────────────────────────

/// Request body for POST /search.
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(default)]
    pub intent: Option<SearchIntent>,
    #[serde(default)]
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
}

fn default_limit() -> usize {
    10
}

#[derive(Debug, Deserialize)]
pub struct WeightsInput {
    pub vector: Option<f32>,
    pub lexical: Option<f32>,
    pub graph: Option<f32>,
}

/// A single search result with scores breakdown.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub node_id: Uuid,
    pub score: f64,
    pub vector_score: Option<f64>,
    pub lexical_score: Option<f64>,
    pub graph_score: Option<f64>,
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
    pub expanded_from: Option<Uuid>,
}

/// Metadata about the search execution.
#[derive(Debug, Serialize)]
pub struct SearchMeta {
    pub total_results: usize,
    pub lexical_backend: String,
    pub dimensions_used: Vec<String>,
    pub elapsed_ms: u64,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct SearchService {
    pool: PgPool,
    vector: VectorAdaptor,
    lexical: LexicalAdaptor,
    graph: GraphAdaptor,
}

impl SearchService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            vector: VectorAdaptor::new(),
            lexical: LexicalAdaptor::new(),
            graph: GraphAdaptor::new(),
        }
    }

    /// Check which backends are available. Call once at startup.
    pub async fn init(&self) {
        let v = self.vector.check_availability(&self.pool).await;
        let l = self.lexical.check_availability(&self.pool).await;
        let g = self.graph.check_availability(&self.pool).await;
        tracing::info!(
            vector = v,
            lexical = l,
            graph = g,
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
        }
    }

    // ── Standard (flat) search ────────────────────────────────────────────────

    async fn search_standard(
        &self,
        req: SearchRequest,
    ) -> anyhow::Result<(Vec<SearchResult>, SearchMeta)> {
        let start = std::time::Instant::now();
        let candidate_limit = req.limit * 5;

        let dim_query = DimensionQuery {
            text: req.query.clone(),
            embedding: req.embedding,
            intent: req.intent,
            session_id: req.session_id,
            node_types: req.node_types,
        };

        let (w_vec, w_lex, w_graph) = resolve_weights(&req.weights);
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

        let dims_used = collect_dims_used(&vec_results, &lex_results, &graph_results);

        #[allow(clippy::type_complexity)]
        let mut node_scores: HashMap<Uuid, DimScores> = HashMap::new();
        for r in &vec_results {
            node_scores.entry(r.node_id).or_insert((None, None, None)).0 = Some(r.normalized_score);
        }
        for r in &lex_results {
            node_scores.entry(r.node_id).or_insert((None, None, None)).1 = Some(r.normalized_score);
        }
        for r in &graph_results {
            node_scores.entry(r.node_id).or_insert((None, None, None)).2 = Some(r.normalized_score);
        }

        let node_ids: Vec<Uuid> = node_scores.keys().cloned().collect();
        if node_ids.is_empty() {
            return Ok((
                vec![],
                empty_meta(self.lexical.bm25_available(), dims_used, &start),
            ));
        }

        let node_map = fetch_node_map(&self.pool, &node_ids).await?;

        let mut results = build_results(
            &node_scores,
            &node_map,
            w_vec,
            w_lex,
            w_graph,
            None,
            recency_bias,
        );
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(ref filter_paths) = req.domain_path {
            results.retain(|r| {
                r.domain_path
                    .as_ref()
                    .is_some_and(|dp| dp.iter().any(|d| filter_paths.contains(d)))
            });
        }
        results.truncate(req.limit);

        let meta = SearchMeta {
            total_results: results.len(),
            lexical_backend: lexical_backend_name(self.lexical.bm25_available()),
            dimensions_used: dims_used,
            elapsed_ms: start.elapsed().as_millis() as u64,
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

        // Phase 1: restrict dimension queries to articles only.
        let article_dim_query = DimensionQuery {
            text: req.query.clone(),
            embedding: req.embedding.clone(),
            intent: req.intent,
            session_id: req.session_id,
            node_types: Some(vec!["article".into()]),
        };

        let (w_vec, w_lex, w_graph) = resolve_weights(&req.weights);
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

        let dims_used = collect_dims_used(&vec_results, &lex_results, &graph_results);

        // Build article score map.
        let mut article_scores: HashMap<Uuid, DimScores> = HashMap::new();
        for r in &vec_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None))
                .0 = Some(r.normalized_score);
        }
        for r in &lex_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None))
                .1 = Some(r.normalized_score);
        }
        for r in &graph_results {
            article_scores
                .entry(r.node_id)
                .or_insert((None, None, None))
                .2 = Some(r.normalized_score);
        }

        if article_scores.is_empty() {
            return Ok((
                vec![],
                empty_meta(self.lexical.bm25_available(), dims_used, &start),
            ));
        }

        let article_ids: Vec<Uuid> = article_scores.keys().cloned().collect();
        let article_node_map = fetch_node_map(&self.pool, &article_ids).await?;

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
            None,
            recency_bias,
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
                    let (_, node_type, title, preview, confidence, modified_at, trust, domain_path) = node;

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
                        confidence: *confidence,
                        trust_score: Some(*trust),
                        node_type: node_type.clone(),
                        title: title.clone(),
                        content_preview: preview.clone(),
                        domain_path: domain_path.clone(),
                        expanded_from: Some(*parent_article_id),
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
        };
        Ok((combined, meta))
    }
}

// ─── Type aliases ─────────────────────────────────────────────────────────────

/// Per-node dimensional scores `(vector, lexical, graph)`.
type DimScores = (Option<f64>, Option<f64>, Option<f64>);

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Parse and normalise weights from the request (or use defaults).
fn resolve_weights(w: &Option<WeightsInput>) -> (f32, f32, f32) {
    let (v, l, g) = match w {
        Some(wi) => (
            wi.vector.unwrap_or(0.65),
            wi.lexical.unwrap_or(0.25),
            wi.graph.unwrap_or(0.10),
        ),
        None => (0.65, 0.25, 0.10),
    };
    let sum = v + l + g;
    if sum > 0.0 {
        (v / sum, l / sum, g / sum)
    } else {
        (0.50, 0.30, 0.20)
    }
}

/// Collect dimension names that produced non-empty result sets.
fn collect_dims_used(
    vec_results: &[impl Sized],
    lex_results: &[impl Sized],
    graph_results: &[impl Sized],
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
    dims
}

fn lexical_backend_name(bm25: bool) -> String {
    if bm25 { "bm25" } else { "ts_rank" }.to_string()
}

fn empty_meta(bm25: bool, dims_used: Vec<String>, start: &std::time::Instant) -> SearchMeta {
    SearchMeta {
        total_results: 0,
        lexical_backend: lexical_backend_name(bm25),
        dimensions_used: dims_used,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

// ─── Node metadata fetch & result construction ───────────────────────────────

type NodeRow = (
    Uuid,
    String,
    Option<String>,
    String,
    f64,
    chrono::DateTime<chrono::Utc>,
    f64,
    Option<Vec<String>>,
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
                n.domain_path
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
/// `expanded_from_override`: when `Some(parent_id)`, all results get that
/// parent — used by the hierarchical expander.  Pass `None` for normal use.
fn build_results(
    node_scores: &HashMap<Uuid, DimScores>,
    node_map: &HashMap<Uuid, NodeRow>,
    w_vec: f32,
    w_lex: f32,
    w_graph: f32,
    expanded_from_override: Option<Uuid>,
    recency_bias: f64,
) -> Vec<SearchResult> {
    let mut results = Vec::new();
    for (node_id, (vs, ls, gs)) in node_scores {
        let Some(node) = node_map.get(node_id) else {
            continue;
        };
        let (_, node_type, title, preview, confidence, modified_at, trust, domain_path) = node;

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

        let days = (chrono::Utc::now() - modified_at).num_seconds() as f64 / 86400.0;
        let freshness = (-0.01 * days).exp();

        let bias = recency_bias.clamp(0.0, 1.0);
        // freshness_weight scales from 0.05 (bias=0) to 0.40 (bias=1)
        let freshness_weight = 0.05 + bias * 0.35;
        // dim_weight absorbs the difference (trust and confidence stay at 0.05 each)
        let dim_weight = 1.0 - freshness_weight - 0.10; // 0.10 = trust + confidence
        let final_score = weighted_sum * dim_weight
            + trust * 0.05
            + *confidence * 0.05
            + freshness * freshness_weight;

        results.push(SearchResult {
            node_id: *node_id,
            score: final_score,
            vector_score: *vs,
            lexical_score: *ls,
            graph_score: *gs,
            confidence: *confidence,
            trust_score: Some(*trust),
            node_type: node_type.clone(),
            title: title.clone(),
            content_preview: preview.clone(),
            domain_path: domain_path.clone(),
            expanded_from: expanded_from_override,
        });
    }
    results
}
