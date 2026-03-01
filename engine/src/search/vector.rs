//! Vector dimension adaptor — pgvector HNSW cosine similarity (SPEC §7.1).

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};

pub struct VectorAdaptor;

impl VectorAdaptor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DimensionAdaptor for VectorAdaptor {
    fn name(&self) -> &'static str {
        "vector"
    }

    async fn check_availability(&self, pool: &PgPool) -> bool {
        // Check pgvector extension exists
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'vector')"
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false)
    }

    async fn search(
        &self,
        pool: &PgPool,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> anyhow::Result<Vec<DimensionResult>> {
        let embedding = match &query.embedding {
            Some(e) => e,
            None => return Ok(vec![]), // No embedding = skip vector dimension
        };

        // Convert f32 vec to pgvector format string: [0.1,0.2,...]
        let vec_str = format!(
            "[{}]",
            embedding.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")
        );

        // Search both node-level and section-level embeddings, taking best per node.
        // This enables sub-document matching while deduplicating to node level.
        let rows = if let Some(candidates) = candidates {
            sqlx::query_as::<_, (Uuid, f64)>(
                "WITH candidates AS (
                    -- Node-level embeddings
                    SELECT ne.node_id, (ne.embedding::vector <=> $1::vector)::float8 AS distance
                    FROM covalence.node_embeddings ne
                    JOIN covalence.nodes n ON n.id = ne.node_id
                    WHERE n.status = 'active' AND ne.node_id = ANY($2)
                    UNION ALL
                    -- Section-level embeddings (sub-document precision)
                    SELECT ns.node_id, (ns.embedding::vector <=> $1::vector)::float8 AS distance
                    FROM covalence.node_sections ns
                    JOIN covalence.nodes n ON n.id = ns.node_id
                    WHERE n.status = 'active' AND ns.node_id = ANY($2) AND ns.embedding IS NOT NULL
                )
                SELECT node_id, MIN(distance) AS distance
                FROM candidates
                GROUP BY node_id
                ORDER BY distance
                LIMIT $3"
            )
            .bind(&vec_str)
            .bind(candidates)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, f64)>(
                "WITH candidates AS (
                    -- Node-level embeddings
                    SELECT ne.node_id, (ne.embedding::vector <=> $1::vector)::float8 AS distance
                    FROM covalence.node_embeddings ne
                    JOIN covalence.nodes n ON n.id = ne.node_id
                    WHERE n.status = 'active'
                    UNION ALL
                    -- Section-level embeddings (sub-document precision)
                    SELECT ns.node_id, (ns.embedding::vector <=> $1::vector)::float8 AS distance
                    FROM covalence.node_sections ns
                    JOIN covalence.nodes n ON n.id = ns.node_id
                    WHERE n.status = 'active' AND ns.embedding IS NOT NULL
                )
                SELECT node_id, MIN(distance) AS distance
                FROM candidates
                GROUP BY node_id
                ORDER BY distance
                LIMIT $2"
            )
            .bind(&vec_str)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(|(node_id, distance)| DimensionResult {
                node_id,
                raw_score: distance,
                normalized_score: 0.0, // set by normalize_scores
            })
            .collect())
    }

    fn normalize_scores(&self, results: &mut [DimensionResult]) {
        // Cosine distance → similarity: normalized = 1.0 - distance
        for r in results.iter_mut() {
            r.normalized_score = (1.0 - r.raw_score).max(0.0).min(1.0);
        }
    }

    fn estimate_selectivity(&self, _query: &DimensionQuery) -> f64 {
        0.3 // moderate selectivity — HNSW returns approximate top-K
    }

    fn parallelizable(&self) -> bool {
        true
    }
}
