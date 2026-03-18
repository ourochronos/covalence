//! Persistent retry queue service with background worker.
//!
//! Jobs are persisted in PostgreSQL and processed by a polling
//! worker loop with per-kind concurrency limits and exponential
//! backoff on failure.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::config::RetryQueueConfig;
use crate::error::Result;
use crate::models::retry_job::{JobKind, QueueStatusRow, RetryJob};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::JobQueueRepo;
use crate::types::ids::{JobId, SourceId};

use super::admin::AdminService;
use super::source::SourceService;

/// Persistent retry queue service.
///
/// Manages job enqueuing, background processing, and dead-letter
/// queue administration. Jobs are persisted in PostgreSQL and
/// processed by a polling worker loop.
pub struct RetryQueueService {
    /// Database repository for `JobQueueRepo` calls.
    repo: Arc<PgRepo>,
    /// Queue configuration (poll interval, backoff, concurrency).
    config: RetryQueueConfig,
    /// Semaphore controlling concurrent reprocess jobs.
    reprocess_sem: Arc<Semaphore>,
    /// Semaphore controlling concurrent edge synthesis jobs.
    edge_sem: Arc<Semaphore>,
    /// Lazily-set source service for executing reprocess jobs.
    source_service: tokio::sync::OnceCell<Arc<SourceService>>,
    /// Lazily-set admin service for edge synthesis and other admin jobs.
    admin_service: tokio::sync::OnceCell<Arc<AdminService>>,
}

impl RetryQueueService {
    /// Create a new retry queue service.
    pub fn new(repo: Arc<PgRepo>, config: RetryQueueConfig) -> Self {
        let reprocess_sem = Arc::new(Semaphore::new(config.reprocess_concurrency));
        let edge_sem = Arc::new(Semaphore::new(config.edge_concurrency));
        Self {
            repo,
            config,
            reprocess_sem,
            edge_sem,
            source_service: tokio::sync::OnceCell::new(),
            admin_service: tokio::sync::OnceCell::new(),
        }
    }

    /// Set the source service for executing reprocess jobs.
    ///
    /// Must be called after construction (before the worker loop
    /// starts) to break the circular dependency between
    /// `RetryQueueService` and `SourceService`.
    pub fn set_source_service(&self, svc: Arc<SourceService>) {
        let _ = self.source_service.set(svc);
    }

    /// Set the admin service for edge synthesis and admin jobs.
    pub fn set_admin_service(&self, svc: Arc<AdminService>) {
        let _ = self.admin_service.set(svc);
    }

    /// Enqueue a source reprocess job.
    ///
    /// Idempotent: returns `None` if a pending job for this source
    /// already exists.
    pub async fn enqueue_reprocess(&self, source_id: SourceId) -> Result<Option<JobId>> {
        let payload = serde_json::json!({ "source_id": source_id.into_uuid().to_string() });
        let key = format!("reprocess:{}", source_id);
        let job = JobQueueRepo::enqueue(
            &*self.repo,
            JobKind::ReprocessSource,
            payload,
            self.config.max_attempts,
            Some(&key),
        )
        .await?;
        Ok(job.map(|j| j.id))
    }

    /// Enqueue per-chunk extraction jobs for all chunks of a source.
    ///
    /// Use this after the monolithic reprocess has created chunks.
    /// The extraction → summary → compose pipeline then runs via
    /// the async job DAG with fan-in triggers.
    pub async fn enqueue_extract_chunks(&self, source_id: SourceId) -> Result<u64> {
        let chunks: Vec<(uuid::Uuid,)> =
            sqlx::query_as("SELECT id FROM chunks WHERE source_id = $1")
                .bind(source_id)
                .fetch_all(self.repo.pool())
                .await?;

        let mut enqueued = 0u64;
        for (chunk_id,) in &chunks {
            let payload = serde_json::json!({
                "chunk_id": chunk_id.to_string(),
                "source_id": source_id.into_uuid().to_string(),
            });
            let key = format!("extract_chunk:{chunk_id}");
            match JobQueueRepo::enqueue(
                &*self.repo,
                JobKind::ExtractChunk,
                payload,
                self.config.max_attempts,
                Some(&key),
            )
            .await
            {
                Ok(Some(_)) => enqueued += 1,
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(chunk_id = %chunk_id, error = %e, "failed to enqueue extract job");
                }
            }
        }

        tracing::info!(
            source_id = %source_id,
            chunks = chunks.len(),
            enqueued,
            "enqueued extract_chunk jobs"
        );
        Ok(enqueued)
    }

    /// Enqueue semantic summary jobs for all unsummarized code entities.
    ///
    /// Returns the number of jobs enqueued.
    pub async fn enqueue_summarize_all(&self) -> Result<u64> {
        // Find code entities that need summaries.
        let rows: Vec<(uuid::Uuid, uuid::Uuid)> = sqlx::query_as(
            "SELECT DISTINCT n.id, COALESCE( \
               (SELECT c.source_id FROM extractions ex \
                JOIN chunks c ON c.id = ex.chunk_id \
                WHERE ex.entity_id = n.id AND ex.entity_type = 'node' \
                LIMIT 1), \
               '00000000-0000-0000-0000-000000000000'::uuid \
             ) as source_id \
             FROM nodes n \
             WHERE n.entity_class = 'code' \
               AND (n.properties->>'semantic_summary' IS NULL \
                    OR n.properties->>'semantic_summary' = '') \
               AND n.node_type != 'code_test' \
               AND n.canonical_name NOT LIKE 'test_%'",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let mut enqueued = 0u64;
        for (node_id, source_id) in &rows {
            let payload = serde_json::json!({
                "node_id": node_id.to_string(),
                "source_id": source_id.to_string(),
            });
            let key = format!("summarize:{node_id}");
            match JobQueueRepo::enqueue(
                &*self.repo,
                JobKind::SummarizeEntity,
                payload,
                self.config.max_attempts,
                Some(&key),
            )
            .await
            {
                Ok(Some(_)) => enqueued += 1,
                Ok(None) => {} // already enqueued
                Err(e) => {
                    tracing::warn!(node_id = %node_id, error = %e, "failed to enqueue summary job");
                }
            }
        }

        tracing::info!(
            enqueued,
            total = rows.len(),
            "enqueued summarize_entity jobs"
        );
        Ok(enqueued)
    }

    /// Enqueue source summary composition jobs for code sources that
    /// have entity summaries but no file-level summary yet.
    pub async fn enqueue_compose_all(&self) -> Result<u64> {
        let rows: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT s.id \
             FROM sources s \
             WHERE s.domain = 'code' \
               AND s.summary IS NULL \
               AND EXISTS ( \
                 SELECT 1 FROM nodes n \
                 JOIN extractions ex ON ex.entity_id = n.id AND ex.entity_type = 'node' \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 WHERE c.source_id = s.id \
                   AND n.entity_class = 'code' \
                   AND n.properties->>'semantic_summary' IS NOT NULL \
               )",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let mut enqueued = 0u64;
        for (source_id,) in &rows {
            let payload = serde_json::json!({ "source_id": source_id.to_string() });
            let key = format!("compose:{source_id}");
            match JobQueueRepo::enqueue(
                &*self.repo,
                JobKind::ComposeSourceSummary,
                payload,
                self.config.max_attempts,
                Some(&key),
            )
            .await
            {
                Ok(Some(_)) => enqueued += 1,
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(source_id = %source_id, error = %e, "failed to enqueue compose job");
                }
            }
        }

        tracing::info!(
            enqueued,
            total = rows.len(),
            "enqueued compose_source_summary jobs"
        );
        Ok(enqueued)
    }

    /// Enqueue an edge synthesis job.
    ///
    /// Idempotent: returns `None` if a pending edge synthesis job
    /// already exists.
    pub async fn enqueue_edge_synthesis(&self, min_cooccurrences: i32) -> Result<Option<JobId>> {
        let payload = serde_json::json!({ "min_cooccurrences": min_cooccurrences });
        let key = "synthesize_edges".to_string();
        let job = JobQueueRepo::enqueue(
            &*self.repo,
            JobKind::SynthesizeEdges,
            payload,
            self.config.max_attempts,
            Some(&key),
        )
        .await?;
        Ok(job.map(|j| j.id))
    }

    /// Spawn the pipeline watchdog as a background task.
    ///
    /// Periodically checks for sources that may have stalled mid-pipeline
    /// (all child jobs completed but fan-in didn't trigger) and re-runs
    /// the fan-in check.
    pub fn spawn_watchdog(self: &Arc<Self>) {
        let repo = Arc::clone(&self.repo);
        let interval = std::time::Duration::from_secs(120); // every 2 minutes

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                // Find code sources with entity summaries but no file summary
                // and no pending compose job — these are stalled.
                let stalled: Vec<(uuid::Uuid,)> = sqlx::query_as(
                    "SELECT s.id FROM sources s \
                     WHERE s.domain = 'code' \
                       AND s.summary IS NULL \
                       AND EXISTS ( \
                         SELECT 1 FROM nodes n \
                         JOIN extractions ex ON ex.entity_id = n.id \
                         JOIN chunks c ON c.id = ex.chunk_id \
                         WHERE c.source_id = s.id \
                           AND n.entity_class = 'code' \
                           AND n.properties->>'semantic_summary' IS NOT NULL \
                       ) \
                       AND NOT EXISTS ( \
                         SELECT 1 FROM retry_jobs rj \
                         WHERE rj.kind = 'compose_source_summary' \
                           AND rj.payload->>'source_id' = s.id::text \
                           AND rj.status IN ('pending', 'running') \
                       ) \
                     LIMIT 20",
                )
                .fetch_all(repo.pool())
                .await
                .unwrap_or_default();

                if !stalled.is_empty() {
                    tracing::info!(
                        count = stalled.len(),
                        "watchdog: detected stalled sources, enqueuing compose jobs"
                    );
                    for (sid,) in &stalled {
                        let payload = serde_json::json!({ "source_id": sid.to_string() });
                        let key = format!("compose:{sid}");
                        let _ = sqlx::query(
                            "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
                             VALUES ('compose_source_summary', $1, $2, 5) \
                             ON CONFLICT (idempotency_key) DO NOTHING",
                        )
                        .bind(&payload)
                        .bind(&key)
                        .execute(repo.pool())
                        .await;
                    }
                }
            }
        });
    }

    /// Spawn the periodic scheduler as a background task.
    ///
    /// Automatically enqueues maintenance jobs on a schedule:
    /// - Edge synthesis: every 6 hours
    /// - Ontology clustering: every 24 hours
    /// - Garbage collection: every 7 days
    pub fn spawn_scheduler(self: &Arc<Self>) {
        let repo = Arc::clone(&self.repo);
        let max_attempts = self.config.max_attempts;

        tokio::spawn(async move {
            let mut edge_interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            let mut gc_interval =
                tokio::time::interval(std::time::Duration::from_secs(7 * 24 * 3600));

            // Skip the first immediate tick.
            edge_interval.tick().await;
            gc_interval.tick().await;

            loop {
                tokio::select! {
                    _ = edge_interval.tick() => {
                        let payload = serde_json::json!({
                            "min_cooccurrences": 2,
                            "max_degree": 2,
                        });
                        let key = "scheduled:synthesize_edges".to_string();
                        match JobQueueRepo::enqueue(
                            &*repo,
                            JobKind::SynthesizeEdges,
                            payload,
                            max_attempts,
                            Some(&key),
                        ).await {
                            Ok(Some(_)) => tracing::info!("scheduler: enqueued edge synthesis"),
                            Ok(None) => {} // already pending
                            Err(e) => tracing::warn!(error = %e, "scheduler: failed to enqueue edge synthesis"),
                        }
                    }
                    _ = gc_interval.tick() => {
                        tracing::info!("scheduler: gc tick (placeholder — wire to admin.gc when ready)");
                    }
                }
            }
        });
    }

    /// Background worker loop. Spawned as a tokio task. Never returns.
    ///
    /// On startup, recovers orphaned `running` jobs (from engine
    /// crashes) back to `pending`. Then polls for work in a loop,
    /// respecting per-kind concurrency limits.
    pub async fn run_worker(&self) {
        // Recovery pass: reset orphaned running jobs.
        if let Err(e) = self.recover_orphaned_jobs().await {
            tracing::warn!(error = %e, "failed to recover orphaned jobs");
        }

        loop {
            let did_work = self.poll_once().await;
            if !did_work {
                tokio::time::sleep(std::time::Duration::from_secs(
                    self.config.poll_interval_secs,
                ))
                .await;
            }
        }
    }

    /// Admin: retry all failed/dead jobs, optionally filtered by kind.
    ///
    /// Returns the number of jobs retried.
    pub async fn retry_failed(&self, kind: Option<JobKind>) -> Result<u64> {
        JobQueueRepo::retry_failed(&*self.repo, kind).await
    }

    /// Admin: queue status summary grouped by kind and status.
    pub async fn queue_status(&self) -> Result<Vec<QueueStatusRow>> {
        JobQueueRepo::queue_status(&*self.repo).await
    }

    /// Admin: list dead-letter jobs.
    pub async fn list_dead(&self, limit: i64) -> Result<Vec<RetryJob>> {
        JobQueueRepo::list_dead(&*self.repo, limit).await
    }

    /// Admin: clear dead-letter queue, optionally filtered by kind.
    ///
    /// Returns the number of jobs deleted.
    pub async fn clear_dead(&self, kind: Option<JobKind>) -> Result<u64> {
        JobQueueRepo::clear_dead(&*self.repo, kind).await
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Recovery pass: reset orphaned `running` jobs back to `pending`.
    async fn recover_orphaned_jobs(&self) -> Result<()> {
        let result = sqlx::query(
            "UPDATE retry_jobs
             SET status = 'pending'::job_status,
                 next_due = now(),
                 updated_at = now()
             WHERE status = 'running'::job_status",
        )
        .execute(self.repo.pool())
        .await?;

        let recovered = result.rows_affected();
        if recovered > 0 {
            tracing::info!(recovered, "recovered orphaned running jobs");
        }
        Ok(())
    }

    /// Try to claim and dispatch jobs up to available concurrency.
    /// Returns `true` if any work was found.
    async fn poll_once(&self) -> bool {
        let reprocess_kinds = [
            JobKind::ReprocessSource,
            JobKind::ExtractStatements,
            JobKind::ExtractEntities,
            JobKind::ExtractChunk,
            JobKind::SummarizeEntity,
            JobKind::ComposeSourceSummary,
            JobKind::EmbedBatch,
        ];

        let mut did_work = false;

        // Claim as many reprocess-family jobs as we have permits.
        loop {
            let permit = match self.reprocess_sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => break, // All permits taken.
            };
            match JobQueueRepo::claim_next(&*self.repo, &reprocess_kinds).await {
                Ok(Some(job)) => {
                    self.spawn_job(job, permit);
                    did_work = true;
                }
                Ok(None) => {
                    drop(permit);
                    break; // No more work.
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to claim reprocess job");
                    drop(permit);
                    break;
                }
            }
        }

        // Try edge synthesis work.
        if let Ok(permit) = self.edge_sem.clone().try_acquire_owned() {
            match JobQueueRepo::claim_next(&*self.repo, &[JobKind::SynthesizeEdges]).await {
                Ok(Some(job)) => {
                    self.spawn_job(job, permit);
                    did_work = true;
                }
                Ok(None) => {
                    drop(permit);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to claim edge job");
                    drop(permit);
                }
            }
        }

        did_work
    }

    /// Spawn a tokio task that executes the job and holds the
    /// semaphore permit until completion.
    fn spawn_job(&self, job: RetryJob, permit: tokio::sync::OwnedSemaphorePermit) {
        let repo = Arc::clone(&self.repo);
        let source_service = self.source_service.get().cloned();
        let admin_service = self.admin_service.get().cloned();
        let base_backoff = self.config.base_backoff_secs;
        let max_backoff = self.config.max_backoff_secs;
        let job_timeout = std::time::Duration::from_secs(self.config.job_timeout_secs);

        tokio::spawn(async move {
            let job_id = job.id;
            let job_kind = job.kind;
            tracing::info!(
                job_id = %job_id,
                kind = ?job_kind,
                attempt = job.attempt,
                "executing queue job"
            );

            let result = match tokio::time::timeout(
                job_timeout,
                execute_job(&job, source_service.as_ref(), admin_service.as_ref()),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => Err(crate::error::Error::Ingestion(format!(
                    "job timed out after {}s",
                    job_timeout.as_secs()
                ))),
            };

            match result {
                Ok(()) => {
                    tracing::info!(job_id = %job_id, kind = ?job_kind, "job succeeded");
                    if let Err(e) = JobQueueRepo::mark_succeeded(&*repo, job_id).await {
                        tracing::error!(
                            job_id = %job_id,
                            error = %e,
                            "failed to mark job as succeeded"
                        );
                    }
                }
                Err(e) => {
                    let class = classify_error(&e);
                    let backoff =
                        compute_backoff_for_class(class, base_backoff, job.attempt, max_backoff);
                    tracing::warn!(
                        job_id = %job_id,
                        kind = ?job_kind,
                        attempt = job.attempt,
                        failure_class = ?class,
                        backoff_secs = backoff,
                        error = %e,
                        "job failed"
                    );

                    let force_dead = class == FailureClass::Permanent;
                    let error_msg = format!("[{class:?}] {e}");
                    if let Err(mark_err) =
                        JobQueueRepo::mark_failed(&*repo, job_id, &error_msg, backoff, force_dead)
                            .await
                    {
                        tracing::error!(
                            job_id = %job_id,
                            error = %mark_err,
                            "failed to mark job as failed"
                        );
                    }
                }
            }

            // Drop permit last — keeps the semaphore held during execution.
            drop(permit);
        });
    }
}

/// Parse a typed payload from a job's JSON, producing a clear error on failure.
fn parse_payload<T: serde::de::DeserializeOwned>(job: &RetryJob) -> Result<T> {
    serde_json::from_value(job.payload.clone())
        .map_err(|e| crate::error::Error::Queue(format!("invalid payload for {:?}: {e}", job.kind)))
}

/// Parse a UUID string, producing a clear queue error on failure.
fn parse_uuid(s: &str, field: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(s)
        .map_err(|e| crate::error::Error::Queue(format!("invalid {field} UUID: {e}")))
}

/// Require a service or return a queue error.
fn require_svc<'a, T>(svc: Option<&'a Arc<T>>, name: &str) -> Result<&'a Arc<T>> {
    svc.ok_or_else(|| crate::error::Error::Queue(format!("{name} not available")))
}

/// Execute a single job based on its kind and typed payload.
async fn execute_job(
    job: &RetryJob,
    source_service: Option<&Arc<SourceService>>,
    admin_service: Option<&Arc<AdminService>>,
) -> Result<()> {
    use crate::models::retry_job::*;

    match job.kind {
        JobKind::ReprocessSource => {
            let p: SourcePayload = parse_payload(job)?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            require_svc(source_service, "source_service")?
                .reprocess(source_id)
                .await?;
            Ok(())
        }
        JobKind::SynthesizeEdges => {
            let p: SynthesizePayload = parse_payload(job)?;
            let svc = require_svc(admin_service, "admin_service")?;
            let result = svc
                .synthesize_cooccurrence_edges(p.min_cooccurrences, p.max_degree)
                .await?;
            tracing::info!(
                edges_created = result.edges_created,
                candidates = result.candidates_evaluated,
                "edge synthesis job complete"
            );
            Ok(())
        }
        JobKind::ExtractStatements | JobKind::ExtractEntities => {
            tracing::warn!(
                kind = ?job.kind,
                "job kind not yet independently implemented, marking as succeeded"
            );
            Ok(())
        }
        JobKind::ExtractChunk => {
            let p: ExtractChunkPayload = parse_payload(job)?;
            let chunk_uuid = parse_uuid(&p.chunk_id, "chunk_id")?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            extract_single_chunk(svc, chunk_uuid, source_id, job).await
        }
        JobKind::SummarizeEntity => {
            let p: SummarizePayload = parse_payload(job)?;
            let node_id = crate::types::ids::NodeId::from_uuid(parse_uuid(&p.node_id, "node_id")?);
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            summarize_single_entity(svc, node_id, source_id).await
        }
        JobKind::ComposeSourceSummary => {
            let p: SourcePayload = parse_payload(job)?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            compose_source_summary_job(svc, source_id).await
        }
        JobKind::EmbedBatch => {
            let p: EmbedBatchPayload = parse_payload(job)?;
            let item_ids: Vec<uuid::Uuid> = p
                .item_ids
                .iter()
                .filter_map(|s| uuid::Uuid::parse_str(s).ok())
                .collect();
            let svc = require_svc(source_service, "source_service")?;
            embed_batch_job(svc, &p.item_table, &item_ids).await
        }
    }
}

/// How a failed job should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Transient error (timeout, connection reset) — normal backoff.
    Transient,
    /// Rate limit or quota exhaustion — long backoff (wait for reset).
    RateLimit,
    /// Permanent error (not found, bad payload) — dead immediately.
    Permanent,
}

/// Classify an error to determine retry strategy.
pub fn classify_error(err: &crate::error::Error) -> FailureClass {
    use crate::error::Error;
    match err {
        // Source deleted or missing → permanent, no point retrying.
        Error::NotFound { .. } => FailureClass::Permanent,
        // Bad queue payload → permanent.
        Error::Queue(msg) if msg.contains("missing") || msg.contains("invalid") => {
            FailureClass::Permanent
        }
        // Ingestion errors need string inspection for rate limits.
        Error::Ingestion(msg) => classify_ingestion_error(msg),
        // Everything else is transient.
        _ => FailureClass::Transient,
    }
}

/// Inspect ingestion error messages for rate limit / quota patterns.
fn classify_ingestion_error(msg: &str) -> FailureClass {
    let lower = msg.to_lowercase();
    if lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("quota")
        || lower.contains("402 payment required")
        || lower.contains("too many requests")
        || lower.contains("capacity")
        || lower.contains("exhausted")
    {
        FailureClass::RateLimit
    } else if lower.contains("not found")
        || lower.contains("404")
        || lower.contains("no raw content")
    {
        FailureClass::Permanent
    } else {
        FailureClass::Transient
    }
}

/// Compute backoff delay in seconds based on failure class.
///
/// - `Transient`: exponential backoff `base * 2^(attempt-1)`, capped at `max`.
/// - `RateLimit`: starts at 15 minutes, doubles up to `max` (typically 1h).
/// - `Permanent`: returns 0 (will be sent to dead-letter immediately).
pub fn compute_backoff_for_class(
    class: FailureClass,
    base_secs: u64,
    attempt: i32,
    max_secs: u64,
) -> u64 {
    match class {
        FailureClass::Permanent => 0,
        FailureClass::RateLimit => {
            // Rate limits: start at 15 min, double each retry, cap at max.
            let rate_base = 900u64; // 15 minutes
            let exp = attempt.saturating_sub(1) as u32;
            rate_base
                .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX))
                .min(max_secs)
        }
        FailureClass::Transient => compute_backoff(base_secs, attempt, max_secs),
    }
}

/// Compute exponential backoff delay in seconds.
///
/// Formula: `base * 2^(attempt - 1)`, clamped to `max`.
pub fn compute_backoff(base_secs: u64, attempt: i32, max_secs: u64) -> u64 {
    let exp = attempt.saturating_sub(1) as u32;
    base_secs
        .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX))
        .min(max_secs)
}

/// Generate a semantic summary for a single code entity.
///
/// This is the async-pipeline equivalent of the sequential summary
/// loop in pipeline.rs Stage 7.25. Each entity gets its own job
/// with independent retry.
async fn summarize_single_entity(
    svc: &Arc<SourceService>,
    node_id: crate::types::ids::NodeId,
    source_id: SourceId,
) -> Result<()> {
    use crate::storage::traits::NodeRepo;
    use std::time::Instant;

    let chat = svc
        .chat_backend
        .as_ref()
        .ok_or_else(|| crate::error::Error::Queue("no chat backend for summaries".to_string()))?;

    let node =
        NodeRepo::get(&*svc.repo, node_id)
            .await?
            .ok_or_else(|| crate::error::Error::NotFound {
                entity_type: "node",
                id: node_id.into_uuid().to_string(),
            })?;

    // Skip if already summarized.
    if node
        .properties
        .get("semantic_summary")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        return Ok(());
    }

    // Build definition pattern for chunk lookup.
    let def_pattern = match node.node_type.as_str() {
        "function" => format!("fn {}", node.canonical_name),
        "struct" => format!("struct {}", node.canonical_name),
        "enum" => format!("enum {}", node.canonical_name),
        "trait" => format!("trait {}", node.canonical_name),
        "impl_block" => format!("impl {}", node.canonical_name),
        "module" => format!("mod {}", node.canonical_name),
        "constant" => format!("const {}", node.canonical_name),
        "macro" => format!("macro_rules! {}", node.canonical_name),
        _ => node.canonical_name.clone(),
    };

    // Find the chunk containing this entity's definition.
    let mut chunk_content: Option<String> = sqlx::query_scalar(
        "SELECT c.content FROM extractions ex \
         JOIN chunks c ON c.id = ex.chunk_id \
         WHERE ex.entity_id = $1 AND ex.entity_type = 'node' \
           AND ex.chunk_id IS NOT NULL \
           AND c.content LIKE '%' || $2 || '%' \
         ORDER BY ex.confidence DESC \
         LIMIT 1",
    )
    .bind(node_id)
    .bind(&def_pattern)
    .fetch_optional(svc.repo.pool())
    .await
    .ok()
    .flatten();

    // Fallback: search all chunks from the same source.
    if chunk_content.is_none() {
        chunk_content = sqlx::query_scalar(
            "SELECT c.content FROM chunks c \
             WHERE c.source_id = $1 \
               AND c.content LIKE '%' || $2 || '%' \
             ORDER BY LENGTH(c.content) ASC \
             LIMIT 1",
        )
        .bind(source_id)
        .bind(&def_pattern)
        .fetch_optional(svc.repo.pool())
        .await
        .ok()
        .flatten();
    }

    let raw = chunk_content
        .as_deref()
        .or(node.description.as_deref())
        .unwrap_or(&node.canonical_name);

    if raw.len() < 50 {
        return Ok(()); // Too short to summarize.
    }

    let file_path = node
        .properties
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let prompt =
        super::prompts::build_summary_prompt(&node.canonical_name, &node.node_type, file_path, raw);

    let start = Instant::now();
    let summary = chat.chat("", &prompt, false, 0.2).await?;
    let duration_ms = start.elapsed().as_millis() as i64;

    let summary = summary.trim();
    if summary.is_empty() {
        return Ok(());
    }

    // Store summary + processing metadata on the node.
    sqlx::query(
        "UPDATE nodes SET \
           properties = jsonb_set(\
             COALESCE(properties, '{}'), \
             '{semantic_summary}', \
             $2::jsonb\
           ), \
           description = $3, \
           embedding = NULL, \
           processing = jsonb_set(\
             COALESCE(processing, '{}'), \
             '{summary}', \
             $4::jsonb\
           ) \
         WHERE id = $1",
    )
    .bind(node_id)
    .bind(serde_json::json!(summary))
    .bind(summary)
    .bind(serde_json::json!({
        "model": "haiku",
        "at": chrono::Utc::now().to_rfc3339(),
        "ms": duration_ms,
        "prompt_version": super::prompts::SUMMARY_PROMPT_VERSION,
        "input_chars": raw.len().min(3000),
        "output_chars": summary.len(),
    }))
    .execute(svc.repo.pool())
    .await?;

    tracing::info!(
        node = %node.canonical_name,
        ms = duration_ms,
        "semantic summary generated (async job)"
    );

    // Fan-in: check if all summary jobs for this source are done.
    // If so, auto-enqueue ComposeSourceSummary.
    let nil = uuid::Uuid::nil();
    if source_id.into_uuid() != nil {
        try_advance_to_compose(svc.repo.pool(), source_id).await;
    }

    Ok(())
}

/// Fan-in trigger: check if all SummarizeEntity jobs for a source
/// are done. If so, enqueue a ComposeSourceSummary job.
///
/// Uses an advisory lock on the source_id to prevent two concurrent
/// completions from both triggering the compose stage.
async fn try_advance_to_compose(pool: &sqlx::PgPool, source_id: SourceId) {
    let sid = source_id.into_uuid();
    let lock_key = (sid.as_u128() & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    // Use a transaction-scoped advisory lock. Unlike session-level locks,
    // these auto-release on commit/rollback/disconnect — no leak risk if
    // the async Future is dropped.
    let Ok(mut tx) = pool.begin().await else {
        return;
    };

    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(lock_key)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(false);

    if !acquired {
        return; // Another worker is handling this fan-in.
    }

    // Check: any pending/running summarize_entity jobs for this source?
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM retry_jobs \
         WHERE kind = 'summarize_entity' \
           AND status IN ('pending', 'running') \
           AND payload->>'source_id' = $1",
    )
    .bind(sid.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap_or(1);

    if remaining == 0 {
        let has_summary: bool =
            sqlx::query_scalar("SELECT summary IS NOT NULL FROM sources WHERE id = $1")
                .bind(sid)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(true);

        if !has_summary {
            let payload = serde_json::json!({ "source_id": sid.to_string() });
            let key = format!("compose:{sid}");
            let _ = sqlx::query(
                "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
                 VALUES ('compose_source_summary', $1, $2, 5) \
                 ON CONFLICT (idempotency_key) DO NOTHING",
            )
            .bind(payload)
            .bind(&key)
            .execute(&mut *tx)
            .await;

            tracing::info!(
                source_id = %sid,
                "fan-in: all summaries complete, enqueued compose_source_summary"
            );
        }
    }

    // Lock auto-releases on commit.
    let _ = tx.commit().await;
}

/// Extract entities from a single chunk via the LLM extractor.
async fn extract_single_chunk(
    svc: &Arc<SourceService>,
    chunk_id: uuid::Uuid,
    source_id: SourceId,
    job: &RetryJob,
) -> Result<()> {
    use crate::ingestion::extractor::ExtractionContext;
    use crate::services::pipeline::ExtractionProvenance;
    use crate::storage::traits::{ChunkRepo, SourceRepo};
    use crate::types::ids::ChunkId;
    use std::time::Instant;

    let extractor = svc.extractor.as_ref().ok_or_else(|| {
        crate::error::Error::Queue("no extractor available for extract_chunk".to_string())
    })?;

    let chunk_id_typed = ChunkId::from_uuid(chunk_id);
    let chunk = ChunkRepo::get(&*svc.repo, chunk_id_typed)
        .await?
        .ok_or_else(|| crate::error::Error::NotFound {
            entity_type: "chunk",
            id: chunk_id.to_string(),
        })?;

    let source = SourceRepo::get(&*svc.repo, source_id).await?;
    let context = ExtractionContext {
        source_type: source.as_ref().map(|s| s.source_type.clone()),
        source_uri: source.as_ref().and_then(|s| s.uri.clone()),
        source_title: source.as_ref().and_then(|s| s.title.clone()),
    };
    let source_domain = source.as_ref().and_then(|s| s.domain.clone());

    let start = Instant::now();
    let result = extractor.extract(&chunk.content, &context).await?;
    let duration_ms = start.elapsed().as_millis() as i64;

    // Store entities and relationships.
    let mut entity_count = 0usize;
    for entity in &result.entities {
        if super::noise_filter::is_noise_entity(&entity.name, &entity.entity_type) {
            continue;
        }
        let node_id = svc
            .resolve_and_store_entity(
                entity,
                ExtractionProvenance::Chunk(chunk_id_typed),
                "llm",
                source_id,
                source_domain.as_deref(),
            )
            .await?;
        if node_id.is_some() {
            entity_count += 1;
        }
    }

    // Mark chunk as processed.
    let ingestion_id = job
        .payload
        .get("ingestion_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    sqlx::query(
        "UPDATE chunks SET processing = jsonb_set(\
           COALESCE(processing, '{}'), '{extraction}', $2::jsonb\
         ) WHERE id = $1",
    )
    .bind(chunk_id)
    .bind(serde_json::json!({
        "model": "haiku",
        "at": chrono::Utc::now().to_rfc3339(),
        "ms": duration_ms,
        "entities_found": entity_count,
        "relationships_found": result.relationships.len(),
        "ingestion_id": ingestion_id,
    }))
    .execute(svc.repo.pool())
    .await?;

    tracing::info!(
        chunk_id = %chunk_id,
        entities = entity_count,
        ms = duration_ms,
        "chunk extraction complete (async job)"
    );

    // Fan-in: check if all ExtractChunk jobs for this source are done.
    // If so, enqueue SummarizeEntity jobs for code entities.
    try_advance_to_summarize(svc.repo.pool(), source_id).await;

    Ok(())
}

/// Fan-in trigger: when all ExtractChunk jobs for a source complete,
/// enqueue SummarizeEntity jobs for the code entities extracted.
async fn try_advance_to_summarize(pool: &sqlx::PgPool, source_id: SourceId) {
    let sid = source_id.into_uuid();
    let lock_key = ((sid.as_u128() >> 1) & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    let Ok(mut tx) = pool.begin().await else {
        return;
    };

    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(lock_key)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(false);

    if !acquired {
        return;
    }

    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM retry_jobs \
         WHERE kind = 'extract_chunk' \
           AND status IN ('pending', 'running') \
           AND payload->>'source_id' = $1",
    )
    .bind(sid.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap_or(1);

    if remaining == 0 {
        let entities: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT n.id \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id AND ex.entity_type = 'node' \
             JOIN chunks c ON c.id = ex.chunk_id \
             WHERE c.source_id = $1 \
               AND n.entity_class = 'code' \
               AND (n.properties->>'semantic_summary' IS NULL \
                    OR n.properties->>'semantic_summary' = '') \
               AND n.node_type != 'code_test' \
               AND n.canonical_name NOT LIKE 'test_%'",
        )
        .bind(sid)
        .fetch_all(&mut *tx)
        .await
        .unwrap_or_default();

        let mut enqueued = 0usize;
        for (node_id,) in &entities {
            let payload = serde_json::json!({
                "node_id": node_id.to_string(),
                "source_id": sid.to_string(),
            });
            let key = format!("summarize:{node_id}");
            if sqlx::query(
                "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
                 VALUES ('summarize_entity', $1, $2, 5) \
                 ON CONFLICT (idempotency_key) DO NOTHING",
            )
            .bind(&payload)
            .bind(&key)
            .execute(&mut *tx)
            .await
            .is_ok()
            {
                enqueued += 1;
            }
        }

        if enqueued > 0 {
            tracing::info!(
                source_id = %sid,
                enqueued,
                "fan-in: all extractions complete, enqueued summarize_entity jobs"
            );
        }
    }

    let _ = tx.commit().await;
}

/// Compose a file-level summary from entity summaries for a code source.
async fn compose_source_summary_job(svc: &Arc<SourceService>, source_id: SourceId) -> Result<()> {
    use crate::ingestion::embedder::truncate_and_validate;
    use crate::ingestion::section_compiler::{SectionSummaryEntry, SourceSummaryInput};
    use crate::storage::traits::SourceRepo;
    use std::time::Instant;

    let summary_compiler = svc.source_summary_compiler.as_ref().ok_or_else(|| {
        crate::error::Error::Queue("no summary compiler for compose_source_summary".to_string())
    })?;

    // Collect entity summaries for this source.
    let summaries: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT canonical_name, node_type, \
                COALESCE(properties->>'semantic_summary', \
                         description, canonical_name) \
         FROM nodes n \
         JOIN extractions ex ON ex.entity_id = n.id \
           AND ex.entity_type = 'node' \
         JOIN chunks c ON c.id = ex.chunk_id \
         WHERE c.source_id = $1 \
           AND n.entity_class = 'code' \
         GROUP BY n.id, canonical_name, node_type, \
                  properties, description \
         ORDER BY n.node_type, n.canonical_name",
    )
    .bind(source_id)
    .fetch_all(svc.repo.pool())
    .await
    .unwrap_or_default();

    if summaries.is_empty() {
        return Ok(());
    }

    let section_entries: Vec<SectionSummaryEntry> = summaries
        .iter()
        .map(|(name, ntype, summary)| SectionSummaryEntry {
            title: format!("{ntype}: {name}"),
            summary: summary.clone(),
        })
        .collect();

    let source = SourceRepo::get(&*svc.repo, source_id).await?;
    let file_name = source
        .as_ref()
        .and_then(|s| s.uri.as_deref())
        .and_then(|u| u.rsplit('/').next())
        .unwrap_or("unknown");

    let start = Instant::now();
    let summary = summary_compiler
        .compile_source_summary(&SourceSummaryInput {
            section_summaries: section_entries,
            source_title: Some(file_name.to_string()),
        })
        .await?;
    let duration_ms = start.elapsed().as_millis() as i64;

    let summary = summary.trim();
    if summary.is_empty() {
        return Ok(());
    }

    SourceRepo::update_summary(&*svc.repo, source_id, summary).await?;

    // Re-embed from composed summary.
    if let Some(ref embedder) = svc.embedder {
        if let Ok(vecs) = embedder.embed(&[summary.to_string()]).await {
            if let Some(emb) = vecs.first() {
                if let Ok(t) = truncate_and_validate(emb, svc.table_dims.source, "sources") {
                    let _ = SourceRepo::update_embedding(&*svc.repo, source_id, &t).await;
                }
            }
        }
    }

    // Record processing metadata.
    sqlx::query(
        "UPDATE sources SET processing = jsonb_set(\
           COALESCE(processing, '{}'), '{compose}', $2::jsonb\
         ) WHERE id = $1",
    )
    .bind(source_id)
    .bind(serde_json::json!({
        "model": "haiku",
        "at": chrono::Utc::now().to_rfc3339(),
        "ms": duration_ms,
        "entities_composed": summaries.len(),
    }))
    .execute(svc.repo.pool())
    .await?;

    tracing::info!(
        source_id = %source_id,
        entities = summaries.len(),
        ms = duration_ms,
        "source summary composed (async job)"
    );

    Ok(())
}

/// Embed a batch of items (nodes or chunks).
async fn embed_batch_job(
    svc: &Arc<SourceService>,
    item_table: &str,
    item_ids: &[uuid::Uuid],
) -> Result<()> {
    use crate::ingestion::embedder::truncate_and_validate;
    use crate::storage::traits::NodeRepo;

    let embedder = svc.embedder.as_ref().ok_or_else(|| {
        crate::error::Error::Queue("no embedder available for embed_batch".to_string())
    })?;

    if item_ids.is_empty() {
        return Ok(());
    }

    match item_table {
        "nodes" => {
            let mut texts = Vec::with_capacity(item_ids.len());
            let mut valid_ids = Vec::with_capacity(item_ids.len());

            for &id in item_ids {
                let node_id = crate::types::ids::NodeId::from_uuid(id);
                if let Ok(Some(node)) = NodeRepo::get(&*svc.repo, node_id).await {
                    // Skip nodes that already have embeddings.
                    let has_emb: bool =
                        sqlx::query_scalar("SELECT embedding IS NOT NULL FROM nodes WHERE id = $1")
                            .bind(id)
                            .fetch_one(svc.repo.pool())
                            .await
                            .unwrap_or(true);

                    if has_emb {
                        continue;
                    }

                    let text = match &node.description {
                        Some(desc) if !desc.is_empty() => {
                            format!("{}: {}", node.canonical_name, desc)
                        }
                        _ => node.canonical_name.clone(),
                    };
                    texts.push(text);
                    valid_ids.push(node_id);
                }
            }

            if !texts.is_empty() {
                let embeddings = embedder.embed(&texts).await?;
                for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                    if let Ok(t) = truncate_and_validate(emb, svc.table_dims.node, "nodes") {
                        let _ = NodeRepo::update_embedding(&*svc.repo, *nid, &t).await;
                    }
                }
                tracing::info!(
                    embedded = valid_ids.len(),
                    "batch node embedding complete (async job)"
                );
            }
        }
        _ => {
            tracing::warn!(table = item_table, "embed_batch: unsupported item table");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_first_attempt() {
        // attempt=1 -> base * 2^0 = base
        assert_eq!(compute_backoff(30, 1, 3600), 30);
    }

    #[test]
    fn backoff_second_attempt() {
        // attempt=2 -> base * 2^1 = 60
        assert_eq!(compute_backoff(30, 2, 3600), 60);
    }

    #[test]
    fn backoff_third_attempt() {
        // attempt=3 -> base * 2^2 = 120
        assert_eq!(compute_backoff(30, 3, 3600), 120);
    }

    #[test]
    fn backoff_clamped_to_max() {
        // attempt=10 -> 30 * 2^9 = 15360, clamped to 3600
        assert_eq!(compute_backoff(30, 10, 3600), 3600);
    }

    #[test]
    fn backoff_zero_attempt() {
        // attempt=0 shouldn't happen in practice, but defensively
        // saturating_sub(0, 1) = -1 as i32, cast to u32 = u32::MAX,
        // which overflows the shift → clamped to max_secs.
        assert_eq!(compute_backoff(30, 0, 3600), 3600);
    }

    #[test]
    fn backoff_large_attempt_no_overflow() {
        // attempt=100 -> should saturate, not panic
        let result = compute_backoff(30, 100, 3600);
        assert_eq!(result, 3600);
    }

    #[test]
    fn backoff_zero_base() {
        assert_eq!(compute_backoff(0, 5, 3600), 0);
    }

    #[test]
    fn backoff_zero_max() {
        assert_eq!(compute_backoff(30, 1, 0), 0);
    }

    #[test]
    fn backoff_progression() {
        let base = 10u64;
        let max = 10_000u64;
        let mut prev = 0u64;
        for attempt in 1..=8 {
            let delay = compute_backoff(base, attempt, max);
            assert!(
                delay >= prev,
                "backoff should be non-decreasing: attempt={attempt}, delay={delay}, prev={prev}"
            );
            prev = delay;
        }
    }

    // --- Error classification tests ---

    #[test]
    fn classify_not_found_is_permanent() {
        let err = crate::error::Error::NotFound {
            entity_type: "source",
            id: "abc".into(),
        };
        assert_eq!(classify_error(&err), FailureClass::Permanent);
    }

    #[test]
    fn classify_bad_payload_is_permanent() {
        let err = crate::error::Error::Queue("missing source_id in payload".into());
        assert_eq!(classify_error(&err), FailureClass::Permanent);
    }

    #[test]
    fn classify_rate_limit_402() {
        let err = crate::error::Error::Ingestion(
            "chat backend API returned 402 Payment Required: credits exhausted".into(),
        );
        assert_eq!(classify_error(&err), FailureClass::RateLimit);
    }

    #[test]
    fn classify_rate_limit_429() {
        let err = crate::error::Error::Ingestion(
            "Sorry, you've hit a rate limit that restricts the number of requests".into(),
        );
        assert_eq!(classify_error(&err), FailureClass::RateLimit);
    }

    #[test]
    fn classify_quota_exhausted() {
        let err = crate::error::Error::Ingestion(
            "TerminalQuotaError: You have exhausted your capacity on this model".into(),
        );
        assert_eq!(classify_error(&err), FailureClass::RateLimit);
    }

    #[test]
    fn classify_transient_timeout() {
        let err = crate::error::Error::Ingestion("connection timeout after 30s".into());
        assert_eq!(classify_error(&err), FailureClass::Transient);
    }

    #[test]
    fn classify_database_error_is_transient() {
        // Database errors (connection pool exhaustion, etc.) are transient.
        let err = crate::error::Error::Graph("connection refused".into());
        assert_eq!(classify_error(&err), FailureClass::Transient);
    }

    // --- Backoff-by-class tests ---

    #[test]
    fn rate_limit_backoff_starts_at_15_min() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::RateLimit, 30, 1, 7200),
            900
        );
    }

    #[test]
    fn rate_limit_backoff_doubles() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::RateLimit, 30, 2, 7200),
            1800
        );
    }

    #[test]
    fn rate_limit_backoff_capped() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::RateLimit, 30, 5, 3600),
            3600
        );
    }

    #[test]
    fn permanent_backoff_is_zero() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::Permanent, 30, 1, 3600),
            0
        );
    }

    #[test]
    fn transient_uses_normal_backoff() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::Transient, 30, 1, 3600),
            30
        );
        assert_eq!(
            compute_backoff_for_class(FailureClass::Transient, 30, 3, 3600),
            120
        );
    }
}
