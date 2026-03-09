//! Vector search dimension — multi-granularity semantic similarity via pgvector.
//!
//! Searches chunks, nodes, and articles by embedding cosine distance.

use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::search::SearchResult;

/// Semantic similarity search using pgvector cosine distance.
///
/// Queries the `chunks`, `nodes`, and `articles` tables for the closest
/// embeddings to the query vector using the `<=>` (cosine distance)
/// operator. Results from all granularities are merged and re-ranked.
pub struct VectorDimension {
    pool: PgPool,
}

impl VectorDimension {
    /// Create a new vector search dimension.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SearchDimension for VectorDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let Some(ref embedding) = query.embedding else {
            return Ok(Vec::new());
        };

        // Format embedding as pgvector literal: '[0.1,0.2,...]'
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        // Search chunks by embedding similarity.
        let chunk_rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, 1.0 - (embedding <=> $1::halfvec) AS score \
             FROM chunks \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT $2",
        )
        .bind(&pgvec)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        // Search nodes by embedding similarity.
        let node_rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, 1.0 - (embedding <=> $1::halfvec) AS score \
             FROM nodes \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT $2",
        )
        .bind(&pgvec)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        // Search articles by embedding similarity.
        let article_rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, \
             1.0 - (embedding <=> $1::halfvec) AS score \
             FROM articles \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT $2",
        )
        .bind(&pgvec)
        .bind(query.limit as i64)
        .fetch_all(&self.pool)
        .await?;

        // Merge and re-rank by score descending.
        let mut combined: Vec<(Uuid, f64, &str)> = Vec::new();
        for (id, score) in chunk_rows {
            combined.push((id, score, "chunk"));
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
