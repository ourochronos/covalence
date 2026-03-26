//! Background worker loop, job scheduling, and watchdog.
//!
//! Contains the main polling loop that claims pending jobs, the
//! per-job spawn/dispatch logic, orphan recovery, the pipeline
//! watchdog, and the periodic scheduler.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::models::retry_job::{JobKind, RetryJob};
use crate::storage::traits::JobQueueRepo;

use super::RetryQueueService;
use super::classification::{FailureClass, classify_error, compute_backoff_for_class};
use super::handlers::execute_job;

impl RetryQueueService {
    /// Spawn the pipeline watchdog as a background task.
    ///
    /// Periodically checks for sources that may have stalled mid-pipeline
    /// (all child jobs completed but fan-in didn't trigger) and re-runs
    /// the fan-in check.
    pub fn spawn_watchdog(self: &Arc<Self>) {
        let repo = Arc::clone(&self.repo);
        let interval = std::time::Duration::from_secs(120); // every 2 minutes
        let code_domain = self.code_domain();
        let code_class = self.code_entity_class();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                // Find code sources with entity summaries but no file summary
                // and no pending compose job — these are stalled.
                use crate::storage::traits::QueueRepo;
                let stalled: Vec<(uuid::Uuid,)> =
                    QueueRepo::list_stalled_sources(&*repo, &code_domain, &code_class)
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
                        let _ = QueueRepo::insert_retry_job_direct(
                            &*repo,
                            "compose_source_summary",
                            &payload,
                            &key,
                            5,
                        )
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

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Recovery pass: reset orphaned `running` jobs back to `pending`.
    pub(super) async fn recover_orphaned_jobs(&self) -> crate::error::Result<()> {
        use crate::storage::traits::QueueRepo;
        let recovered = QueueRepo::recover_orphaned_jobs(&*self.repo).await?;
        if recovered > 0 {
            tracing::info!(recovered, "recovered orphaned running jobs");
        }
        Ok(())
    }

    /// Get the semaphore for a given job kind.
    fn sem_for_kind(&self, kind: JobKind) -> &Arc<Semaphore> {
        match kind {
            JobKind::ProcessSource | JobKind::ReprocessSource => &self.sem_process,
            JobKind::ExtractChunk => &self.sem_extract,
            JobKind::SummarizeEntity => &self.sem_summarize,
            JobKind::ComposeSourceSummary => &self.sem_compose,
            JobKind::SynthesizeEdges => &self.sem_edge,
            JobKind::EmbedBatch => &self.sem_embed,
            JobKind::ExtractStatements | JobKind::ExtractEntities => &self.sem_process,
        }
    }

    /// Try to claim and dispatch jobs up to available per-kind concurrency.
    /// Returns `true` if any work was found.
    pub(super) async fn poll_once(&self) -> bool {
        // Per-kind job groups with their semaphores.
        let job_groups: &[(JobKind, &[JobKind])] = &[
            (JobKind::ExtractChunk, &[JobKind::ExtractChunk]),
            (
                JobKind::ProcessSource,
                &[
                    JobKind::ProcessSource,
                    JobKind::ReprocessSource,
                    JobKind::ExtractStatements,
                    JobKind::ExtractEntities,
                ],
            ),
            (JobKind::SummarizeEntity, &[JobKind::SummarizeEntity]),
            (
                JobKind::ComposeSourceSummary,
                &[JobKind::ComposeSourceSummary],
            ),
            (JobKind::EmbedBatch, &[JobKind::EmbedBatch]),
            (JobKind::SynthesizeEdges, &[JobKind::SynthesizeEdges]),
        ];

        let mut did_work = false;

        for (sem_kind, claim_kinds) in job_groups {
            let sem = self.sem_for_kind(*sem_kind);
            loop {
                let permit = match sem.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => break,
                };
                match JobQueueRepo::claim_next(&*self.repo, claim_kinds).await {
                    Ok(Some(job)) => {
                        self.spawn_job(job, permit);
                        did_work = true;
                    }
                    Ok(None) => {
                        drop(permit);
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to claim job");
                        drop(permit);
                        break;
                    }
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
            let job_start = std::time::Instant::now();
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

            let kind_str = format!("{job_kind:?}");
            let elapsed = job_start.elapsed().as_secs_f64();

            match result {
                Ok(()) => {
                    crate::metrics::record_queue_job(&kind_str, "success");
                    crate::metrics::record_queue_job_duration(&kind_str, elapsed);
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
                    crate::metrics::record_queue_job(&kind_str, "failure");
                    crate::metrics::record_queue_job_duration(&kind_str, elapsed);
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
