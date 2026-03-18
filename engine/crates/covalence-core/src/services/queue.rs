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

    /// Try to claim and dispatch one job. Returns `true` if work was
    /// found (even if execution failed).
    async fn poll_once(&self) -> bool {
        // Try reprocess-family work first (includes async pipeline jobs).
        let reprocess_kinds = [
            JobKind::ReprocessSource,
            JobKind::ExtractStatements,
            JobKind::ExtractEntities,
            JobKind::ExtractChunk,
            JobKind::SummarizeEntity,
            JobKind::ComposeSourceSummary,
            JobKind::EmbedBatch,
        ];

        if let Ok(permit) = self.reprocess_sem.clone().try_acquire_owned() {
            match JobQueueRepo::claim_next(&*self.repo, &reprocess_kinds).await {
                Ok(Some(job)) => {
                    self.spawn_job(job, permit);
                    return true;
                }
                Ok(None) => {
                    // No reprocess work. Drop permit and try edge work.
                    drop(permit);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to claim reprocess job");
                    drop(permit);
                }
            }
        }

        // Try edge synthesis work.
        if let Ok(permit) = self.edge_sem.clone().try_acquire_owned() {
            match JobQueueRepo::claim_next(&*self.repo, &[JobKind::SynthesizeEdges]).await {
                Ok(Some(job)) => {
                    self.spawn_job(job, permit);
                    return true;
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

        false
    }

    /// Spawn a tokio task that executes the job and holds the
    /// semaphore permit until completion.
    fn spawn_job(&self, job: RetryJob, permit: tokio::sync::OwnedSemaphorePermit) {
        let repo = Arc::clone(&self.repo);
        let source_service = self.source_service.get().cloned();
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

            let result =
                match tokio::time::timeout(job_timeout, execute_job(&job, source_service.as_ref()))
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

/// Execute a single job based on its kind and payload.
async fn execute_job(job: &RetryJob, source_service: Option<&Arc<SourceService>>) -> Result<()> {
    match job.kind {
        JobKind::ReprocessSource => {
            let source_id_str = job
                .payload
                .get("source_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::error::Error::Queue("missing source_id in payload".to_string())
                })?;
            let uuid = uuid::Uuid::parse_str(source_id_str)
                .map_err(|e| crate::error::Error::Queue(format!("invalid source_id UUID: {e}")))?;
            let source_id = SourceId::from_uuid(uuid);

            let svc = source_service.ok_or_else(|| {
                crate::error::Error::Queue("source_service not available for reprocess".to_string())
            })?;

            svc.reprocess(source_id).await?;
            Ok(())
        }
        JobKind::SynthesizeEdges => {
            // TODO: wire up edge synthesis service call
            tracing::info!(
                min_cooccurrences = job
                    .payload
                    .get("min_cooccurrences")
                    .and_then(|v| v.as_i64()),
                "edge synthesis job — not yet implemented"
            );
            Ok(())
        }
        JobKind::ExtractStatements | JobKind::ExtractEntities => {
            // These kinds are claimed but not yet separately
            // implemented — reprocess handles the full pipeline.
            tracing::warn!(
                kind = ?job.kind,
                "job kind not yet independently implemented, marking as succeeded"
            );
            Ok(())
        }
        JobKind::ExtractChunk => {
            // TODO: extract entities from a single chunk
            tracing::info!(
                chunk_id = job.payload.get("chunk_id").and_then(|v| v.as_str()),
                "extract_chunk job — stub"
            );
            Ok(())
        }
        JobKind::SummarizeEntity => {
            let node_id_str = job
                .payload
                .get("node_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::error::Error::Queue("missing node_id in payload".to_string())
                })?;
            let source_id_str = job
                .payload
                .get("source_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::error::Error::Queue("missing source_id in payload".to_string())
                })?;
            let node_uuid = uuid::Uuid::parse_str(node_id_str)
                .map_err(|e| crate::error::Error::Queue(format!("invalid node_id: {e}")))?;
            let source_uuid = uuid::Uuid::parse_str(source_id_str)
                .map_err(|e| crate::error::Error::Queue(format!("invalid source_id: {e}")))?;
            let node_id = crate::types::ids::NodeId::from_uuid(node_uuid);
            let source_id = SourceId::from_uuid(source_uuid);

            let svc = source_service.ok_or_else(|| {
                crate::error::Error::Queue(
                    "source_service not available for summarize_entity".to_string(),
                )
            })?;

            summarize_single_entity(svc, node_id, source_id).await
        }
        JobKind::ComposeSourceSummary => {
            // TODO: compose file-level summary from entity summaries
            tracing::info!(
                source_id = job.payload.get("source_id").and_then(|v| v.as_str()),
                "compose_source_summary job — stub"
            );
            Ok(())
        }
        JobKind::EmbedBatch => {
            // TODO: embed a batch of items
            tracing::info!(
                batch_size = job
                    .payload
                    .get("item_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len()),
                "embed_batch job — stub"
            );
            Ok(())
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

    let prompt = format!(
        "You are a code documentation engine. \
         Summarize the `{name}` {ntype} from the code below.\n\n\
         Output a concise (50-150 word) natural language summary of \
         its purpose, inputs, outputs, and key behavior. Focus on \
         WHAT it does and WHY, not HOW. Write as if explaining to \
         someone who understands the domain but hasn't read the code.\n\n\
         IMPORTANT: Only describe `{name}`. Ignore other code in the \
         same block.\n\n\
         File: {file}\n\n```\n{code}\n```",
        name = node.canonical_name,
        ntype = node.node_type,
        file = file_path,
        code = &raw[..raw.len().min(3000)],
    );

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
        "prompt_version": 2,
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
