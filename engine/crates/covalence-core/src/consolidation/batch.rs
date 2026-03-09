//! Batch consolidation tier.
//!
//! Groups sources by topic cluster, compiles articles via LLM,
//! applies Bayesian confidence aggregation, detects contentions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::ids::SourceId;

/// Status of a batch consolidation job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    /// Job is queued but not yet started.
    Pending,
    /// Job is currently executing.
    Running,
    /// Job finished successfully.
    Complete,
    /// Job failed with an error.
    Failed,
}

/// A batch consolidation job tracking a set of sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchJob {
    /// Unique job identifier.
    pub id: Uuid,
    /// Source IDs to consolidate in this batch.
    pub source_ids: Vec<SourceId>,
    /// Current job status.
    pub status: BatchStatus,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job completed (if finished).
    pub completed_at: Option<DateTime<Utc>>,
}

/// Trait for running batch consolidation on a set of sources.
#[async_trait::async_trait]
pub trait BatchConsolidator: Send + Sync {
    /// Execute a batch consolidation job.
    async fn run_batch(&self, job: &mut BatchJob) -> crate::error::Result<()>;
}
