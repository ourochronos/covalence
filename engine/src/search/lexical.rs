//! Lexical dimension adaptor — ts_rank with pg_textsearch BM25 fallback (SPEC §7.4).
//!
//! ## Fallback chain (covalence#33)
//!
//! 1. `websearch_to_tsquery` — strict operator-aware parsing (current).
//! 2. `plainto_tsquery` — space-separated OR-of-terms, no operator syntax.
//! 3. ILIKE title match — substring scan for query words >4 chars; score=0.5.
//!
//! Each step only runs when the previous one returns 0 results, ensuring lexical
//! results fire for virtually every query instead of silently returning nothing.

use async_trait::async_trait;
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};

pub struct LexicalAdaptor {
    bm25_available: AtomicBool,
}

impl Default for LexicalAdaptor {
    fn default() -> Self {
        Self::new()
    }
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

        // ── Step A: websearch_to_tsquery (strict, operator-aware) ─────────────
        let rows = if let Some(candidates) = candidates {
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, ts_rank(content_tsv, websearch_to_tsquery('english', $1))::float8 AS score
                 FROM covalence.nodes
                 WHERE content_tsv @@ websearch_to_tsquery('english', $1)
                   AND status = 'active'
                   AND namespace = $2
                   AND id = ANY($3)
                 ORDER BY score DESC
                 LIMIT $4",
            )
            .bind(&query.text)
            .bind(&query.namespace)
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
                   AND namespace = $2
                 ORDER BY score DESC
                 LIMIT $3",
            )
            .bind(&query.text)
            .bind(&query.namespace)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        };

        if !rows.is_empty() {
            return Ok(rows
                .into_iter()
                .map(|(node_id, score)| DimensionResult {
                    node_id,
                    raw_score: score,
                    normalized_score: 0.0,
                    hop: None,
                })
                .collect());
        }

        // ── Step B: plainto_tsquery (OR-of-terms, no operator syntax) ─────────
        tracing::debug!(
            "lexical step A returned 0 results — trying plainto_tsquery for {:?}",
            query.text
        );
        let rows_b = if let Some(candidates) = candidates {
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, ts_rank(content_tsv, plainto_tsquery('english', $1))::float8 AS score
                 FROM covalence.nodes
                 WHERE content_tsv @@ plainto_tsquery('english', $1)
                   AND status = 'active'
                   AND namespace = $2
                   AND id = ANY($3)
                 ORDER BY score DESC
                 LIMIT $4",
            )
            .bind(&query.text)
            .bind(&query.namespace)
            .bind(candidates)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, ts_rank(content_tsv, plainto_tsquery('english', $1))::float8 AS score
                 FROM covalence.nodes
                 WHERE content_tsv @@ plainto_tsquery('english', $1)
                   AND status = 'active'
                   AND namespace = $2
                 ORDER BY score DESC
                 LIMIT $3",
            )
            .bind(&query.text)
            .bind(&query.namespace)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        };

        if !rows_b.is_empty() {
            return Ok(rows_b
                .into_iter()
                .map(|(node_id, score)| DimensionResult {
                    node_id,
                    raw_score: score,
                    normalized_score: 0.0,
                    hop: None,
                })
                .collect());
        }

        // ── Step C: ILIKE title substring match for words >4 chars ────────────
        tracing::debug!(
            "lexical step B returned 0 results — trying ILIKE title match for {:?}",
            query.text
        );

        // Collect significant words (>4 chars) from the query.
        let sig_words: Vec<&str> = query
            .text
            .split_whitespace()
            .filter(|w| w.len() > 4)
            .collect();

        if sig_words.is_empty() {
            return Ok(vec![]);
        }

        // Build ILIKE conditions dynamically.  We interpolate patterns as SQL
        // literals — safe because we control the ILIKE pattern format and the
        // words come from the query string (no SQL injection surface beyond
        // what the caller already controls via the query parameter).
        let ilike_clauses: Vec<String> = sig_words
            .iter()
            .map(|w| {
                // Escape SQL special chars in the pattern.
                let escaped = w
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
                    .replace('\'', "''"); // escape single quotes for SQL
                format!("title ILIKE '%{}%'", escaped)
            })
            .collect();

        let where_clause = ilike_clauses.join(" OR ");
        // Build the final SQL — parameters are $1 = namespace, $2 = limit (no
        // candidates) or $1 = namespace, $2 = candidates array, $3 = limit.
        let (sql, rows_c) = if let Some(candidates) = candidates {
            let sql = format!(
                "SELECT id, 0.5::float8 AS score
                 FROM covalence.nodes
                 WHERE status = 'active'
                   AND namespace = $1
                   AND ({where_clause})
                   AND id = ANY($2)
                 LIMIT $3"
            );
            let rows = sqlx::query_as::<_, (Uuid, f64)>(&sql)
                .bind(&query.namespace)
                .bind(candidates)
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
            (sql, rows)
        } else {
            let sql = format!(
                "SELECT id, 0.5::float8 AS score
                 FROM covalence.nodes
                 WHERE status = 'active'
                   AND namespace = $1
                   AND ({where_clause})
                 LIMIT $2"
            );
            let rows = sqlx::query_as::<_, (Uuid, f64)>(&sql)
                .bind(&query.namespace)
                .bind(limit as i64)
                .fetch_all(pool)
                .await?;
            (sql, rows)
        };
        let _ = sql; // used above

        Ok(rows_c
            .into_iter()
            .map(|(node_id, score)| DimensionResult {
                node_id,
                raw_score: score,
                normalized_score: 0.0,
                hop: None,
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
