//! Graph dimension adaptor — edge traversal from candidate anchors (SPEC §7.1, §7.2).
//!
//! Runs AFTER lexical+vector (cascade step 2). Traverses typed edges from
//! candidate anchor nodes to discover structurally related nodes.

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};
use crate::models::{EdgeType, SearchIntent};

pub struct GraphAdaptor;

impl Default for GraphAdaptor {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphAdaptor {
    pub fn new() -> Self {
        Self
    }

    /// Intent → priority edge types (SPEC §7.1 table).
    fn priority_edges(intent: Option<&SearchIntent>) -> Vec<String> {
        match intent {
            Some(SearchIntent::Factual) => vec![
                EdgeType::Confirms.as_label().to_string(),
                EdgeType::Originates.as_label().to_string(),
            ],
            Some(SearchIntent::Temporal) => vec![
                EdgeType::Precedes.as_label().to_string(),
                EdgeType::Follows.as_label().to_string(),
            ],
            Some(SearchIntent::Causal) => vec![
                EdgeType::Causes.as_label().to_string(),
                EdgeType::MotivatedBy.as_label().to_string(),
                EdgeType::Implements.as_label().to_string(),
            ],
            Some(SearchIntent::Entity) => vec![EdgeType::Involves.as_label().to_string()],
            None => vec![], // all edges, no filter
        }
    }
}

#[async_trait]
impl DimensionAdaptor for GraphAdaptor {
    fn name(&self) -> &'static str {
        "graph"
    }

    async fn check_availability(&self, pool: &PgPool) -> bool {
        // Check that the edges table exists
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema = 'covalence' AND table_name = 'edges')"
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
        let anchors = match candidates {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(vec![]), // Graph requires anchor nodes from prior dimensions
        };

        let priority = Self::priority_edges(query.intent.as_ref());

        // 1-hop traversal from anchor nodes via SQL edges mirror.
        // Score = edge weight × confidence, optionally boosted for priority edges.
        let rows = if priority.is_empty() {
            // No intent filter — all edges
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT DISTINCT ON (neighbor_id) neighbor_id, score FROM (
                    SELECT
                        CASE WHEN e.source_node_id = ANY($1) THEN e.target_node_id ELSE e.source_node_id END AS neighbor_id,
                        (e.weight * e.confidence)::float8 AS score
                    FROM covalence.edges e
                    WHERE (e.source_node_id = ANY($1) OR e.target_node_id = ANY($1))
                ) sub
                JOIN covalence.nodes n ON n.id = sub.neighbor_id
                WHERE n.status = 'active'
                  AND NOT (sub.neighbor_id = ANY($1))
                ORDER BY neighbor_id, score DESC
                LIMIT $2"
            )
            .bind(anchors)
            .bind(limit as i64)
            .fetch_all(pool)
            .await?
        } else {
            // Intent-filtered: boost priority edge types by 2×
            sqlx::query_as::<_, (Uuid, f64)>(
                "SELECT DISTINCT ON (neighbor_id) neighbor_id, score FROM (
                    SELECT
                        CASE WHEN e.source_node_id = ANY($1) THEN e.target_node_id ELSE e.source_node_id END AS neighbor_id,
                        (e.weight * e.confidence * CASE WHEN e.edge_type = ANY($3) THEN 2.0 ELSE 1.0 END)::float8 AS score
                    FROM covalence.edges e
                    WHERE (e.source_node_id = ANY($1) OR e.target_node_id = ANY($1))
                ) sub
                JOIN covalence.nodes n ON n.id = sub.neighbor_id
                WHERE n.status = 'active'
                  AND NOT (sub.neighbor_id = ANY($1))
                ORDER BY neighbor_id, score DESC
                LIMIT $2"
            )
            .bind(anchors)
            .bind(limit as i64)
            .bind(&priority)
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
        // path_cost normalization: score = 1.0 / (1.0 + path_cost)
        // Since raw_score = weight * confidence (higher = better), invert:
        // normalized = raw_score / max_raw_score (simple ratio normalization)
        if results.is_empty() {
            return;
        }
        let max = results
            .iter()
            .map(|r| r.raw_score)
            .fold(f64::NEG_INFINITY, f64::max);
        if max > 0.0 {
            for r in results.iter_mut() {
                r.normalized_score = r.raw_score / max;
            }
        }
    }

    fn estimate_selectivity(&self, _query: &DimensionQuery) -> f64 {
        0.5 // depends on graph density
    }

    fn parallelizable(&self) -> bool {
        false // must run after lexical+vector (cascade step 2)
    }
}
