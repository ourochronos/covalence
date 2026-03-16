//! Persistent retry job model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ids::JobId;

/// A persistent retry job record.
#[derive(Debug, Clone)]
pub struct RetryJob {
    /// Unique job identifier.
    pub id: JobId,
    /// The kind of work this job performs.
    pub kind: JobKind,
    /// Current status of the job.
    pub status: JobStatus,
    /// Arbitrary JSON payload (e.g. `{"source_id": "..."}`)
    pub payload: serde_json::Value,
    /// When the job is next eligible for pickup.
    pub next_due: DateTime<Utc>,
    /// How many times this job has been attempted so far.
    pub attempt: i32,
    /// Maximum attempts before the job is moved to dead.
    pub max_attempts: i32,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job was last updated.
    pub updated_at: DateTime<Utc>,
    /// Error message from the last failed attempt.
    pub last_error: Option<String>,
    /// Reason the job was moved to dead status.
    pub dead_reason: Option<String>,
    /// Optional idempotency key to prevent duplicate enqueues.
    pub idempotency_key: Option<String>,
}

/// The kind of work a job performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Reprocess a source (re-chunk, re-extract, re-embed).
    ReprocessSource,
    /// Extract statements from a source's chunks.
    ExtractStatements,
    /// Extract entities from a source's statements.
    ExtractEntities,
    /// Synthesize co-occurrence edges across the graph.
    SynthesizeEdges,
}

impl JobKind {
    /// Convert to the PostgreSQL enum string representation.
    pub fn as_pg_str(&self) -> &'static str {
        match self {
            Self::ReprocessSource => "reprocess_source",
            Self::ExtractStatements => "extract_statements",
            Self::ExtractEntities => "extract_entities",
            Self::SynthesizeEdges => "synthesize_edges",
        }
    }

    /// Parse from PostgreSQL enum string.
    pub fn from_pg_str(s: &str) -> Option<Self> {
        match s {
            "reprocess_source" => Some(Self::ReprocessSource),
            "extract_statements" => Some(Self::ExtractStatements),
            "extract_entities" => Some(Self::ExtractEntities),
            "synthesize_edges" => Some(Self::SynthesizeEdges),
            _ => None,
        }
    }
}

/// Status of a retry job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting to be picked up by the worker.
    Pending,
    /// Currently executing.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Failed and will be retried (or moved to dead).
    Failed,
    /// Permanently failed — will not be retried.
    Dead,
}

impl JobStatus {
    /// Convert to the PostgreSQL enum string representation.
    pub fn as_pg_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Dead => "dead",
        }
    }

    /// Parse from PostgreSQL enum string.
    pub fn from_pg_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "dead" => Some(Self::Dead),
            _ => None,
        }
    }
}

/// Queue status summary row.
#[derive(Debug, Clone, Serialize)]
pub struct QueueStatusRow {
    /// Job kind.
    pub kind: String,
    /// Job status.
    pub status: String,
    /// Count of jobs with this kind+status.
    pub count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kind_pg_roundtrip() {
        let kinds = [
            JobKind::ReprocessSource,
            JobKind::ExtractStatements,
            JobKind::ExtractEntities,
            JobKind::SynthesizeEdges,
        ];
        for kind in &kinds {
            let s = kind.as_pg_str();
            let back = JobKind::from_pg_str(s);
            assert_eq!(back, Some(*kind), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn job_kind_from_pg_str_unknown() {
        assert_eq!(JobKind::from_pg_str("unknown"), None);
    }

    #[test]
    fn job_status_pg_roundtrip() {
        let statuses = [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Succeeded,
            JobStatus::Failed,
            JobStatus::Dead,
        ];
        for status in &statuses {
            let s = status.as_pg_str();
            let back = JobStatus::from_pg_str(s);
            assert_eq!(back, Some(*status), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn job_status_from_pg_str_unknown() {
        assert_eq!(JobStatus::from_pg_str("unknown"), None);
    }

    #[test]
    fn job_kind_serde_roundtrip() {
        let kind = JobKind::ReprocessSource;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"reprocess_source\"");
        let back: JobKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn job_status_serde_roundtrip() {
        let status = JobStatus::Pending;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"pending\"");
        let back: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn queue_status_row_serializes() {
        let row = QueueStatusRow {
            kind: "reprocess_source".into(),
            status: "pending".into(),
            count: 42,
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["kind"], "reprocess_source");
        assert_eq!(json["status"], "pending");
        assert_eq!(json["count"], 42);
    }
}
