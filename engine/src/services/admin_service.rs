//! Admin stats and maintenance operations (SPEC §5.4).

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::*;
use crate::graph::{GraphRepository, SqlGraphRepository};

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct StatsResponse {
    pub nodes: NodeStats,
    pub edges: EdgeStats,
    pub queue: QueueStats,
    pub embeddings: EmbeddingStats,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct NodeStats {
    pub total: i64,
    pub sources: i64,
    pub articles: i64,
    pub sessions: i64,
    pub active: i64,
    pub archived: i64,
    pub pinned: i64,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EdgeStats {
    pub sql_count: i64,
    pub age_count: i64,
    pub in_sync: bool,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct QueueStats {
    pub pending: i64,
    pub processing: i64,
    pub failed: i64,
    pub completed_24h: i64,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EmbeddingStats {
    pub total: i64,
    pub nodes_without: i64,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct MaintenanceRequest {
    pub recompute_scores: Option<bool>,
    pub process_queue: Option<bool>,
    pub evict_if_over_capacity: Option<bool>,
    pub evict_count: Option<i64>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct MaintenanceResponse {
    pub actions_taken: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct SyncEdgesResponse {
    /// AGE edges with no SQL counterpart that were deleted.
    pub orphaned_deleted: i64,
    /// SQL edges missing from AGE that were created.
    pub missing_created: i64,
    /// SQL edges that already had an AGE counterpart (no action needed).
    pub already_synced: i64,
}

/// Result returned by [`AdminService::staleness_scan`].
#[derive(Debug, serde::Serialize)]
pub struct StalenessResult {
    /// Number of active articles deemed stale.
    pub stale_count: usize,
    /// Number of articles newly queued for recompilation (excludes already-queued).
    pub queued_count: usize,
}

pub struct AdminService {
    pool: PgPool,
    graph: SqlGraphRepository,
}

impl AdminService {
    pub fn new(pool: PgPool) -> Self {
        let graph = SqlGraphRepository::new(pool.clone());
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
             FROM covalence.nodes",
        )
        .fetch_one(&self.pool)
        .await?;

        // Edge counts + sync check
        let sql_edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covalence.edges")
            .fetch_one(&self.pool)
            .await?;

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
                r#"UPDATE covalence.slow_path_queue
                   SET status = 'failed', result = '{"error":"timed_out"}'::jsonb
                   WHERE status = 'processing' AND started_at < now() - interval '10 minutes'"#,
            )
            .execute(&self.pool)
            .await?;
            actions.push(format!(
                "timed out {} stale queue jobs",
                result.rows_affected()
            ));
        }

        if req.evict_if_over_capacity.unwrap_or(false) {
            let max_active = 1000i64; // configurable later
            let evict_count = req.evict_count.unwrap_or(10);

            let active_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM covalence.nodes WHERE node_type = 'article' AND status = 'active'"
            ).fetch_one(&self.pool).await?;

            if active_count > max_active {
                // RETURNING id lets us remove each evicted node from the live
                // AGE graph while preserving the SQL edges for history.
                let evicted_rows = sqlx::query(
                    "UPDATE covalence.nodes SET status = 'archived', archived_at = now() \
                     WHERE id IN ( \
                       SELECT id FROM covalence.nodes \
                       WHERE node_type = 'article' AND status = 'active' AND pinned = false \
                       ORDER BY usage_score ASC LIMIT $1 \
                     ) \
                     RETURNING id",
                )
                .bind(evict_count)
                .fetch_all(&self.pool)
                .await?;

                let evicted_count = evicted_rows.len();
                for row in &evicted_rows {
                    let node_id: Uuid = row.get("id");
                    if let Err(e) = self.graph.archive_vertex(node_id).await {
                        tracing::warn!(
                            node_id = %node_id,
                            "eviction: archive_vertex failed (non-fatal): {e}"
                        );
                    }
                }

                actions.push(format!("evicted {evicted_count} low-usage articles"));
            } else {
                actions.push(format!(
                    "no eviction needed ({active_count}/{max_active} active)"
                ));
            }
        }

        if actions.is_empty() {
            actions.push("no operations requested".into());
        }

        Ok(MaintenanceResponse {
            actions_taken: actions,
        })
    }

    /// Synchronise AGE graph edges with the SQL `covalence.edges` mirror.
    ///
    /// Three passes:
    /// 1. **Orphan cleanup** — AGE edges whose `edge_id` property is absent from SQL are deleted.
    /// 2. **Missing creation** — SQL edges with `age_id IS NULL` are written into AGE (if both
    ///    endpoint vertices already exist in AGE).
    /// 3. Counts everything else as already-synced.
    pub async fn sync_edges(&self) -> AppResult<SyncEdgesResponse> {
        use crate::graph::GraphRepository as _;

        let mut orphaned_deleted: i64 = 0;
        let mut missing_created: i64 = 0;

        // ── Pass 1: Orphan cleanup ─────────────────────────────────────────────
        // Get every (age_internal_id, sql_edge_uuid) pair from the AGE graph.
        let age_refs = self.graph.list_age_edge_refs().await.map_err(|e| {
            crate::errors::AppError::Internal(anyhow::anyhow!("sync_edges AGE query: {e}"))
        })?;

        // Build a set of all SQL edge UUIDs for O(1) lookup.
        let sql_edge_ids: std::collections::HashSet<Uuid> =
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM covalence.edges")
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();

        for (age_internal_id, maybe_sql_uuid) in &age_refs {
            let is_orphan = match maybe_sql_uuid {
                None => true, // no edge_id property — legacy edge with no SQL counterpart
                Some(uuid) => !sql_edge_ids.contains(uuid),
            };
            if is_orphan {
                if let Err(e) = self
                    .graph
                    .delete_age_edge_by_internal_id(*age_internal_id)
                    .await
                {
                    tracing::warn!(
                        age_id = age_internal_id,
                        "sync_edges: failed to delete orphaned AGE edge: {e}"
                    );
                } else {
                    orphaned_deleted += 1;
                }
            }
        }

        // ── Pass 2: Missing creation ───────────────────────────────────────────
        // SQL edges that were never written to AGE (age_id IS NULL).
        let missing_rows = sqlx::query(
            "SELECT e.id, e.source_node_id, e.target_node_id, e.edge_type, \
                    COALESCE(e.confidence, 1.0) AS confidence \
             FROM   covalence.edges e \
             WHERE  e.age_id IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        for row in &missing_rows {
            use sqlx::Row as _;
            let edge_id: Uuid = row.get("id");
            let from_id: Uuid = row.get("source_node_id");
            let to_id: Uuid = row.get("target_node_id");
            let edge_type_str: String = row.get("edge_type");
            let confidence: f64 = row.try_get("confidence").unwrap_or(1.0);

            let edge_type: crate::models::EdgeType = match edge_type_str.parse() {
                Ok(et) => et,
                Err(_) => {
                    tracing::warn!(
                        edge_id    = %edge_id,
                        edge_type  = %edge_type_str,
                        "sync_edges: unknown edge_type, skipping"
                    );
                    continue;
                }
            };

            match self
                .graph
                .create_age_edge_for_sql(edge_id, from_id, to_id, edge_type, confidence as f32)
                .await
            {
                Ok(Some(_)) => missing_created += 1,
                Ok(None) => {
                    // One or both vertices missing in AGE — not an error, but we
                    // can't create the edge yet.
                    tracing::debug!(
                        edge_id = %edge_id,
                        "sync_edges: vertices not yet in AGE, skipping edge"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        edge_id = %edge_id,
                        "sync_edges: failed to create AGE edge: {e}"
                    );
                }
            }
        }

        // ── Pass 3: Already-synced count ──────────────────────────────────────
        let total_sql: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM covalence.edges WHERE age_id IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        // Subtract newly created ones (they now have age_id set after Pass 2).
        let already_synced = total_sql.saturating_sub(missing_created);

        Ok(SyncEdgesResponse {
            orphaned_deleted,
            missing_created,
            already_synced,
        })
    }
}

// ── Queue listing ───────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct QueueEntry {
    pub id: Uuid,
    pub task_type: String,
    pub node_id: Option<Uuid>,
    pub status: String,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl AdminService {
    pub async fn list_queue(
        &self,
        status_filter: Option<&str>,
        limit: i64,
    ) -> AppResult<Vec<QueueEntry>> {
        let rows = sqlx::query(
            "SELECT id, task_type, node_id, status, priority, created_at, started_at, completed_at
             FROM covalence.slow_path_queue
             WHERE ($1::text IS NULL OR status = $1)
             ORDER BY priority DESC, created_at ASC
             LIMIT $2",
        )
        .bind(status_filter)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|r| {
                use sqlx::Row;
                Ok(QueueEntry {
                    id: r.try_get("id")?,
                    task_type: r.try_get("task_type")?,
                    node_id: r.try_get("node_id")?,
                    status: r.try_get("status")?,
                    priority: r.try_get("priority")?,
                    created_at: r.try_get("created_at")?,
                    started_at: r.try_get("started_at")?,
                    completed_at: r.try_get("completed_at")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(AppError::Database)
    }

    pub async fn get_queue_entry(&self, id: Uuid) -> AppResult<Option<QueueEntry>> {
        let row = sqlx::query(
            "SELECT id, task_type, node_id, status, priority, created_at, started_at, completed_at
             FROM covalence.slow_path_queue WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => {
                use sqlx::Row;
                Ok(Some(QueueEntry {
                    id: r.try_get("id")?,
                    task_type: r.try_get("task_type")?,
                    node_id: r.try_get("node_id")?,
                    status: r.try_get("status")?,
                    priority: r.try_get("priority")?,
                    created_at: r.try_get("created_at")?,
                    started_at: r.try_get("started_at")?,
                    completed_at: r.try_get("completed_at")?,
                }))
            }
            None => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub async fn record_usage(
        &self,
        node_id: Uuid,
        session_id: Option<&str>,
        query_text: &str,
        rank: i32,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO covalence.usage_traces (id, node_id, session_id, query_text, retrieval_rank)
             VALUES ($1, $2, $3, $4, $5)"
        )
        .bind(Uuid::new_v4())
        .bind(node_id)
        .bind(session_id)
        .bind(query_text)
        .bind(rank)
        .execute(&self.pool)
        .await?;

        sqlx::query("UPDATE covalence.nodes SET accessed_at = now() WHERE id = $1")
            .bind(node_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Queue embed tasks for all active nodes that lack embeddings.
    pub async fn queue_embed_all(&self) -> AppResult<i64> {
        let result = sqlx::query(
            r#"INSERT INTO covalence.slow_path_queue (id, task_type, node_id, payload, status, priority)
             SELECT gen_random_uuid(), 'embed', n.id, '{}'::jsonb, 'pending', 3
             FROM covalence.nodes n
             WHERE n.status = 'active'
               AND NOT EXISTS (SELECT 1 FROM covalence.node_embeddings ne WHERE ne.node_id = n.id)
               AND NOT EXISTS (
                   SELECT 1 FROM covalence.slow_path_queue q
                   WHERE q.node_id = n.id AND q.task_type = 'embed' AND q.status IN ('pending', 'processing')
               )"#
        ).execute(&self.pool).await?;
        Ok(result.rows_affected() as i64)
    }

    pub async fn retry_queue_entry(&self, id: Uuid) -> AppResult<QueueEntry> {
        // Verify entry exists and is failed
        let entry = self.get_queue_entry(id).await?;
        match entry {
            None => return Err(AppError::NotFound("queue entry not found".into())),
            Some(ref e) if e.status != "failed" => {
                return Err(AppError::BadRequest(format!(
                    "queue entry has status '{}'; only failed entries can be retried",
                    e.status
                )));
            }
            _ => {}
        }

        sqlx::query(
            "UPDATE covalence.slow_path_queue
             SET status = 'pending', started_at = NULL, completed_at = NULL, result = NULL
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        self.get_queue_entry(id)
            .await?
            .ok_or_else(|| AppError::NotFound("queue entry not found after retry reset".into()))
    }

    pub async fn delete_queue_entry(&self, id: Uuid) -> AppResult<bool> {
        let result = sqlx::query("DELETE FROM covalence.slow_path_queue WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Scan for stale articles and enqueue them for recompilation.
    ///
    /// An article is considered **stale** when there exist active source nodes
    /// that:
    /// * were created *after* the article's `modified_at` timestamp, AND
    /// * share at least one `domain_path` element with the article (or are
    ///   linked via `ORIGINATES` edges to another article in the same domain),
    ///   AND
    /// * are **not** already linked to the article via an `ORIGINATES` edge.
    ///
    /// Each stale article is queued with `task_type = 'recompile'` if it does
    /// not already have a pending/processing recompile entry.
    pub async fn staleness_scan(&self) -> AppResult<StalenessResult> {
        // ── Step 1: find stale articles ───────────────────────────────────────
        let stale_ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT DISTINCT a.id
            FROM covalence.nodes a
            WHERE a.node_type = 'article'
              AND a.status    = 'active'
              AND EXISTS (
                SELECT 1
                FROM   covalence.nodes s
                WHERE  s.node_type = 'source'
                  AND  s.status    = 'active'
                  AND  s.created_at > a.modified_at
                  AND (
                    -- direct domain overlap
                    (    a.domain_path IS NOT NULL
                     AND s.domain_path IS NOT NULL
                     AND a.domain_path && s.domain_path)
                    OR
                    -- source is already linked to another article in the same domain
                    EXISTS (
                      SELECT 1
                      FROM   covalence.edges     ars
                      JOIN   covalence.nodes      a2  ON ars.target_node_id = a2.id
                      WHERE  ars.source_node_id = s.id
                        AND  ars.edge_type      = 'ORIGINATES'
                        AND  a2.domain_path IS NOT NULL
                        AND  a.domain_path  IS NOT NULL
                        AND  a2.domain_path && a.domain_path
                    )
                  )
                  -- not already linked to this article
                  AND NOT EXISTS (
                    SELECT 1
                    FROM   covalence.edges ars2
                    WHERE  ars2.source_node_id = s.id
                      AND  ars2.target_node_id = a.id
                      AND  ars2.edge_type      = 'ORIGINATES'
                  )
              )
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let stale_count = stale_ids.len();
        let mut queued_count = 0usize;

        // ── Step 2: queue each stale article (skip if already queued) ─────────
        for article_id in &stale_ids {
            let result = sqlx::query(
                r#"
                INSERT INTO covalence.slow_path_queue
                    (id, task_type, node_id, payload, status)
                SELECT gen_random_uuid(), 'recompile', $1, '{}'::jsonb, 'pending'
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM   covalence.slow_path_queue
                    WHERE  task_type = 'recompile'
                      AND  node_id   = $1
                      AND  status    IN ('pending', 'processing')
                )
                "#,
            )
            .bind(article_id)
            .execute(&self.pool)
            .await?;

            if result.rows_affected() > 0 {
                queued_count += 1;
            }
        }

        Ok(StalenessResult {
            stale_count,
            queued_count,
        })
    }
}
