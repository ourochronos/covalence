//! Vector search dimension — multi-granularity semantic similarity via pgvector.
//!
//! Searches chunks, nodes, sources, and articles by embedding cosine
//! distance. Each table is queried independently so that a dimension
//! mismatch in one table does not prevent results from the others.

use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::search::SearchResult;

/// Semantic similarity search using pgvector cosine distance.
///
/// Queries the `chunks`, `nodes`, `sources`, and `articles` tables
/// for the closest embeddings to the query vector using the `<=>`
/// (cosine distance) operator. Each table is queried independently
/// so that a dimension mismatch in one table does not prevent
/// results from the others. Results from all granularities are
/// merged and re-ranked.
pub struct VectorDimension {
    pool: PgPool,
}

impl VectorDimension {
    /// Create a new vector search dimension.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Query a single table for embedding similarity, returning
    /// an empty vec (with a warning log) on error rather than
    /// propagating the failure.
    async fn query_table(
        &self,
        table: &str,
        pgvec: &str,
        limit: i64,
    ) -> Vec<(Uuid, f64)> {
        let sql = format!(
            "SELECT id, \
             (1.0 - (embedding <=> $1::halfvec))::float8 AS score \
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
}

impl SearchDimension for VectorDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let Some(ref embedding) = query.embedding else {
            return Ok(Vec::new());
        };

        tracing::debug!(
            embedding_dim = embedding.len(),
            "vector search query embedding"
        );

        // Format embedding as pgvector literal: '[0.1,0.2,...]'
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let limit = query.limit as i64;

        // Query each table independently so a dimension mismatch
        // in one table does not prevent results from others.
        let (chunk_rows, node_rows, source_rows, article_rows) =
            tokio::join!(
                self.query_table("chunks", &pgvec, limit),
                self.query_table("nodes", &pgvec, limit),
                self.query_table("sources", &pgvec, limit),
                self.query_table("articles", &pgvec, limit),
            );

        // Merge and re-rank by score descending.
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
        combined.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        combined.truncate(query.limit);

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
