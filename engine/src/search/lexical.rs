//! Lexical dimension adaptor — ts_rank with pg_textsearch BM25 fallback (SPEC §7.4).

use async_trait::async_trait;
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};

pub struct LexicalAdaptor {
    bm25_available: AtomicBool,
}

impl LexicalAdaptor {
    pub fn new() -> Self {
        Self {
            bm25_available: AtomicBool::new(false),
        }
    }

    pub fn bm25_available(&self) -> bool {
        self.bm25_available.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl DimensionAdaptor for LexicalAdaptor {
    fn name(&self) -> &'static str {
        "lexical"
    }

    async fn check_availability(&self, pool: &PgPool) -> bool {
        // Check if pg_textsearch (BM25) is available
        let bm25 = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'pg_textsearch')",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        self.bm25_available.store(bm25, Ordering::Relaxed);
        if bm25 {
            tracing::info!("pg_textsearch BM25 available — using BM25 scoring");
        } else {
            tracing::info!("pg_textsearch not available — falling back to ts_rank");
        }

        // Always available — ts_rank is built-in
        true
    }

    async fn search(
        &self,
        pool: &PgPool,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> anyhow::Result<Vec<DimensionResult>> {
        if query.text.is_empty() {
            return Ok(vec![]);
        }

        // Always use ts_rank for v0 (BM25 is preview quality)
        let rows = if let Some(candidates) = candidates {
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, ts_rank(content_tsv, websearch_to_tsquery('english', $1))::float8 AS score
                 FROM covalence.nodes
                 WHERE content_tsv @@ websearch_to_tsquery('english', $1)
                   AND status = 'active'
                   AND id = ANY($2)
                 ORDER BY score DESC
                 LIMIT $3"
            )
            .bind(&query.text)
            .bind(candidates)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, ts_rank(content_tsv, websearch_to_tsquery('english', $1))::float8 AS score
                 FROM covalence.nodes
                 WHERE content_tsv @@ websearch_to_tsquery('english', $1)
                   AND status = 'active'
                 ORDER BY score DESC
                 LIMIT $2"
            )
            .bind(&query.text)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(|(node_id, score)| DimensionResult {
                node_id,
                raw_score: score,
                normalized_score: 0.0,
            })
            .collect())
    }

    fn normalize_scores(&self, results: &mut [DimensionResult]) {
        if results.is_empty() {
            return;
        }
        // Min-max normalization within result set
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
                1.0 // all same score → all maximum
            };
        }
    }

    fn estimate_selectivity(&self, _query: &DimensionQuery) -> f64 {
        0.2 // relatively selective — requires keyword match
    }

    fn parallelizable(&self) -> bool {
        true
    }
}
