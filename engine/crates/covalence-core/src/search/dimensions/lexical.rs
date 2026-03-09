//! Lexical search dimension — multi-granularity full-text search via tsvector.
//!
//! Searches chunks, nodes, and articles using PostgreSQL full-text search
//! with trigram fallback for chunks.

use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::search::SearchResult;

/// Full-text search using PostgreSQL tsvector with fallback strategies.
///
/// Searches across chunks, nodes, and articles at multiple granularities.
/// For chunks, tries `plainto_tsquery` first, then `websearch_to_tsquery`,
/// then trigram ILIKE as a last resort. Node search matches against
/// `name_tsv` (canonical name + description). Article search matches
/// against `body_tsv` (title + body). Results from all granularities
/// are merged and re-ranked by score.
pub struct LexicalDimension {
    pool: PgPool,
}

impl LexicalDimension {
    /// Create a new lexical search dimension.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Search chunks via tsvector with fallback strategies.
    async fn search_chunks(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        // Primary: plainto_tsquery
        let rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, ts_rank_cd(content_tsv, query) AS score \
             FROM chunks, plainto_tsquery('english', $1) query \
             WHERE content_tsv @@ query \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        if !rows.is_empty() {
            return Ok(to_results(rows));
        }

        // Fallback: websearch_to_tsquery
        let rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, ts_rank_cd(content_tsv, query) AS score \
             FROM chunks, websearch_to_tsquery('english', $1) query \
             WHERE content_tsv @@ query \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        if !rows.is_empty() {
            return Ok(to_results(rows));
        }

        // Last resort: trigram ILIKE
        let pattern = format!("%{}%", query.text);
        let rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, similarity(content, $1)::float8 AS score \
             FROM chunks \
             WHERE content ILIKE $2 \
             ORDER BY score DESC \
             LIMIT $3",
        )
        .bind(&query.text)
        .bind(&pattern)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(to_results(rows))
    }
}

impl SearchDimension for LexicalDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        if query.text.is_empty() {
            return Ok(Vec::new());
        }

        // Search chunks via tsvector.
        let chunk_results = self.search_chunks(query).await?;

        // Search nodes via name_tsv (covers canonical_name + description).
        let node_rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, ts_rank_cd(name_tsv, query) AS score \
             FROM nodes, plainto_tsquery('english', $1) query \
             WHERE name_tsv @@ query \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        // Search articles via body_tsv (covers title + body).
        let article_rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, ts_rank_cd(body_tsv, query) AS score \
             FROM articles, plainto_tsquery('english', $1) query \
             WHERE body_tsv @@ query \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        // Merge all results and re-rank by score descending.
        let mut combined: Vec<(Uuid, f64, &str)> = Vec::new();
        for r in &chunk_results {
            combined.push((r.id, r.score, "chunk"));
        }
        for (id, score) in &node_rows {
            combined.push((*id, *score, "node"));
        }
        for (id, score) in &article_rows {
            combined.push((*id, *score, "article"));
        }
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(query.limit);

        let results = combined
            .into_iter()
            .enumerate()
            .map(|(i, (id, score, rtype))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "lexical".to_string(),
                snippet: None,
                result_type: Some(rtype.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Lexical
    }
}

/// Convert `(id, score)` rows into ranked `SearchResult` items.
///
/// Used internally for chunk-only results from fallback strategies.
fn to_results(rows: Vec<(Uuid, f64)>) -> Vec<SearchResult> {
    rows.into_iter()
        .enumerate()
        .map(|(i, (id, score))| SearchResult {
            id,
            score,
            rank: i + 1,
            dimension: "lexical".to_string(),
            snippet: None,
            result_type: Some("chunk".to_string()),
        })
        .collect()
}
