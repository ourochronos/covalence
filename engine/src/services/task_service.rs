//! Task state-machine service — worker lifecycle in the KB (covalence#114).
//!
//! Exposes CRUD operations for `covalence.tasks` and a stats endpoint that
//! returns per-status counts plus avg/p95 lead time for the past 7 days.
//! Auto-timeout maintenance lives in [`AdminService::maintenance`] but
//! delegates the actual SQL here via [`TaskService::timeout_stale`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::{AppError, AppResult};

// ─── Domain types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: Uuid,
    pub label: String,
    pub issue_ref: Option<String>,
    pub status: String,
    pub assigned_session_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub timeout_at: Option<DateTime<Utc>>,
    pub failure_class: Option<String>,
    pub result_summary: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub label: String,
    pub issue_ref: Option<String>,
    pub assigned_session_id: Option<String>,
    pub timeout_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTaskRequest {
    pub status: Option<String>,
    pub assigned_session_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub timeout_at: Option<DateTime<Utc>>,
    pub failure_class: Option<String>,
    pub result_summary: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ListTasksParams {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct TaskStats {
    /// Per-status task counts (all time).
    pub counts: StatusCounts,
    /// Average lead time in seconds for tasks completed in the last 7 days.
    /// `None` if no completed tasks in the window.
    pub avg_lead_time_secs: Option<f64>,
    /// 95th-percentile lead time in seconds for tasks completed in the last 7 days.
    /// `None` if no completed tasks in the window.
    pub p95_lead_time_secs: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct StatusCounts {
    pub pending: i64,
    pub assigned: i64,
    pub running: i64,
    pub done: i64,
    pub failed: i64,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct TaskService {
    pool: PgPool,
}

impl TaskService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ── Create ────────────────────────────────────────────────────────────────

    pub async fn create(&self, req: CreateTaskRequest) -> AppResult<Task> {
        let meta = if req.metadata.is_null() {
            serde_json::Value::Object(Default::default())
        } else {
            req.metadata
        };

        let row = sqlx::query(
            "INSERT INTO covalence.tasks
               (label, issue_ref, assigned_session_id, timeout_at, metadata)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, label, issue_ref, status, assigned_session_id,
                       started_at, completed_at, timeout_at, failure_class,
                       result_summary, metadata, created_at, updated_at",
        )
        .bind(&req.label)
        .bind(&req.issue_ref)
        .bind(&req.assigned_session_id)
        .bind(req.timeout_at)
        .bind(&meta)
        .fetch_one(&self.pool)
        .await?;

        task_from_row(&row)
    }

    // ── Get ───────────────────────────────────────────────────────────────────

    pub async fn get(&self, id: Uuid) -> AppResult<Option<Task>> {
        let row = sqlx::query(
            "SELECT id, label, issue_ref, status, assigned_session_id,
                    started_at, completed_at, timeout_at, failure_class,
                    result_summary, metadata, created_at, updated_at
             FROM covalence.tasks WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(task_from_row(&r)?)),
            None => Ok(None),
        }
    }

    // ── List ──────────────────────────────────────────────────────────────────

    pub async fn list(&self, params: ListTasksParams) -> AppResult<Vec<Task>> {
        let limit = params.limit.unwrap_or(20).min(200);
        let rows = sqlx::query(
            "SELECT id, label, issue_ref, status, assigned_session_id,
                    started_at, completed_at, timeout_at, failure_class,
                    result_summary, metadata, created_at, updated_at
             FROM covalence.tasks
             WHERE ($1::text IS NULL OR status = $1)
             ORDER BY created_at DESC
             LIMIT $2",
        )
        .bind(&params.status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(task_from_row).collect()
    }

    // ── Update ────────────────────────────────────────────────────────────────

    pub async fn update(&self, id: Uuid, req: UpdateTaskRequest) -> AppResult<Task> {
        // Validate status transition if a new status is provided.
        if let Some(ref s) = req.status {
            let valid = ["pending", "assigned", "running", "done", "failed"];
            if !valid.contains(&s.as_str()) {
                return Err(AppError::BadRequest(format!(
                    "invalid status '{s}'; must be one of: pending, assigned, running, done, failed"
                )));
            }
        }

        // Auto-set timestamps based on status transitions.
        let now = Utc::now();
        let started_at = req.started_at.or_else(|| {
            if req.status.as_deref() == Some("running") {
                Some(now)
            } else {
                None
            }
        });
        let completed_at = req.completed_at.or_else(|| {
            if matches!(req.status.as_deref(), Some("done") | Some("failed")) {
                Some(now)
            } else {
                None
            }
        });

        let row = sqlx::query(
            "UPDATE covalence.tasks SET
               status              = COALESCE($2, status),
               assigned_session_id = COALESCE($3, assigned_session_id),
               started_at          = COALESCE($4, started_at),
               completed_at        = COALESCE($5, completed_at),
               timeout_at          = COALESCE($6, timeout_at),
               failure_class       = COALESCE($7, failure_class),
               result_summary      = COALESCE($8, result_summary),
               metadata            = COALESCE($9, metadata),
               updated_at          = now()
             WHERE id = $1
             RETURNING id, label, issue_ref, status, assigned_session_id,
                       started_at, completed_at, timeout_at, failure_class,
                       result_summary, metadata, created_at, updated_at",
        )
        .bind(id)
        .bind(&req.status)
        .bind(&req.assigned_session_id)
        .bind(started_at)
        .bind(completed_at)
        .bind(req.timeout_at)
        .bind(&req.failure_class)
        .bind(&req.result_summary)
        .bind(&req.metadata)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => task_from_row(&r),
            None => Err(AppError::NotFound(format!("task {id} not found"))),
        }
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    pub async fn stats(&self) -> AppResult<TaskStats> {
        // Per-status counts (all time).
        let count_row = sqlx::query(
            "SELECT
               COUNT(*) FILTER (WHERE status = 'pending')  AS pending,
               COUNT(*) FILTER (WHERE status = 'assigned') AS assigned,
               COUNT(*) FILTER (WHERE status = 'running')  AS running,
               COUNT(*) FILTER (WHERE status = 'done')     AS done,
               COUNT(*) FILTER (WHERE status = 'failed')   AS failed
             FROM covalence.tasks",
        )
        .fetch_one(&self.pool)
        .await?;

        let counts = StatusCounts {
            pending: count_row.try_get::<i64, _>("pending").unwrap_or(0),
            assigned: count_row.try_get::<i64, _>("assigned").unwrap_or(0),
            running: count_row.try_get::<i64, _>("running").unwrap_or(0),
            done: count_row.try_get::<i64, _>("done").unwrap_or(0),
            failed: count_row.try_get::<i64, _>("failed").unwrap_or(0),
        };

        // Lead time = completed_at - started_at for tasks that have both and
        // were completed within the last 7 days.
        let lt_row = sqlx::query(
            "SELECT
               AVG(EXTRACT(EPOCH FROM (completed_at - started_at))) AS avg_secs,
               PERCENTILE_CONT(0.95) WITHIN GROUP (
                 ORDER BY EXTRACT(EPOCH FROM (completed_at - started_at))
               ) AS p95_secs
             FROM covalence.tasks
             WHERE status IN ('done', 'failed')
               AND started_at IS NOT NULL
               AND completed_at IS NOT NULL
               AND completed_at >= now() - interval '7 days'",
        )
        .fetch_one(&self.pool)
        .await?;

        let avg_lead_time_secs: Option<f64> = lt_row.try_get("avg_secs").unwrap_or(None);
        let p95_lead_time_secs: Option<f64> = lt_row.try_get("p95_secs").unwrap_or(None);

        Ok(TaskStats {
            counts,
            avg_lead_time_secs,
            p95_lead_time_secs,
        })
    }

    // ── Auto-timeout ──────────────────────────────────────────────────────────

    /// Mark any running tasks whose `timeout_at` has passed as `failed` with
    /// `failure_class = 'timeout'`.  Returns the number of tasks timed out.
    pub async fn timeout_stale(&self) -> AppResult<u64> {
        let result = sqlx::query(
            "UPDATE covalence.tasks
             SET status        = 'failed',
                 failure_class = 'timeout',
                 completed_at  = now(),
                 updated_at    = now()
             WHERE status    = 'running'
               AND timeout_at IS NOT NULL
               AND timeout_at < now()",
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

// ─── Row mapper ───────────────────────────────────────────────────────────────

fn task_from_row(row: &sqlx::postgres::PgRow) -> AppResult<Task> {
    Ok(Task {
        id: row.try_get("id")?,
        label: row.try_get("label")?,
        issue_ref: row.try_get("issue_ref")?,
        status: row.try_get("status")?,
        assigned_session_id: row.try_get("assigned_session_id")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        timeout_at: row.try_get("timeout_at")?,
        failure_class: row.try_get("failure_class")?,
        result_summary: row.try_get("result_summary")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
