//! Admin stats and maintenance operations (SPEC §5.4).

use sqlx::{PgPool, Row};

use crate::errors::*;
use crate::graph::{AgeGraphRepository, GraphRepository};

#[derive(Debug, serde::Serialize)]
pub struct StatsResponse {
    pub nodes: NodeStats,
    pub edges: EdgeStats,
    pub queue: QueueStats,
    pub embeddings: EmbeddingStats,
}

#[derive(Debug, serde::Serialize)]
pub struct NodeStats {
    pub total: i64,
    pub sources: i64,
    pub articles: i64,
    pub sessions: i64,
    pub active: i64,
    pub archived: i64,
    pub pinned: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct EdgeStats {
    pub sql_count: i64,
    pub age_count: i64,
    pub in_sync: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct QueueStats {
    pub pending: i64,
    pub processing: i64,
    pub failed: i64,
    pub completed_24h: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct EmbeddingStats {
    pub total: i64,
    pub nodes_without: i64,
}

#[derive(Debug, serde::Deserialize)]
pub struct MaintenanceRequest {
    pub recompute_scores: Option<bool>,
    pub process_queue: Option<bool>,
    pub evict_if_over_capacity: Option<bool>,
    pub evict_count: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct MaintenanceResponse {
    pub actions_taken: Vec<String>,
}

pub struct AdminService {
    pool: PgPool,
    graph: AgeGraphRepository,
}

impl AdminService {
    pub fn new(pool: PgPool) -> Self {
        let graph = AgeGraphRepository::new(pool.clone(), "covalence");
        Self { pool, graph }
    }

    pub async fn stats(&self) -> AppResult<StatsResponse> {
        // Node counts
        let node_row = sqlx::query(
            "SELECT \
               COUNT(*) AS total, \
               COUNT(*) FILTER (WHERE node_type = 'source') AS sources, \
               COUNT(*) FILTER (WHERE node_type = 'article') AS articles, \
               COUNT(*) FILTER (WHERE node_type = 'session') AS sessions, \
               COUNT(*) FILTER (WHERE status = 'active') AS active, \
               COUNT(*) FILTER (WHERE status = 'archived') AS archived, \
               COUNT(*) FILTER (WHERE pinned = true) AS pinned \
             FROM covalence.nodes"
        ).fetch_one(&self.pool).await?;

        // Edge counts + sync check
        let sql_edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covalence.edges")
            .fetch_one(&self.pool).await?;

        let age_edges = self.graph.count_edges().await.unwrap_or(-1);

        // Queue stats
        let queue_row = sqlx::query(
            "SELECT \
               COUNT(*) FILTER (WHERE status = 'pending') AS pending, \
               COUNT(*) FILTER (WHERE status = 'processing') AS processing, \
               COUNT(*) FILTER (WHERE status = 'failed') AS failed, \
               COUNT(*) FILTER (WHERE status = 'completed' AND completed_at > now() - interval '24 hours') AS completed_24h \
             FROM covalence.slow_path_queue"
        ).fetch_one(&self.pool).await?;

        // Embedding coverage
        let embed_row = sqlx::query(
            "SELECT \
               (SELECT COUNT(*) FROM covalence.node_embeddings) AS total, \
               (SELECT COUNT(*) FROM covalence.nodes WHERE status = 'active' AND content IS NOT NULL) - \
               (SELECT COUNT(*) FROM covalence.node_embeddings ne JOIN covalence.nodes n ON ne.node_id = n.id WHERE n.status = 'active') AS nodes_without"
        ).fetch_one(&self.pool).await?;

        Ok(StatsResponse {
            nodes: NodeStats {
                total: node_row.get("total"),
                sources: node_row.get("sources"),
                articles: node_row.get("articles"),
                sessions: node_row.get("sessions"),
                active: node_row.get("active"),
                archived: node_row.get("archived"),
                pinned: node_row.get("pinned"),
            },
            edges: EdgeStats {
                sql_count: sql_edges,
                age_count: age_edges,
                in_sync: sql_edges == age_edges,
            },
            queue: QueueStats {
                pending: queue_row.get("pending"),
                processing: queue_row.get("processing"),
                failed: queue_row.get("failed"),
                completed_24h: queue_row.get("completed_24h"),
            },
            embeddings: EmbeddingStats {
                total: embed_row.get("total"),
                nodes_without: embed_row.get("nodes_without"),
            },
        })
    }

    pub async fn maintenance(&self, req: MaintenanceRequest) -> AppResult<MaintenanceResponse> {
        let mut actions = Vec::new();

        if req.recompute_scores.unwrap_or(false) {
            // Recompute usage scores from retrieval events
            sqlx::query(
                "UPDATE covalence.nodes n SET usage_score = COALESCE(
                    (SELECT COUNT(*)::float * EXP(-0.01 * EXTRACT(EPOCH FROM (now() - MAX(ue.accessed_at))) / 86400.0)
                     FROM covalence.usage_traces ue WHERE ue.node_id = n.id), 0.0
                ) WHERE n.status = 'active'"
            ).execute(&self.pool).await?;
            actions.push("recomputed usage scores".into());
        }

        if req.process_queue.unwrap_or(false) {
            // For now, just mark stale processing jobs as failed
            let result = sqlx::query(
                "UPDATE covalence.slow_path_queue \
                 SET status = 'failed', error = 'timed out' \
                 WHERE status = 'processing' AND started_at < now() - interval '10 minutes'"
            ).execute(&self.pool).await?;
            actions.push(format!("timed out {} stale queue jobs", result.rows_affected()));
        }

        if req.evict_if_over_capacity.unwrap_or(false) {
            let max_active = 1000i64; // configurable later
            let evict_count = req.evict_count.unwrap_or(10);

            let active_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM covalence.nodes WHERE node_type = 'article' AND status = 'active'"
            ).fetch_one(&self.pool).await?;

            if active_count > max_active {
                let result = sqlx::query(
                    "UPDATE covalence.nodes SET status = 'archived', archived_at = now() \
                     WHERE id IN ( \
                       SELECT id FROM covalence.nodes \
                       WHERE node_type = 'article' AND status = 'active' AND pinned = false \
                       ORDER BY usage_score ASC LIMIT $1 \
                     )"
                )
                    .bind(evict_count)
                    .execute(&self.pool).await?;
                actions.push(format!("evicted {} low-usage articles", result.rows_affected()));
            } else {
                actions.push(format!("no eviction needed ({active_count}/{max_active} active)"));
            }
        }

        if actions.is_empty() {
            actions.push("no operations requested".into());
        }

        Ok(MaintenanceResponse { actions_taken: actions })
    }
}
