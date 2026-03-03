//! Structural dimension adaptor — graph-embedding cosine similarity (covalence#52).
//!
//! Runs AFTER vector+lexical (cascade step 3). For each anchor candidate from
//! prior dimensions, looks up its embedding in `graph_embeddings` (preferring
//! `node2vec`, falling back to `spectral`) and finds the top-K most
//! structurally similar nodes via pgvector cosine distance (`<=>`).
//!
//! ## Feature flag
//!
//! This adaptor is gated by `COVALENCE_STRUCTURAL_SEARCH=true`. When the env
//! var is absent or not `"true"`, `search()` returns immediately with an empty
//! result set so it has zero cost on deployments without graph embeddings.
//!
//! ## Score normalization
//!
//! Raw similarity = `1.0 − cosine_distance` ∈ [−1, 1] (pgvector can return
//! slightly negative values for nearly-orthogonal vectors). Scores are
//! min-max normalized to [0, 1] before fusion.

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};

pub struct StructuralAdaptor;

impl Default for StructuralAdaptor {
    fn default() -> Self {
        Self::new()
    }
}

impl StructuralAdaptor {
    pub fn new() -> Self {
        Self
    }

    /// Returns `true` when the structural search feature flag is active.
    fn is_enabled() -> bool {
        std::env::var("COVALENCE_STRUCTURAL_SEARCH").as_deref() == Ok("true")
    }

    /// Find the top-`limit` nodes most similar to `anchor_id` using the
    /// specified embedding `method` (`node2vec` or `spectral`).
    async fn similar_by_method(
        pool: &PgPool,
        anchor_id: Uuid,
        method: &str,
        limit: usize,
    ) -> Vec<(Uuid, f64)> {
        sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT ge2.node_id,
                    (1.0 - (ge2.embedding <=> ge1.embedding))::float8 AS similarity
             FROM   covalence.graph_embeddings ge1
             JOIN   covalence.graph_embeddings ge2 ON ge1.method = ge2.method
             WHERE  ge1.node_id = $1
               AND  ge2.node_id != $1
               AND  ge1.method  = $2
             ORDER  BY ge2.embedding <=> ge1.embedding
             LIMIT  $3",
        )
        .bind(anchor_id)
        .bind(method)
        .bind(limit as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
    }
}

#[async_trait]
impl DimensionAdaptor for StructuralAdaptor {
    fn name(&self) -> &'static str {
        "structural"
    }

    /// Returns `true` when at least one row exists in `graph_embeddings`.
    async fn check_availability(&self, pool: &PgPool) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM covalence.graph_embeddings")
            .fetch_one(pool)
            .await
            .map(|c| c > 0)
            .unwrap_or(false)
    }

    /// Find structurally similar nodes for all anchor candidates.
    ///
    /// * Returns immediately (empty) when `COVALENCE_STRUCTURAL_SEARCH != "true"`.
    /// * Requires `candidates` to be non-empty (cascade step: anchors from prior dims).
    /// * Prefers `node2vec` embeddings; falls back to `spectral` per-anchor.
    /// * Merges results across anchors, keeping the highest similarity per node.
    async fn search(
        &self,
        pool: &PgPool,
        _query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> anyhow::Result<Vec<DimensionResult>> {
        // Feature-flag gate — zero cost when disabled.
        if !Self::is_enabled() {
            return Ok(vec![]);
        }

        let anchors = match candidates {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(vec![]), // structural requires anchor nodes from prior dims
        };

        // best[node_id] = highest similarity across all anchor queries
        let mut best: HashMap<Uuid, f64> = HashMap::new();

        for &anchor_id in anchors {
            // Prefer node2vec; fall back to spectral if no node2vec entry exists.
            let mut rows = Self::similar_by_method(pool, anchor_id, "node2vec", limit).await;
            if rows.is_empty() {
                rows = Self::similar_by_method(pool, anchor_id, "spectral", limit).await;
            }

            for (node_id, similarity) in rows {
                let entry = best.entry(node_id).or_insert(similarity);
                if similarity > *entry {
                    *entry = similarity;
                }
            }
        }

        let mut results: Vec<DimensionResult> = best
            .into_iter()
            .map(|(node_id, similarity)| DimensionResult {
                node_id,
                raw_score: similarity,
                normalized_score: 0.0, // set by normalize_scores
                hop: None,
            })
            .collect();

        // Sort by raw score descending before truncating.
        results.sort_by(|a, b| {
            b.raw_score
                .partial_cmp(&a.raw_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    /// Min-max normalization to [0.0, 1.0].
    fn normalize_scores(&self, results: &mut [DimensionResult]) {
        if results.is_empty() {
            return;
        }
        let min = results
            .iter()
            .map(|r| r.raw_score)
            .fold(f64::INFINITY, f64::min);
        let max = results
            .iter()
            .map(|r| r.raw_score)
            .fold(f64::NEG_INFINITY, f64::max);
        let range = max - min;
        for r in results.iter_mut() {
            r.normalized_score = if range > 0.0 {
                (r.raw_score - min) / range
            } else {
                1.0 // all scores identical → all maximum
            };
        }
    }

    fn estimate_selectivity(&self, _query: &DimensionQuery) -> f64 {
        0.4 // depends on embedding coverage in the graph
    }

    fn parallelizable(&self) -> bool {
        false // cascade: runs after vector+lexical
    }
}
