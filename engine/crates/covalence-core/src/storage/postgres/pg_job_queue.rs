//! JobQueueRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::retry_job::{JobKind, JobStatus, QueueStatusRow, RetryJob};
use crate::storage::traits::JobQueueRepo;
use crate::types::ids::JobId;

use super::PgRepo;

impl JobQueueRepo for PgRepo {
    async fn enqueue(
        &self,
        kind: JobKind,
        payload: serde_json::Value,
        max_attempts: i32,
        idempotency_key: Option<&str>,
    ) -> Result<Option<RetryJob>> {
        let id = JobId::new();
        let row = sqlx::query(
            "INSERT INTO retry_jobs (id, kind, payload, max_attempts, idempotency_key)
             VALUES ($1, $2::job_kind, $3, $4, $5)
             ON CONFLICT (idempotency_key) DO NOTHING
             RETURNING id, kind::text, status::text, payload,
                       next_due, attempt, max_attempts,
                       created_at, updated_at, last_error,
                       dead_reason, idempotency_key",
        )
        .bind(id)
        .bind(kind.as_pg_str())
        .bind(&payload)
        .bind(max_attempts)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| job_from_row(&r)))
    }

    async fn claim_next(&self, kinds: &[JobKind]) -> Result<Option<RetryJob>> {
        let kind_strs: Vec<&str> = kinds.iter().map(JobKind::as_pg_str).collect();
        let row = sqlx::query(
            "WITH claimed AS (
                SELECT id FROM retry_jobs
                WHERE status = 'pending'::job_status
                  AND next_due <= now()
                  AND kind::text = ANY($1)
                ORDER BY next_due
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            )
            UPDATE retry_jobs
            SET status = 'running'::job_status,
                attempt = attempt + 1,
                updated_at = now()
            WHERE id = (SELECT id FROM claimed)
            RETURNING id, kind::text, status::text, payload,
                      next_due, attempt, max_attempts,
                      created_at, updated_at, last_error,
                      dead_reason, idempotency_key",
        )
        .bind(&kind_strs)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| job_from_row(&r)))
    }

    async fn mark_succeeded(&self, id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE retry_jobs
             SET status = 'succeeded'::job_status,
                 idempotency_key = NULL,
                 updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_failed(
        &self,
        id: JobId,
        error: &str,
        backoff_secs: u64,
        force_dead: bool,
    ) -> Result<JobStatus> {
        if force_dead {
            // Permanent failure — go straight to dead-letter.
            sqlx::query(
                "UPDATE retry_jobs
                 SET status = 'dead'::job_status,
                     last_error = $2,
                     dead_reason = $3,
                     updated_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .bind(format!("permanent failure: {error}"))
            .execute(&self.pool)
            .await?;
            return Ok(JobStatus::Dead);
        }

        // Check if attempts are exhausted.
        let row = sqlx::query("SELECT attempt, max_attempts FROM retry_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        let row = row.ok_or_else(|| crate::error::Error::Queue(format!("job not found: {id}")))?;

        let attempt: i32 = row.get("attempt");
        let max_attempts: i32 = row.get("max_attempts");

        if attempt >= max_attempts {
            // Exhausted retries — dead-letter.
            sqlx::query(
                "UPDATE retry_jobs
                 SET status = 'dead'::job_status,
                     last_error = $2,
                     dead_reason = $3,
                     updated_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .bind(format!(
                "exhausted {max_attempts} attempts, last error: {error}"
            ))
            .execute(&self.pool)
            .await?;
            Ok(JobStatus::Dead)
        } else {
            // Reschedule with backoff.
            sqlx::query(
                "UPDATE retry_jobs
                 SET status = 'pending'::job_status,
                     last_error = $2,
                     next_due = now() + ($3 || ' seconds')::interval,
                     updated_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .bind(backoff_secs.to_string())
            .execute(&self.pool)
            .await?;
            Ok(JobStatus::Pending)
        }
    }

    async fn retry_failed(&self, kind: Option<JobKind>) -> Result<u64> {
        let result = match kind {
            Some(k) => {
                sqlx::query(
                    "UPDATE retry_jobs
                     SET status = 'pending'::job_status,
                         next_due = now(),
                         last_error = NULL,
                         updated_at = now()
                     WHERE status = 'failed'::job_status
                       AND kind = $1::job_kind",
                )
                .bind(k.as_pg_str())
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "UPDATE retry_jobs
                     SET status = 'pending'::job_status,
                         next_due = now(),
                         last_error = NULL,
                         updated_at = now()
                     WHERE status = 'failed'::job_status",
                )
                .execute(&self.pool)
                .await?
            }
        };
        Ok(result.rows_affected())
    }

    async fn queue_status(&self) -> Result<Vec<QueueStatusRow>> {
        let rows = sqlx::query(
            "SELECT kind::text, status::text, count(*)
             FROM retry_jobs
             GROUP BY kind, status
             ORDER BY kind, status",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| QueueStatusRow {
                kind: r.get("kind"),
                status: r.get("status"),
                count: r.get("count"),
            })
            .collect())
    }

    async fn clear_dead(&self, kind: Option<JobKind>) -> Result<u64> {
        let result = match kind {
            Some(k) => {
                sqlx::query(
                    "DELETE FROM retry_jobs
                     WHERE status = 'dead'::job_status
                       AND kind = $1::job_kind",
                )
                .bind(k.as_pg_str())
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "DELETE FROM retry_jobs
                     WHERE status = 'dead'::job_status",
                )
                .execute(&self.pool)
                .await?
            }
        };
        Ok(result.rows_affected())
    }

    async fn list_dead(&self, limit: i64) -> Result<Vec<RetryJob>> {
        let rows = sqlx::query(
            "SELECT id, kind::text, status::text, payload,
                    next_due, attempt, max_attempts,
                    created_at, updated_at, last_error,
                    dead_reason, idempotency_key
             FROM retry_jobs
             WHERE status = 'dead'::job_status
             ORDER BY updated_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(job_from_row).collect())
    }

    async fn resurrect_dead(&self, kind: Option<JobKind>) -> Result<u64> {
        let result = match kind {
            Some(k) => {
                sqlx::query(
                    "UPDATE retry_jobs
                     SET status = 'pending'::job_status,
                         attempt = 0,
                         next_due = now(),
                         last_error = NULL,
                         dead_reason = NULL,
                         idempotency_key = NULL,
                         updated_at = now()
                     WHERE status = 'dead'::job_status
                       AND kind = $1::job_kind",
                )
                .bind(k.as_pg_str())
                .execute(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "UPDATE retry_jobs
                     SET status = 'pending'::job_status,
                         attempt = 0,
                         next_due = now(),
                         last_error = NULL,
                         dead_reason = NULL,
                         idempotency_key = NULL,
                         updated_at = now()
                     WHERE status = 'dead'::job_status",
                )
                .execute(&self.pool)
                .await?
            }
        };
        Ok(result.rows_affected())
    }
}

/// Parse a [`RetryJob`] from a PostgreSQL row.
///
/// Expects `kind` and `status` to be cast to `::text` in the query.
fn job_from_row(row: &sqlx::postgres::PgRow) -> RetryJob {
    let kind_str: String = row.get("kind");
    let status_str: String = row.get("status");
    RetryJob {
        id: row.get("id"),
        kind: JobKind::from_pg_str(&kind_str).unwrap_or(JobKind::ReprocessSource),
        status: JobStatus::from_pg_str(&status_str).unwrap_or(JobStatus::Pending),
        payload: row.get("payload"),
        next_due: row.get("next_due"),
        attempt: row.get("attempt"),
        max_attempts: row.get("max_attempts"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        last_error: row.get("last_error"),
        dead_reason: row.get("dead_reason"),
        idempotency_key: row.get("idempotency_key"),
    }
}
