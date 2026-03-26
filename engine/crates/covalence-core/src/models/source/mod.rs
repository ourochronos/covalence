//! Source model -- provenance record for ingested material.

mod source_type;
mod update_class;

pub use source_type::SourceType;
pub use update_class::UpdateClass;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::SourceId;

/// A provenance record for any ingested material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Unique identifier.
    pub id: SourceId,
    /// Type classification of this source.
    pub source_type: String,
    /// Original location (URL, file path, etc.).
    pub uri: Option<String>,
    /// Human-readable title.
    pub title: Option<String>,
    /// Author or creator.
    pub author: Option<String>,
    /// Project namespace (default "covalence"). NULL = global.
    pub project: String,
    /// Knowledge domain: code, spec, design, research, external.
    /// Deprecated: use `domains` for multi-domain classification.
    pub domain: Option<String>,
    /// Multi-domain classification (a source can belong to multiple
    /// visibility scopes). Populated from the `domains TEXT[]` column.
    #[serde(default)]
    pub domains: Vec<String>,
    /// When the source was originally created/published.
    pub created_date: Option<DateTime<Utc>>,
    /// When this system ingested the source.
    pub ingested_at: DateTime<Utc>,
    /// SHA-256 hash of raw content for dedup.
    pub content_hash: Vec<u8>,
    /// Format-specific metadata.
    pub metadata: serde_json::Value,
    /// Optional original text.
    pub raw_content: Option<String>,
    /// Beta distribution alpha (confirmations).
    pub trust_alpha: f64,
    /// Beta distribution beta (contradictions).
    pub trust_beta: f64,
    /// Cached `alpha / (alpha + beta)`.
    pub reliability_score: f64,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// Update class (`append_only`, `versioned`, `correction`, `refactor`, `takedown`).
    pub update_class: Option<String>,
    /// FK to the previous version of this source.
    pub supersedes_id: Option<SourceId>,
    /// Version counter, increments on update.
    pub content_version: i32,
    /// Document-level embedding vector (replaces doc-level chunk).
    pub embedding: Option<Vec<f32>>,
    /// Normalized content text (post-parse, post-normalize).
    /// Chunking operates on this text; chunk byte offsets
    /// reference positions within it.
    pub normalized_content: Option<String>,
    /// SHA-256 hash of `normalized_content` for fast change
    /// detection during re-ingestion.
    pub normalized_hash: Option<Vec<u8>>,
    /// LLM-compiled summary of the source (statement pipeline).
    pub summary: Option<String>,
    /// Processing status: accepted → processing → complete → failed.
    pub status: String,
}

impl Source {
    /// Create a new source with trust priors derived from its type.
    pub fn new(source_type: SourceType, content_hash: Vec<u8>) -> Self {
        let (alpha, beta) = source_type.initial_trust();
        let reliability = alpha / (alpha + beta);
        Self {
            id: SourceId::new(),
            source_type: source_type.as_str().to_string(),
            uri: None,
            title: None,
            author: None,
            project: "covalence".to_string(),
            domain: None,
            domains: Vec::new(),
            created_date: None,
            ingested_at: Utc::now(),
            content_hash,
            metadata: serde_json::Value::Object(Default::default()),
            raw_content: None,
            trust_alpha: alpha,
            trust_beta: beta,
            reliability_score: reliability,
            clearance_level: ClearanceLevel::default(),
            update_class: None,
            supersedes_id: None,
            content_version: 1,
            embedding: None,
            normalized_content: None,
            normalized_hash: None,
            summary: None,
            status: "accepted".to_string(),
        }
    }

    /// Recompute cached reliability score from alpha and beta.
    pub fn recompute_reliability(&mut self) {
        self.reliability_score = self.trust_alpha / (self.trust_alpha + self.trust_beta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_new_has_default_project() {
        let source = Source::new(SourceType::Document, vec![0u8; 32]);
        assert_eq!(source.project, "covalence");
        assert!(source.domain.is_none());
    }

    #[test]
    fn source_new_uses_type_trust() {
        let source = Source::new(SourceType::Document, vec![0u8; 32]);
        assert_eq!(source.trust_alpha, 4.0);
        assert_eq!(source.trust_beta, 1.0);
        assert!((source.reliability_score - 0.80).abs() < 0.01);
        assert_eq!(source.content_version, 1);
        assert_eq!(source.clearance_level, ClearanceLevel::default());
    }

    #[test]
    fn recompute_reliability_updates_cached() {
        let mut source = Source::new(SourceType::Document, vec![0u8; 32]);
        // Manually change alpha/beta
        source.trust_alpha = 2.0;
        source.trust_beta = 3.0;
        source.recompute_reliability();
        assert!((source.reliability_score - 0.4).abs() < 1e-10);
    }

    #[test]
    fn source_serde_roundtrip() {
        let source = Source::new(SourceType::Code, vec![1, 2, 3]);
        let json = serde_json::to_string(&source).unwrap();
        let restored: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.source_type, "code");
        assert_eq!(restored.content_hash, vec![1, 2, 3]);
    }
}
