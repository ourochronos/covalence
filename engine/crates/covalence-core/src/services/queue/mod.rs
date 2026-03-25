//! Persistent retry queue service with background worker.
//!
//! Jobs are persisted in PostgreSQL and processed by a polling
//! worker loop with per-kind concurrency limits and exponential
//! backoff on failure.

pub mod classification;
mod fan_in;
mod handlers;
mod worker;

pub use classification::{
    FailureClass, classify_error, compute_backoff, compute_backoff_for_class,
};

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
    pub(crate) repo: Arc<PgRepo>,
    /// Queue configuration (poll interval, backoff, concurrency).
    pub(crate) config: RetryQueueConfig,
    /// Per-kind concurrency semaphores.
    pub(crate) sem_process: Arc<Semaphore>,
    pub(crate) sem_extract: Arc<Semaphore>,
    pub(crate) sem_summarize: Arc<Semaphore>,
    pub(crate) sem_compose: Arc<Semaphore>,
    pub(crate) sem_edge: Arc<Semaphore>,
    pub(crate) sem_embed: Arc<Semaphore>,
    /// Lazily-set source service for executing reprocess jobs.
    pub(crate) source_service: tokio::sync::OnceCell<Arc<SourceService>>,
    /// Lazily-set admin service for edge synthesis and other admin jobs.
    pub(crate) admin_service: tokio::sync::OnceCell<Arc<AdminService>>,
}

impl RetryQueueService {
    /// Create a new retry queue service with per-kind concurrency.
    pub fn new(repo: Arc<PgRepo>, config: RetryQueueConfig) -> Self {
        Self {
            sem_process: Arc::new(Semaphore::new(config.reprocess_concurrency)),
            sem_extract: Arc::new(Semaphore::new(config.extract_concurrency)),
            sem_summarize: Arc::new(Semaphore::new(config.summarize_concurrency)),
            sem_compose: Arc::new(Semaphore::new(config.compose_concurrency)),
            sem_edge: Arc::new(Semaphore::new(config.edge_concurrency)),
            sem_embed: Arc::new(Semaphore::new(config.embed_concurrency)),
            repo,
            config,
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

    /// Get the configured code entity class from the source service's
    /// pipeline config, falling back to `"code"` if the service is
    /// not yet wired.
    pub(crate) fn code_entity_class(&self) -> String {
        self.source_service
            .get()
            .map(|svc| svc.pipeline.code_entity_class.clone())
            .unwrap_or_else(|| "code".to_string())
    }

    /// Get the configured code domain from the source service's
    /// pipeline config, falling back to `"code"` if the service is
    /// not yet wired.
    pub(crate) fn code_domain(&self) -> String {
        self.source_service
            .get()
            .map(|svc| svc.pipeline.code_domain.clone())
            .unwrap_or_else(|| "code".to_string())
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
        use crate::storage::traits::QueueRepo;
        let chunks: Vec<(uuid::Uuid,)> =
            QueueRepo::get_chunks_by_source(&*self.repo, source_id).await?;

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
        let code_class = self.code_entity_class();

        // Find code entities that need summaries.
        use crate::storage::traits::QueueRepo;
        let rows: Vec<(uuid::Uuid, uuid::Uuid)> =
            QueueRepo::list_unsummarized_entities_for_summary(&*self.repo, &code_class).await?;

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
        let code_domain = self.code_domain();
        let code_class = self.code_entity_class();

        use crate::storage::traits::QueueRepo;
        let rows: Vec<(uuid::Uuid,)> =
            QueueRepo::list_sources_needing_compose(&*self.repo, &code_domain, &code_class).await?;

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

    /// Admin: resurrect dead jobs — reset to pending with attempt=0.
    ///
    /// Returns the number of jobs resurrected.
    pub async fn resurrect_dead(&self, kind: Option<JobKind>) -> Result<u64> {
        JobQueueRepo::resurrect_dead(&*self.repo, kind).await
    }
}
