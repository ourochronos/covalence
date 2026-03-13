//! Global / community summary search dimension.
//!
//! Searches community summary embeddings and section embeddings
//! for thematic and global queries. Handles broad questions that
//! can't be answered by any single chunk — they require synthesizing
//! across the corpus.
//!
//! Falls back to article embeddings when no community summary
//! nodes are available (articles are compiled from communities,
//! so they serve as an approximation).

use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::config::TableDimensions;
use crate::error::Result;
use crate::ingestion::embedder::truncate_and_validate;
use crate::search::SearchResult;

/// Community summary search dimension.
///
/// Queries community summary nodes (and optionally articles)
/// by embedding cosine distance. This is the 6th search
/// dimension, targeting broad thematic queries that span
/// multiple communities or topics.
pub struct GlobalDimension {
    pool: PgPool,
    table_dims: TableDimensions,
}

impl GlobalDimension {
    /// Create a new global search dimension with per-table
    /// embedding dimensions for query truncation.
    pub fn new(pool: PgPool, table_dims: TableDimensions) -> Self {
        Self { pool, table_dims }
    }

    /// Format an embedding as a pgvector literal, truncated to
    /// the target dimension for the given table.
    ///
    /// Returns an error if the embedding contains non-finite values
    /// rather than silently formatting an invalid pgvector literal.
    fn pgvec_for_table(&self, embedding: &[f64], table: &str) -> Result<String> {
        let target_dim = match table {
            "nodes" => self.table_dims.node,
            "articles" => self.table_dims.article,
            "sections" => self.table_dims.chunk, // sections use chunk dim
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

impl SearchDimension for GlobalDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let Some(ref embedding) = query.embedding else {
            return Ok(Vec::new());
        };

        let limit = query.limit as i64;

        // Truncate query embedding per-table to match column dims.
        let pgvec_nodes = self.pgvec_for_table(embedding, "nodes")?;
        let pgvec_articles = self.pgvec_for_table(embedding, "articles")?;
        let pgvec_sections = self.pgvec_for_table(embedding, "sections")?;

        // Run community summaries + sections concurrently.
        let section_limit = (limit / 2).max(2);
        let summary_limit = limit - section_limit;
        let (summary_rows, section_rows) = tokio::join!(
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, \
                 GREATEST(0.0, 1.0 - (embedding <=> $1::halfvec)) AS score \
                 FROM nodes \
                 WHERE embedding IS NOT NULL \
                   AND node_type = 'community_summary' \
                 ORDER BY embedding <=> $1::halfvec \
                 LIMIT $2",
            )
            .bind(&pgvec_nodes)
            .bind(summary_limit)
            .fetch_all(&self.pool),
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT id, \
                 GREATEST(0.0, 1.0 - (embedding <=> $1::halfvec)) AS score \
                 FROM sections \
                 WHERE embedding IS NOT NULL \
                 ORDER BY embedding <=> $1::halfvec \
                 LIMIT $2",
            )
            .bind(&pgvec_sections)
            .bind(section_limit)
            .fetch_all(&self.pool),
        );
        let summary_rows = summary_rows?;
        let section_rows = section_rows?;

        // Merge community summaries and sections.
        let mut combined: Vec<(Uuid, f64, &str)> = Vec::new();
        for (id, score) in &summary_rows {
            combined.push((*id, *score, "node"));
        }
        for (id, score) in &section_rows {
            combined.push((*id, *score, "section"));
        }

        // If we have results from summaries or sections, use them.
        if !combined.is_empty() {
            combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            combined.truncate(limit as usize);
            return Ok(combined
                .into_iter()
                .enumerate()
                .map(|(i, (id, score, rtype))| SearchResult {
                    id,
                    score,
                    rank: i + 1,
                    dimension: "global".to_string(),
                    snippet: None,
                    result_type: Some(rtype.to_string()),
                })
                .collect());
        }

        // Fall back to articles (compiled from communities).
        let article_rows = sqlx::query_as::<_, (Uuid, f64)>(
            "SELECT id, \
             GREATEST(0.0, 1.0 - (embedding <=> $1::halfvec)) AS score \
             FROM articles \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT $2",
        )
        .bind(&pgvec_articles)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(article_rows
            .into_iter()
            .enumerate()
            .map(|(i, (id, score))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "global".to_string(),
                snippet: None,
                result_type: Some("article".to_string()),
            })
            .collect())
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Global
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_dimension_kind() {
        // We can't construct without a real pool, but we can
        // verify the kind method via the trait at the type level.
        assert_eq!(DimensionKind::Global.to_string(), "global");
    }
}
