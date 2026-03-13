//! Vector search dimension — multi-granularity semantic similarity via pgvector.
//!
//! Searches chunks, nodes, sources, and articles by embedding cosine
//! distance. Each table is queried independently with a per-table
//! truncated query embedding to match column dimensions.

use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::config::TableDimensions;
use crate::error::Result;
use crate::ingestion::embedder::truncate_and_validate;
use crate::search::SearchResult;

/// Semantic similarity search using pgvector cosine distance.
///
/// Queries the `chunks`, `nodes`, `sources`, and `articles` tables
/// for the closest embeddings to the query vector using the `<=>`
/// (cosine distance) operator. Each table is queried independently
/// with its own truncated embedding to match the column dimension.
/// Results from all granularities are merged and re-ranked.
pub struct VectorDimension {
    pool: PgPool,
    table_dims: TableDimensions,
}

impl VectorDimension {
    /// Create a new vector search dimension with per-table
    /// embedding dimensions.
    pub fn new(pool: PgPool, table_dims: TableDimensions) -> Self {
        Self { pool, table_dims }
    }

    /// Query a single table for embedding similarity, returning
    /// an empty vec (with a warning log) on error rather than
    /// propagating the failure.
    async fn query_table(&self, table: &str, pgvec: &str, limit: i64) -> Vec<(Uuid, f64)> {
        // Cosine distance: 0 = identical, 1 = orthogonal, 2 = opposite.
        // Convert to similarity in [0, 1] using GREATEST to clamp.
        let sql = format!(
            "SELECT id, \
             GREATEST(0.0, 1.0 - (embedding <=> $1::halfvec))::float8 AS score \
             FROM {table} \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT $2"
        );
        match sqlx::query_as::<_, (Uuid, f64)>(&sql)
            .bind(pgvec)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(
                    table,
                    error = %e,
                    "vector search skipped for table"
                );
                Vec::new()
            }
        }
    }

    /// Hierarchical (coarse-to-fine) search:
    /// 1. Find top-K sources via source embedding similarity
    /// 2. Search summary chunks (L2 then L1) within those sources
    /// 3. Search paragraph/section chunks within those sources
    ///
    /// This ensures chunk results come from sources that are
    /// globally relevant to the query, eliminating "right paragraph,
    /// wrong document" mismatches.
    async fn hierarchical_search(
        &self,
        embedding: &[f64],
        limit: i64,
    ) -> Vec<(Uuid, f64, &str)> {
        let pgvec_source = match self.pgvec_for_table(embedding, "sources") {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let pgvec_chunk = match self.pgvec_for_table(embedding, "chunks") {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        // Stage 1: Find top-K relevant sources (coarse)
        let top_k_sources = 10i64;
        let source_rows = self
            .query_table("sources", &pgvec_source, top_k_sources)
            .await;

        if source_rows.is_empty() {
            return Vec::new();
        }

        let source_ids: Vec<Uuid> = source_rows.iter().map(|(id, _)| *id).collect();
        let source_scores: std::collections::HashMap<Uuid, f64> =
            source_rows.iter().copied().collect();

        // Stage 2: Search chunks gated by source (fine)
        let source_id_list = source_ids
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(",");

        let chunk_sql = format!(
            "SELECT id, source_id, \
             GREATEST(0.0, 1.0 - (embedding <=> $1::halfvec))::float8 AS score \
             FROM chunks \
             WHERE embedding IS NOT NULL \
             AND source_id IN ({source_id_list}) \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT $2"
        );

        let chunk_rows: Vec<(Uuid, Uuid, f64)> = match sqlx::query_as(&chunk_sql)
            .bind(&pgvec_chunk)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "hierarchical chunk search failed"
                );
                Vec::new()
            }
        };

        let mut combined: Vec<(Uuid, f64, &str)> = Vec::new();

        // Include source results
        for (id, score) in &source_rows {
            combined.push((*id, *score, "source"));
        }

        // Include chunk results with parent boost:
        // If a chunk's source scores highly, slightly boost the chunk.
        for (chunk_id, source_id, chunk_score) in &chunk_rows {
            let parent_boost = source_scores
                .get(source_id)
                .map(|s| s * 0.1)
                .unwrap_or(0.0);
            combined.push((*chunk_id, chunk_score + parent_boost, "chunk"));
        }

        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(limit as usize);
        combined
    }

    /// Format an embedding as a pgvector literal, truncated to
    /// the target dimension for the given table.
    ///
    /// Returns an error if the embedding contains non-finite values
    /// (NaN, Inf) rather than silently formatting them into an
    /// invalid pgvector literal.
    fn pgvec_for_table(&self, embedding: &[f64], table: &str) -> Result<String> {
        let target_dim = match table {
            "chunks" => self.table_dims.chunk,
            "nodes" => self.table_dims.node,
            "sources" => self.table_dims.source,
            "articles" => self.table_dims.article,
            _ => self.table_dims.chunk,
        };
        let truncated = truncate_and_validate(embedding, target_dim, table)?;
        Ok(format!(
            "[{}]",
            truncated
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ))
    }
}

impl SearchDimension for VectorDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let Some(ref embedding) = query.embedding else {
            return Ok(Vec::new());
        };

        tracing::debug!(
            embedding_dim = embedding.len(),
            hierarchical = query.hierarchical,
            "vector search query embedding"
        );

        let limit = query.limit as i64;

        // Hierarchical mode: coarse-to-fine source-gated search.
        let combined = if query.hierarchical {
            tracing::debug!("using hierarchical coarse-to-fine vector search");
            let (node_limit, article_limit) = (
                (limit / 4).max(2),
                (limit / 8).max(2),
            );
            let pgvec_node = self.pgvec_for_table(embedding, "nodes")?;
            let pgvec_article = self.pgvec_for_table(embedding, "articles")?;

            // Run hierarchical search alongside node/article search.
            let (hier_results, node_rows, article_rows) = tokio::join!(
                self.hierarchical_search(embedding, limit),
                self.query_table("nodes", &pgvec_node, node_limit),
                self.query_table("articles", &pgvec_article, article_limit),
            );

            let mut combined = hier_results;
            for (id, score) in node_rows {
                combined.push((id, score, "node"));
            }
            for (id, score) in article_rows {
                combined.push((id, score, "article"));
            }
            combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            combined.truncate(query.limit);
            combined
        } else {
            // Standard flat search across all tables.
            let (chunk_limit, node_limit, source_limit, article_limit) = per_table_limits(limit);

            let pgvec_chunk = self.pgvec_for_table(embedding, "chunks")?;
            let pgvec_node = self.pgvec_for_table(embedding, "nodes")?;
            let pgvec_source = self.pgvec_for_table(embedding, "sources")?;
            let pgvec_article = self.pgvec_for_table(embedding, "articles")?;

            let (chunk_rows, node_rows, source_rows, article_rows) = tokio::join!(
                self.query_table("chunks", &pgvec_chunk, chunk_limit),
                self.query_table("nodes", &pgvec_node, node_limit),
                self.query_table("sources", &pgvec_source, source_limit),
                self.query_table("articles", &pgvec_article, article_limit),
            );

            let mut combined: Vec<(Uuid, f64, &str)> = Vec::new();
            for (id, score) in chunk_rows {
                combined.push((id, score, "chunk"));
            }
            for (id, score) in source_rows {
                combined.push((id, score, "source"));
            }
            for (id, score) in node_rows {
                combined.push((id, score, "node"));
            }
            for (id, score) in article_rows {
                combined.push((id, score, "article"));
            }
            combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            combined.truncate(query.limit);
            combined
        };

        let results = combined
            .into_iter()
            .enumerate()
            .map(|(i, (id, score, rtype))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "vector".to_string(),
                snippet: None,
                result_type: Some(rtype.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Vector
    }
}

/// Compute per-table limits from a total limit budget.
///
/// Chunks get half, nodes a quarter, sources and articles an eighth
/// each. Every table gets at least 2 results.
pub fn per_table_limits(total: i64) -> (i64, i64, i64, i64) {
    let chunk = (total / 2).max(2);
    let node = (total / 4).max(2);
    let source = (total / 8).max(2);
    let article = (total / 8).max(2);
    (chunk, node, source, article)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_table_limits_default() {
        let (chunk, node, source, article) = per_table_limits(20);
        assert_eq!(chunk, 10);
        assert_eq!(node, 5);
        assert_eq!(source, 2);
        assert_eq!(article, 2);
    }

    #[test]
    fn per_table_limits_small_budget() {
        // With limit=1, every table should get at least 2
        let (chunk, node, source, article) = per_table_limits(1);
        assert_eq!(chunk, 2);
        assert_eq!(node, 2);
        assert_eq!(source, 2);
        assert_eq!(article, 2);
    }

    #[test]
    fn per_table_limits_large_budget() {
        let (chunk, node, source, article) = per_table_limits(100);
        assert_eq!(chunk, 50);
        assert_eq!(node, 25);
        assert_eq!(source, 12);
        assert_eq!(article, 12);
    }

    #[test]
    fn result_merge_ordering() {
        // Verify that combined results sort by score descending
        let mut combined: Vec<(Uuid, f64, &str)> = vec![
            (Uuid::nil(), 0.5, "chunk"),
            (Uuid::nil(), 0.9, "source"),
            (Uuid::nil(), 0.7, "node"),
            (Uuid::nil(), 0.3, "article"),
        ];
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        assert!((combined[0].1 - 0.9).abs() < f64::EPSILON);
        assert!((combined[1].1 - 0.7).abs() < f64::EPSILON);
        assert!((combined[2].1 - 0.5).abs() < f64::EPSILON);
        assert!((combined[3].1 - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn result_merge_truncation() {
        let mut combined: Vec<(Uuid, f64, &str)> = (0..10)
            .map(|i| (Uuid::nil(), i as f64 * 0.1, "chunk"))
            .collect();
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        combined.truncate(3);
        assert_eq!(combined.len(), 3);
        assert!((combined[0].1 - 0.9).abs() < f64::EPSILON);
    }
}
