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

    /// Format an embedding as a pgvector literal, truncated to
    /// the target dimension for the given table.
    fn pgvec_for_table(&self, embedding: &[f64], table: &str) -> String {
        let target_dim = match table {
            "chunks" => self.table_dims.chunk,
            "nodes" => self.table_dims.node,
            "sources" => self.table_dims.source,
            "articles" => self.table_dims.article,
            _ => self.table_dims.chunk,
        };
        // Use truncate_and_validate; if validation fails
        // (should never happen), fall back to raw truncation.
        let truncated = truncate_and_validate(embedding, target_dim, table)
            .unwrap_or_else(|_| embedding[..target_dim.min(embedding.len())].to_vec());
        format!(
            "[{}]",
            truncated
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
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

        let limit = query.limit as i64;

        // Per-table limits to prevent any single table from crowding
        // out others. Chunks get half the budget (primary content),
        // nodes get a quarter, sources and articles share the rest.
        // Each table gets at least 2 results so nothing is starved.
        let chunk_limit = (limit / 2).max(2);
        let node_limit = (limit / 4).max(2);
        let source_limit = (limit / 8).max(2);
        let article_limit = (limit / 8).max(2);

        // Format per-table pgvec literals (truncated to column dim).
        let pgvec_chunk = self.pgvec_for_table(embedding, "chunks");
        let pgvec_node = self.pgvec_for_table(embedding, "nodes");
        let pgvec_source = self.pgvec_for_table(embedding, "sources");
        let pgvec_article = self.pgvec_for_table(embedding, "articles");

        // Query each table independently so a dimension mismatch
        // in one table does not prevent results from others.
        let (chunk_rows, node_rows, source_rows, article_rows) = tokio::join!(
            self.query_table("chunks", &pgvec_chunk, chunk_limit),
            self.query_table("nodes", &pgvec_node, node_limit),
            self.query_table("sources", &pgvec_source, source_limit),
            self.query_table("articles", &pgvec_article, article_limit),
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
        combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
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
