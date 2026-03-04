//! Admin stats and maintenance operations (SPEC §5.4).

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::*;
use crate::graph::{GraphRepository, SharedGraph, SqlGraphRepository, algorithms::pagerank};

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
    /// When `true`, enqueue a `recompute_graph_embeddings` task.
    /// Pass `method` in the sub-field to select `"node2vec"`, `"spectral"`, or `"both"`.
    pub recompute_graph_embeddings: Option<bool>,
    /// Embedding method: `"node2vec"`, `"spectral"`, or `"both"` (default).
    pub graph_embeddings_method: Option<String>,
    /// When `true`, scan for articles whose `next_consolidation_at` has arrived
    /// and enqueue a `consolidate_article` task for each (covalence#67).
    /// This mirrors the worker heartbeat but can be triggered on-demand via
    /// the maintenance API.
    pub scan_due_consolidations: Option<bool>,
    /// When `true`, refresh the `covalence.contends_derived` materialized view
    /// (covalence#99).  Equivalent to calling `POST /admin/refresh-inference`.
    pub refresh_inference: Option<bool>,
    /// When `true`, aggregate gap_log (past 30 days) → gap_registry (covalence#100).
    /// Topics with query_count >= 3 are upserted; gap_score computed from
    /// avg_top_score and normalized demand.
    pub compute_gaps: Option<bool>,
    /// When `true`, recompute the `structural_importance` column for all active
    /// article nodes (covalence#101).  Uses in-degree (edges pointing at the node)
    /// plus contention accommodation count plus pinned flag.
    pub compute_structural_importance: Option<bool>,
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
    /// Shared in-memory graph used for PageRank-based usage_score blending.
    /// Set via [`AdminService::with_graph`].
    shared_graph: Option<SharedGraph>,
}

impl AdminService {
    pub fn new(pool: PgPool) -> Self {
        let graph = SqlGraphRepository::new(pool.clone());
        Self {
            pool,
            graph,
            shared_graph: None,
        }
    }

    /// Attach the shared in-memory graph (enables PageRank influence on
    /// `usage_score` recompute when `COVALENCE_TOPOLOGICAL_CONFIDENCE=true`).
    pub fn with_graph(mut self, graph: SharedGraph) -> Self {
        self.shared_graph = Some(graph);
        self
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

    #[allow(deprecated)]
    pub async fn maintenance(&self, req: MaintenanceRequest) -> AppResult<MaintenanceResponse> {
        let mut actions = Vec::new();

        if req.recompute_scores.unwrap_or(false) {
            // Pass 1: Recompute usage scores from retrieval events.
            sqlx::query(
                "UPDATE covalence.nodes n SET usage_score = COALESCE(
                    (SELECT COUNT(*)::float * EXP(-0.01 * EXTRACT(EPOCH FROM (now() - MAX(ue.accessed_at))) / 86400.0)
                     FROM covalence.usage_traces ue WHERE ue.node_id = n.id), 0.0
                ) WHERE n.status = 'active'"
            ).execute(&self.pool).await?;
            actions.push("recomputed usage scores".into());

            // Pass 2 (feature-flagged): blend PageRank into usage_score.
            // Enabled only when COVALENCE_TOPOLOGICAL_CONFIDENCE=true and a
            // shared graph is available.
            let topo_enabled = std::env::var("COVALENCE_TOPOLOGICAL_CONFIDENCE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);

            if topo_enabled {
                if let Some(shared) = &self.shared_graph {
                    let graph = shared.read().await;
                    let pr_scores = pagerank(&graph, 0.85, 20);
                    drop(graph);

                    let mut blended_count = 0usize;
                    for (node_id, pr_score) in &pr_scores {
                        let result = sqlx::query(
                            "UPDATE covalence.nodes \
                             SET usage_score = LEAST(1.0, usage_score * 0.8 + $1 * 0.2) \
                             WHERE id = $2 AND status = 'active'",
                        )
                        .bind(pr_score)
                        .bind(node_id)
                        .execute(&self.pool)
                        .await?;
                        blended_count += result.rows_affected() as usize;
                    }
                    actions.push(format!(
                        "blended pagerank into usage_score for {blended_count} nodes"
                    ));
                } else {
                    tracing::debug!(
                        "COVALENCE_TOPOLOGICAL_CONFIDENCE=true but no shared graph attached; \
                         skipping pagerank blend"
                    );
                }
            }
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
            // `COVALENCE_MAX_ARTICLES` overrides the default capacity ceiling.
            // Useful for testing eviction behaviour without creating 1 000 articles.
            let max_active: i64 = std::env::var("COVALENCE_MAX_ARTICLES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1000);
            let evict_count = req.evict_count.unwrap_or(10);

            let active_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM covalence.nodes WHERE node_type = 'article' AND status = 'active'"
            ).fetch_one(&self.pool).await?;

            if active_count > max_active {
                // ── Structural-importance eviction guard (covalence#101) ───────
                // Use the persisted structural_importance column to compute a
                // combined evict_score.  Articles with structural_importance >= 0.8
                // are fully protected.  The rest are ranked by:
                //
                //   evict_score = (1 - structural_importance)
                //               * (1 - usage_score_normalized)
                //
                // Lower evict_score → evicted first (least load-bearing + least used).
                let evicted_rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT
                            id,
                            structural_importance,
                            usage_score,
                            usage_score / NULLIF(MAX(usage_score) OVER (), 0) AS usage_score_normalized
                        FROM covalence.nodes
                        WHERE node_type = 'article'
                          AND status    = 'active'
                          AND pinned    = false
                          AND structural_importance < 0.8
                    ),
                    scored AS (
                        SELECT
                            id,
                            (1.0 - structural_importance)
                            * (1.0 - COALESCE(usage_score_normalized, 0.0)) AS evict_score
                        FROM candidates
                    )
                    UPDATE covalence.nodes
                       SET status      = 'archived',
                           archived_at = now()
                     WHERE id IN (
                         SELECT id FROM scored
                         ORDER BY evict_score ASC
                         LIMIT $1
                     )
                    RETURNING id
                    "#,
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

                actions.push(format!(
                    "evicted {evicted_count} articles by evict_score \
                     (structural_importance >= 0.8 protected)"
                ));
            } else {
                actions.push(format!(
                    "no eviction needed ({active_count}/{max_active} active)"
                ));
            }
        }

        if req.recompute_graph_embeddings.unwrap_or(false) {
            let method = req
                .graph_embeddings_method
                .as_deref()
                .unwrap_or("both")
                .to_string();
            sqlx::query(
                "INSERT INTO covalence.slow_path_queue \
                 (id, task_type, node_id, payload, status, priority) \
                 VALUES (gen_random_uuid(), 'recompute_graph_embeddings', NULL, $1, 'pending', 2)",
            )
            .bind(serde_json::json!({ "method": method }))
            .execute(&self.pool)
            .await?;
            actions.push(format!(
                "queued recompute_graph_embeddings(method={method})"
            ));
        }

        if req.scan_due_consolidations.unwrap_or(false) {
            // Query for active articles whose expanding-interval timer has fired.
            // Skips articles with no linked sources (orphan guard applied by the
            // handler itself, but we count them for the action log).
            let due_rows = sqlx::query(
                "SELECT n.id, COALESCE(n.consolidation_count, 0) AS consolidation_count
                 FROM   covalence.nodes n
                 WHERE  n.node_type             = 'article'
                   AND  n.status                = 'active'
                   AND  n.next_consolidation_at IS NOT NULL
                   AND  n.next_consolidation_at <= now()
                   AND  NOT EXISTS (
                       SELECT 1
                       FROM   covalence.slow_path_queue q
                       WHERE  q.task_type               = 'consolidate_article'
                         AND  q.payload->>'article_id'  = n.id::text
                         AND  q.status IN ('pending', 'processing')
                   )",
            )
            .fetch_all(&self.pool)
            .await?;

            let due_count = due_rows.len();
            let mut queued_count = 0usize;

            for row in &due_rows {
                use sqlx::Row as _;
                let article_id: Uuid = row.get("id");
                let count: i32 = row.get("consolidation_count");
                let next_pass = count + 1;

                let result = sqlx::query(
                    "INSERT INTO covalence.slow_path_queue \
                     (id, task_type, node_id, payload, status, priority) \
                     VALUES (gen_random_uuid(), 'consolidate_article', NULL, $1, 'pending', 3)",
                )
                .bind(serde_json::json!({
                    "article_id": article_id.to_string(),
                    "pass": next_pass,
                }))
                .execute(&self.pool)
                .await;

                match result {
                    Ok(_) => queued_count += 1,
                    Err(e) => tracing::warn!(
                        article_id = %article_id,
                        "scan_due_consolidations: failed to enqueue task: {e}"
                    ),
                }
            }

            actions.push(format!(
                "scan_due_consolidations: {due_count} due, {queued_count} newly queued"
            ));
        }

        if req.refresh_inference.unwrap_or(false) {
            self.refresh_inference_view().await?;
            actions.push("refreshed contends_derived materialized view".into());
        }

        if req.compute_gaps.unwrap_or(false) {
            let n = self.compute_gap_registry().await?;
            actions.push(format!("compute_gaps: upserted {n} gap topics"));
        }

        if req.compute_structural_importance.unwrap_or(false) {
            let n = self.compute_structural_importance().await?;
            actions.push(format!("compute_structural_importance: updated {n} nodes"));
        }

        if actions.is_empty() {
            actions.push("no operations requested".into());
        }

        Ok(MaintenanceResponse {
            actions_taken: actions,
        })
    }

    /// Refresh the `covalence.contends_derived` materialized view.
    ///
    /// Uses `CONCURRENTLY` so existing readers are not blocked during refresh.
    /// Callable via `POST /admin/refresh-inference` or through
    /// `admin_maintenance` with `refresh_inference: true`.
    pub async fn refresh_inference_view(&self) -> AppResult<()> {
        sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY covalence.contends_derived")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Synchronise AGE graph edges with the SQL `covalence.edges` mirror.
    ///
    /// Three passes:
    /// 1. **Orphan cleanup** — AGE edges whose `edge_id` property is absent from SQL are deleted.
    /// 2. **Missing creation** — SQL edges with `age_id IS NULL` are written into AGE (if both
    ///    endpoint vertices already exist in AGE).
    /// 3. Counts everything else as already-synced.
    #[allow(deprecated)]
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

// ── Intent-aware graph statistics (Phase 7, covalence#54) ──────────────────

/// Edge count breakdown by search-intent category.
///
/// Returned by `GET /admin/graph/intent-stats`.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct IntentStatsResponse {
    /// Edges matching the Factual intent: CONFIRMS + ORIGINATES + COMPILED_FROM.
    pub factual_edges: i64,
    /// Edges matching the Temporal intent: PRECEDES + FOLLOWS.
    pub temporal_edges: i64,
    /// Edges matching the Causal intent: CAUSES + MOTIVATED_BY + IMPLEMENTS.
    pub causal_edges: i64,
    /// Edges matching the Entity intent: INVOLVES.
    pub entity_edges: i64,
    /// Edges that don't fall into any of the four intent categories.
    pub other_edges: i64,
    /// Grand total of all edges.
    pub total_edges: i64,
}

impl AdminService {
    /// Return the edge-count breakdown by intent category.
    ///
    /// Queries the SQL `covalence.edges` table, which mirrors the AGE graph.
    /// COMPILED_FROM is included with factual edges as the legacy alias for ORIGINATES.
    pub async fn graph_intent_stats(&self) -> AppResult<IntentStatsResponse> {
        let row = sqlx::query(
            "SELECT
               COUNT(*) FILTER (WHERE edge_type IN ('CONFIRMS', 'ORIGINATES', 'COMPILED_FROM'))
                   AS factual,
               COUNT(*) FILTER (WHERE edge_type IN ('PRECEDES', 'FOLLOWS'))
                   AS temporal,
               COUNT(*) FILTER (WHERE edge_type IN ('CAUSES', 'MOTIVATED_BY', 'IMPLEMENTS'))
                   AS causal,
               COUNT(*) FILTER (WHERE edge_type = 'INVOLVES')
                   AS entity,
               COUNT(*) AS total
             FROM covalence.edges",
        )
        .fetch_one(&self.pool)
        .await?;

        use sqlx::Row;
        let factual: i64 = row.get("factual");
        let temporal: i64 = row.get("temporal");
        let causal: i64 = row.get("causal");
        let entity: i64 = row.get("entity");
        let total: i64 = row.get("total");
        let other = total - factual - temporal - causal - entity;

        Ok(IntentStatsResponse {
            factual_edges: factual,
            temporal_edges: temporal,
            causal_edges: causal,
            entity_edges: entity,
            other_edges: other,
            total_edges: total,
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

// ── Epistemic SLIs (covalence#88) ──────────────────────────────────────────

/// A single Service-Level Indicator reading.
///
/// Contains the measured `value`, the `target` threshold, and a derived
/// `healthy` flag indicating whether the indicator is within acceptable bounds.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct SliReading {
    /// The measured ratio (0.0 – 1.0 for coverage/rate metrics).
    pub value: f32,
    /// The target threshold for "healthy" classification.
    pub target: f32,
    /// `true` when the value meets the target (direction depends on SLI type).
    pub healthy: bool,
}

/// Full response for `GET /admin/epistemic`.
///
/// Reports six knowledge-health Service-Level Indicators that together form
/// a measurable baseline for epistemic system quality.
///
/// # Phase 1 SLIs implemented here
/// | SLI | Target | Direction |
/// |-----|--------|-----------|
/// | `embedding_coverage`  | ≥ 0.98 | higher is better |
/// | `knowledge_freshness` | ≥ 0.70 | higher is better |
/// | `graph_connectivity`  | ≥ 0.95 | higher is better |
/// | `confidence_health`   | ≥ 0.85 | higher is better |
/// | `contention_rate`     | < 0.05 | lower is better  |
/// | `queue_health`        | < 0.05 | lower is better  |
///
/// # Phase 2 (not yet implemented)
/// `retrieval_quality` (precision/recall from usage traces) is intentionally
/// omitted here — it requires an offline eval harness with ground-truth
/// relevance labels and cannot be computed with a single SQL query.
/// Tracking issue: covalence#88.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct EpistemicSliResponse {
    /// Fraction of active articles that have a content embedding stored in
    /// `node_embeddings`. Embedding-less articles are invisible to vector
    /// search; target ≥ 0.98.
    pub embedding_coverage: SliReading,

    /// Fraction of active articles whose `modified_at` falls within the last
    /// 30 days. A low score indicates the corpus is stale; target ≥ 0.70.
    pub knowledge_freshness: SliReading,

    /// Fraction of active nodes that have at least one edge (inbound or
    /// outbound) in the `covalence.edges` table. Isolated nodes cannot be
    /// surfaced by graph-traversal strategies; target ≥ 0.95.
    pub graph_connectivity: SliReading,

    /// Fraction of active articles whose `confidence` column exceeds 0.5.
    /// Low confidence signals uncertain or under-sourced knowledge; target ≥
    /// 0.85.
    pub confidence_health: SliReading,

    /// Ratio of `detected` contentions to total active articles. Elevated
    /// contention indicates unresolved contradictions in the corpus; target
    /// < 0.05.
    pub contention_rate: SliReading,

    /// Ratio of `failed` queue items to all non-completed queue items
    /// (pending + processing + failed + 1 sentinel to prevent /0). Elevated
    /// failure rate signals a broken worker or bad payloads; target < 0.05.
    pub queue_health: SliReading,
}

impl AdminService {
    /// Compute all six Phase-1 epistemic SLIs and return them as a structured
    /// response suitable for `GET /admin/epistemic`.
    ///
    /// All six metrics are computed in three parallel SQL queries to keep
    /// latency low.  Each query uses conditional aggregation so no temporary
    /// tables or subquery explosions are needed.
    pub async fn epistemic_slis(&self) -> AppResult<EpistemicSliResponse> {
        // ── Query 1: article-level metrics ───────────────────────────────────
        // Computes embedding coverage, knowledge freshness, and confidence
        // health in a single pass over covalence.nodes.
        let article_row = sqlx::query(
            "SELECT
               COUNT(*) FILTER (WHERE node_type = 'article' AND status = 'active')
                   AS total_articles,
               COUNT(ne.node_id)
                   FILTER (WHERE n.node_type = 'article' AND n.status = 'active')
                   AS articles_with_embeddings,
               COUNT(*) FILTER (
                   WHERE node_type = 'article'
                     AND status    = 'active'
                     AND modified_at >= now() - INTERVAL '30 days'
               ) AS fresh_articles,
               COUNT(*) FILTER (
                   WHERE node_type = 'article'
                     AND status    = 'active'
                     AND confidence > 0.5
               ) AS articles_high_confidence
             FROM covalence.nodes n
             LEFT JOIN covalence.node_embeddings ne ON ne.node_id = n.id
               AND n.node_type = 'article'
               AND n.status    = 'active'",
        )
        .fetch_one(&self.pool)
        .await?;

        // ── Query 2: graph connectivity ───────────────────────────────────────
        // Counts all active nodes and those with at least one edge.
        let connectivity_row = sqlx::query(
            "SELECT
               COUNT(*) AS total_nodes,
               COUNT(*) FILTER (WHERE id IN (
                   SELECT DISTINCT source_node_id FROM covalence.edges
                   UNION
                   SELECT DISTINCT target_node_id FROM covalence.edges
               )) AS connected_nodes
             FROM covalence.nodes
             WHERE status = 'active'",
        )
        .fetch_one(&self.pool)
        .await?;

        // ── Query 3: contentions & queue health ───────────────────────────────
        let contention_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM covalence.contentions WHERE status = 'detected'",
        )
        .fetch_one(&self.pool)
        .await?;

        let queue_row = sqlx::query(
            "SELECT
               COUNT(*) FILTER (WHERE status = 'failed')     AS failed,
               COUNT(*) FILTER (WHERE status = 'pending')    AS pending,
               COUNT(*) FILTER (WHERE status = 'processing') AS processing
             FROM covalence.slow_path_queue",
        )
        .fetch_one(&self.pool)
        .await?;

        // ── Assemble readings ─────────────────────────────────────────────────
        let total_articles: i64 = article_row.try_get("total_articles").unwrap_or(0);
        let articles_with_embeddings: i64 =
            article_row.try_get("articles_with_embeddings").unwrap_or(0);
        let fresh_articles: i64 = article_row.try_get("fresh_articles").unwrap_or(0);
        let articles_high_confidence: i64 =
            article_row.try_get("articles_high_confidence").unwrap_or(0);

        let total_nodes: i64 = connectivity_row.try_get("total_nodes").unwrap_or(0);
        let connected_nodes: i64 = connectivity_row.try_get("connected_nodes").unwrap_or(0);

        let failed: i64 = queue_row.try_get("failed").unwrap_or(0);
        let pending: i64 = queue_row.try_get("pending").unwrap_or(0);
        let processing: i64 = queue_row.try_get("processing").unwrap_or(0);

        // Avoid division-by-zero: empty corpus → trivially healthy for coverage
        // metrics, and 0.0 for rate metrics.
        let embedding_coverage_val = if total_articles == 0 {
            1.0_f32
        } else {
            articles_with_embeddings as f32 / total_articles as f32
        };

        let freshness_val = if total_articles == 0 {
            1.0_f32
        } else {
            fresh_articles as f32 / total_articles as f32
        };

        let connectivity_val = if total_nodes == 0 {
            1.0_f32
        } else {
            connected_nodes as f32 / total_nodes as f32
        };

        let confidence_val = if total_articles == 0 {
            1.0_f32
        } else {
            articles_high_confidence as f32 / total_articles as f32
        };

        let contention_val = if total_articles == 0 {
            0.0_f32
        } else {
            contention_count as f32 / total_articles as f32
        };

        // Sentinel +1 prevents division-by-zero even if queue is empty.
        let queue_denominator = pending + processing + failed + 1;
        let queue_val = failed as f32 / queue_denominator as f32;

        Ok(EpistemicSliResponse {
            embedding_coverage: SliReading {
                value: embedding_coverage_val,
                target: 0.98,
                healthy: embedding_coverage_val >= 0.98,
            },
            knowledge_freshness: SliReading {
                value: freshness_val,
                target: 0.70,
                healthy: freshness_val >= 0.70,
            },
            graph_connectivity: SliReading {
                value: connectivity_val,
                target: 0.95,
                healthy: connectivity_val >= 0.95,
            },
            confidence_health: SliReading {
                value: confidence_val,
                target: 0.85,
                healthy: confidence_val >= 0.85,
            },
            contention_rate: SliReading {
                value: contention_val,
                target: 0.05,
                healthy: contention_val < 0.05,
            },
            queue_health: SliReading {
                value: queue_val,
                target: 0.05,
                healthy: queue_val < 0.05,
            },
        })
    }
}

// ── Structural Importance (covalence#101) ───────────────────────────────────

impl AdminService {
    /// Batch-recompute the `structural_importance` column for all active article
    /// nodes using a formula based on:
    ///
    /// - **in_degree**: number of active edges pointing at the node.
    /// - **accommodation_count**: contentions resolved with `resolution = 'supersede_b'`
    ///   (i.e. the source won, meaning the article was important enough to be
    ///   updated / the system accommodated it).
    /// - **pinned**: adds a flat 0.5 bonus.
    ///
    /// Formula (all terms clamped to [0, 1] via LEAST):
    /// ```text
    /// structural_importance = LEAST(1.0,
    ///     log(1 + in_degree) / NULLIF(log(1 + max_in_degree), 0)
    ///   + CASE WHEN pinned THEN 0.5 ELSE 0.0 END
    ///   + 0.3 * accommodation_count / NULLIF(max_accommodation_count, 0)
    /// )
    /// ```
    ///
    /// When there are no edges at all (`max_in_degree = 0`) every node is set to
    /// 0.0 (with pinned still getting +0.5, clamped to its raw value).
    ///
    /// Returns the number of rows updated.
    pub async fn compute_structural_importance(&self) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            WITH raw AS (
                SELECT
                    n.id,
                    n.pinned,
                    (SELECT COUNT(*)
                     FROM covalence.edges e
                     WHERE e.target_node_id = n.id
                       AND e.valid_to IS NULL) AS in_degree,
                    (SELECT COUNT(*)
                     FROM covalence.contentions c
                     WHERE c.node_id = n.id
                       AND c.resolution = 'supersede_b') AS accommodation_count
                FROM covalence.nodes n
                WHERE n.node_type = 'article'
                  AND n.status    = 'active'
            ),
            maxima AS (
                SELECT
                    MAX(in_degree)::float           AS max_in_degree,
                    MAX(accommodation_count)::float AS max_accommodation_count
                FROM raw
            )
            UPDATE covalence.nodes dst
               SET structural_importance = LEAST(1.0,
                       COALESCE(
                           ln(1.0 + raw.in_degree::float)
                           / NULLIF(ln(1.0 + maxima.max_in_degree), 0),
                           0.0
                       )
                   + CASE WHEN raw.pinned THEN 0.5 ELSE 0.0 END
                   + COALESCE(
                           0.3 * raw.accommodation_count::float
                           / NULLIF(maxima.max_accommodation_count, 0),
                           0.0
                     )
               )
              FROM raw
              CROSS JOIN maxima
             WHERE dst.id = raw.id
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

// ── Gap Registry (covalence#100) ────────────────────────────────────────────

/// A single entry from the gap registry, returned by `GET /admin/gaps`.
#[derive(Debug, serde::Serialize)]
pub struct GapEntry {
    pub id: Uuid,
    pub topic: String,
    pub namespace: String,
    pub query_count: i32,
    pub avg_top_score: Option<f64>,
    pub last_queried_at: Option<DateTime<Utc>>,
    pub gap_score: Option<f64>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AdminService {
    /// Aggregate `gap_log` (past 30 days) into `gap_registry`.
    ///
    /// Algorithm:
    /// 1. Group gap_log by `lower(trim(query))` + `namespace`.
    /// 2. Keep only groups with `query_count >= 3`.
    /// 3. Compute `gap_score = 0.70*(1-avg_top_score) + 0.30*(normalized demand)`.
    ///    - `avg_top_score` defaults to 0.0 when all results were empty (no score).
    ///    - `normalized demand` = `query_count / max(query_count over all groups)`.
    /// 4. UPSERT into `gap_registry` (keyed on `(topic, namespace)`).
    ///
    /// Returns the number of topics upserted.
    pub async fn compute_gap_registry(&self) -> AppResult<i64> {
        // Aggregate gap_log → gap_registry for topics with >= 3 queries.
        // gap_score = 0.70*(1-avg_top_score) + 0.30*(normalized demand)
        sqlx::query(
            r#"
            WITH agg AS (
                SELECT
                    lower(trim(query))          AS topic,
                    namespace,
                    COUNT(*)::int               AS query_count,
                    AVG(COALESCE(top_score, 0)) AS avg_top_score,
                    MAX(created_at)             AS last_queried_at
                FROM covalence.gap_log
                WHERE created_at >= now() - interval '30 days'
                GROUP BY lower(trim(query)), namespace
                HAVING COUNT(*) >= 3
            ),
            max_demand AS (
                SELECT GREATEST(MAX(query_count), 1)::float AS max_qc FROM agg
            ),
            scored AS (
                SELECT
                    agg.topic,
                    agg.namespace,
                    agg.query_count,
                    agg.avg_top_score,
                    agg.last_queried_at,
                    0.70 * (1.0 - agg.avg_top_score)
                    + 0.30 * (agg.query_count::float / max_demand.max_qc)
                        AS gap_score
                FROM agg
                CROSS JOIN max_demand
            )
            INSERT INTO covalence.gap_registry
                (id, topic, namespace, query_count, avg_top_score,
                 last_queried_at, gap_score, status, created_at, updated_at)
            SELECT
                gen_random_uuid(),
                topic,
                namespace,
                query_count,
                avg_top_score,
                last_queried_at,
                gap_score,
                'open',
                now(),
                now()
            FROM scored
            ON CONFLICT (topic, namespace) DO UPDATE
                SET query_count     = EXCLUDED.query_count,
                    avg_top_score   = EXCLUDED.avg_top_score,
                    last_queried_at = EXCLUDED.last_queried_at,
                    gap_score       = EXCLUDED.gap_score,
                    updated_at      = now()
            "#,
        )
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() as i64)
        .map_err(AppError::Database)
    }

    /// Return the top-10 open gap topics sorted by `gap_score DESC`.
    pub async fn list_gaps(&self, namespace: Option<&str>) -> AppResult<Vec<GapEntry>> {
        let rows = sqlx::query(
            "SELECT id, topic, namespace, query_count, avg_top_score,
                    last_queried_at, gap_score, status, created_at, updated_at
             FROM covalence.gap_registry
             WHERE status = 'open'
               AND ($1::text IS NULL OR namespace = $1)
             ORDER BY gap_score DESC NULLS LAST
             LIMIT 10",
        )
        .bind(namespace)
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|r| {
                use sqlx::Row;
                Ok(GapEntry {
                    id: r.try_get("id")?,
                    topic: r.try_get("topic")?,
                    namespace: r.try_get("namespace")?,
                    query_count: r.try_get("query_count")?,
                    avg_top_score: r.try_get("avg_top_score")?,
                    last_queried_at: r.try_get("last_queried_at")?,
                    gap_score: r.try_get("gap_score")?,
                    status: r.try_get("status")?,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(AppError::Database)
    }
}
