//! Lexical search dimension — multi-granularity full-text search
//! via tsvector.
//!
//! Searches chunks, nodes, and articles using PostgreSQL full-text
//! search with trigram fallback for chunks. Chunk results include
//! `ts_headline` snippets highlighting the matched terms.

use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::search::SearchResult;

/// Maximum snippet length for `ts_headline` output.
const SNIPPET_MAX_WORDS: i32 = 35;

/// Full-text search using PostgreSQL tsvector with fallback
/// strategies.
///
/// Searches across chunks, nodes, and articles at multiple
/// granularities. For chunks, tries `plainto_tsquery` first, then
/// `websearch_to_tsquery`, then trigram ILIKE as a last resort.
/// Node search matches against `name_tsv` (canonical name +
/// description). Article search matches against `body_tsv`
/// (title + body). Results from all granularities are merged
/// and re-ranked by score.
///
/// Chunk results include `ts_headline` snippets with highlighted
/// matching terms.
pub struct LexicalDimension {
    pool: PgPool,
}

impl LexicalDimension {
    /// Create a new lexical search dimension.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Search nodes via `name_tsv` (canonical name + description).
    async fn search_nodes(&self, query: &SearchQuery) -> Result<Vec<(Uuid, f64)>> {
        let rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, \
                    ts_rank_cd(name_tsv, query)::float8 AS score \
             FROM nodes, \
                  plainto_tsquery('english', $1) query \
             WHERE name_tsv @@ query \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Search articles via `body_tsv` (title + body).
    async fn search_articles(&self, query: &SearchQuery) -> Result<Vec<(Uuid, f64)>> {
        let rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, \
                    ts_rank_cd(body_tsv, query)::float8 AS score \
             FROM articles, \
                  plainto_tsquery('english', $1) query \
             WHERE body_tsv @@ query \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Search chunks via tsvector with fallback strategies.
    ///
    /// Returns results with snippets from `ts_headline` when using
    /// the tsvector strategies. Trigram fallback returns a truncated
    /// content prefix instead.
    async fn search_chunks(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        // Primary: plainto_tsquery with ts_headline snippets.
        let rows = sqlx::query_as::<_, (Uuid, f64, String)>(
            "SELECT c.id, \
                    ts_rank_cd(c.content_tsv, q)::float8 AS score, \
                    ts_headline('english', c.content, q, \
                        'MaxWords=' || $3 || \
                        ', MinWords=15, ShortWord=3') \
                        AS snippet \
             FROM chunks c, \
                  plainto_tsquery('english', $1) q \
             WHERE c.content_tsv @@ q \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .bind(SNIPPET_MAX_WORDS)
        .fetch_all(&self.pool)
        .await?;

        if !rows.is_empty() {
            return Ok(to_results_with_snippets(rows));
        }

        // Fallback: websearch_to_tsquery with ts_headline.
        let rows = sqlx::query_as::<_, (Uuid, f64, String)>(
            "SELECT c.id, \
                    ts_rank_cd(c.content_tsv, q)::float8 AS score, \
                    ts_headline('english', c.content, q, \
                        'MaxWords=' || $3 || \
                        ', MinWords=15, ShortWord=3') \
                        AS snippet \
             FROM chunks c, \
                  websearch_to_tsquery('english', $1) q \
             WHERE c.content_tsv @@ q \
             ORDER BY score DESC \
             LIMIT $2",
        )
        .bind(&query.text)
        .bind(query.limit as i64)
        .bind(SNIPPET_MAX_WORDS)
        .fetch_all(&self.pool)
        .await?;

        if !rows.is_empty() {
            return Ok(to_results_with_snippets(rows));
        }

        // Last resort: trigram ILIKE (no ts_headline available,
        // use content prefix as snippet).
        let pattern = format!("%{}%", query.text);
        let rows = sqlx::query_as::<_, (Uuid, f64, String)>(
            "SELECT id, \
                    similarity(content, $1)::float8 AS score, \
                    left(content, 200) AS snippet \
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

        Ok(to_results_with_snippets(rows))
    }
}

impl SearchDimension for LexicalDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        if query.text.is_empty() {
            return Ok(Vec::new());
        }

        // Run all three sub-searches concurrently.
        let (chunk_results, node_rows, article_rows) = tokio::join!(
            self.search_chunks(query),
            self.search_nodes(query),
            self.search_articles(query),
        );
        let chunk_results = chunk_results?;
        let node_rows = node_rows?;
        let article_rows = article_rows?;

        // Merge all results and re-rank by score descending.
        let mut combined: Vec<(Uuid, f64, &str, Option<String>)> = Vec::new();
        for r in &chunk_results {
            combined.push((r.id, r.score, "chunk", r.snippet.clone()));
        }
        for (id, score) in &node_rows {
            combined.push((*id, *score, "node", None));
        }
        for (id, score) in &article_rows {
            combined.push((*id, *score, "article", None));
        }
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(query.limit);

        let results = combined
            .into_iter()
            .enumerate()
            .map(|(i, (id, score, rtype, snippet))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "lexical".to_string(),
                snippet,
                result_type: Some(rtype.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Lexical
    }
}

/// Convert `(id, score, snippet)` rows into ranked `SearchResult`
/// items with snippets.
fn to_results_with_snippets(rows: Vec<(Uuid, f64, String)>) -> Vec<SearchResult> {
    rows.into_iter()
        .enumerate()
        .map(|(i, (id, score, snippet))| SearchResult {
            id,
            score,
            rank: i + 1,
            dimension: "lexical".to_string(),
            snippet: Some(snippet),
            result_type: Some("chunk".to_string()),
        })
        .collect()
}
