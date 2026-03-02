//! Search service — orchestrates the three-dimensional cascade (SPEC §7.2).
//!
//! Step 1 (parallel): Lexical + Vector via tokio::try_join!
//! Step 2 (sequential): Graph from candidate anchors
//! Step 3: Score fusion via ScoreFusion

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::SearchIntent;
use crate::search::dimension::{DimensionAdaptor, DimensionQuery};
use crate::search::graph::GraphAdaptor;
use crate::search::lexical::LexicalAdaptor;
use crate::search::vector::VectorAdaptor;

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
    pub node_type: String,
    pub title: Option<String>,
    pub content_preview: String,
}

/// Metadata about the search execution.
#[derive(Debug, Serialize)]
pub struct SearchMeta {
    pub total_results: usize,
    pub lexical_backend: String,
    pub dimensions_used: Vec<String>,
    pub elapsed_ms: u64,
}

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
        let start = std::time::Instant::now();
        let candidate_limit = req.limit * 5; // over-fetch for fusion

        let dim_query = DimensionQuery {
            text: req.query.clone(),
            embedding: req.embedding,
            intent: req.intent,
            session_id: req.session_id,
            node_types: req.node_types,
        };

        // Resolve weights
        let (w_vec, w_lex, w_graph) = {
            let (v, l, g) = match &req.weights {
                Some(w) => (
                    w.vector.unwrap_or(0.65),
                    w.lexical.unwrap_or(0.25),
                    w.graph.unwrap_or(0.10),
                ),
                None => (0.65, 0.25, 0.10),
            };
            let sum = v + l + g;
            if sum > 0.0 {
                (v / sum, l / sum, g / sum)
            } else {
                (0.50, 0.30, 0.20)
            }
        };

        // Step 1: Parallel lexical + vector
        let (mut vec_results, mut lex_results) = tokio::try_join!(
            self.vector
                .search(&self.pool, &dim_query, None, candidate_limit),
            self.lexical
                .search(&self.pool, &dim_query, None, candidate_limit),
        )?;

        // Normalize
        self.vector.normalize_scores(&mut vec_results);
        self.lexical.normalize_scores(&mut lex_results);

        // Collect candidate set
        let mut candidate_set: Vec<Uuid> = vec_results.iter().map(|r| r.node_id).collect();
        for r in &lex_results {
            if !candidate_set.contains(&r.node_id) {
                candidate_set.push(r.node_id);
            }
        }

        // Step 2: Graph from candidates
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

        // Track which dimensions produced results
        let mut dims_used = vec![];
        if !vec_results.is_empty() {
            dims_used.push("vector".to_string());
        }
        if !lex_results.is_empty() {
            dims_used.push("lexical".to_string());
        }
        if !graph_results.is_empty() {
            dims_used.push("graph".to_string());
        }

        // Step 3: Fusion
        // Build per-node score maps
        let mut node_scores: HashMap<Uuid, (Option<f64>, Option<f64>, Option<f64>)> =
            HashMap::new();

        for r in &vec_results {
            node_scores.entry(r.node_id).or_insert((None, None, None)).0 = Some(r.normalized_score);
        }
        for r in &lex_results {
            node_scores.entry(r.node_id).or_insert((None, None, None)).1 = Some(r.normalized_score);
        }
        for r in &graph_results {
            node_scores.entry(r.node_id).or_insert((None, None, None)).2 = Some(r.normalized_score);
        }

        // Fetch node metadata + compute final scores
        let node_ids: Vec<Uuid> = node_scores.keys().cloned().collect();
        if node_ids.is_empty() {
            let meta = SearchMeta {
                total_results: 0,
                lexical_backend: if self.lexical.bm25_available() {
                    "bm25"
                } else {
                    "ts_rank"
                }
                .to_string(),
                dimensions_used: dims_used,
                elapsed_ms: start.elapsed().as_millis() as u64,
            };
            return Ok((vec![], meta));
        }

        // Bulk fetch node info
        let nodes = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                Option<String>,
                String,
                f64,
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            "SELECT id, node_type, title, LEFT(content, 200) AS preview,
                    COALESCE(confidence, 0.5)::float8 AS confidence,
                    modified_at
             FROM covalence.nodes
             WHERE id = ANY($1) AND status = 'active'",
        )
        .bind(&node_ids)
        .fetch_all(&self.pool)
        .await?;

        let node_map: HashMap<Uuid, _> = nodes.into_iter().map(|n| (n.0, n)).collect();

        let mut results: Vec<SearchResult> = Vec::new();
        for (node_id, (vs, ls, gs)) in &node_scores {
            let Some(node) = node_map.get(node_id) else {
                continue;
            };
            let (_, node_type, title, preview, confidence, modified_at) = node;

            // Weighted sum — absent dimensions contribute 0 (no inflation)
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
            let dim_score = weighted_sum;

            // Freshness decay
            let days = (chrono::Utc::now() - modified_at).num_seconds() as f64 / 86400.0;
            let freshness = (-0.01 * days).exp();

            // Final score: dimensional score is dominant, confidence/freshness are tiebreakers
            let final_score = dim_score * 0.85 + *confidence * 0.10 + freshness * 0.05;

            results.push(SearchResult {
                node_id: *node_id,
                score: final_score,
                vector_score: *vs,
                lexical_score: *ls,
                graph_score: *gs,
                confidence: *confidence,
                node_type: node_type.clone(),
                title: title.clone(),
                content_preview: preview.clone(),
            });
        }

        // Sort by final score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(req.limit);

        let meta = SearchMeta {
            total_results: results.len(),
            lexical_backend: if self.lexical.bm25_available() {
                "bm25"
            } else {
                "ts_rank"
            }
            .to_string(),
            dimensions_used: dims_used,
            elapsed_ms: start.elapsed().as_millis() as u64,
        };

        Ok((results, meta))
    }
}
