//! Source model -- provenance record for ingested material.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::SourceId;

/// Knowledge domain classification for sources.
///
/// Classifies a source's role in the knowledge graph. Set at ingestion
/// time via `derive_domain()` based on URI patterns and source type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDomain {
    /// Source code files (.rs, .go, etc.).
    Code,
    /// Specification documents (spec/*.md).
    Spec,
    /// Architecture decisions, design docs, project meta.
    Design,
    /// Academic papers, external knowledge.
    Research,
    /// Third-party documentation, API references.
    External,
}

impl SourceDomain {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Spec => "spec",
            Self::Design => "design",
            Self::Research => "research",
            Self::External => "external",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "code" => Some(Self::Code),
            "spec" => Some(Self::Spec),
            "design" => Some(Self::Design),
            "research" => Some(Self::Research),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

/// Type classification for ingested sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    /// Published document (paper, report, book).
    Document,
    /// Web page or blog post.
    WebPage,
    /// Chat transcript or conversation log.
    Conversation,
    /// Source code or repository.
    Code,
    /// API response or structured data feed.
    Api,
    /// Manually entered knowledge.
    Manual,
    /// Tool output (CLI, scripts, automated systems).
    ToolOutput,
    /// Direct observation or measurement.
    Observation,
}

impl SourceType {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::WebPage => "web_page",
            Self::Conversation => "conversation",
            Self::Code => "code",
            Self::Api => "api",
            Self::Manual => "manual",
            Self::ToolOutput => "tool_output",
            Self::Observation => "observation",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "document" => Some(Self::Document),
            "web_page" => Some(Self::WebPage),
            "conversation" => Some(Self::Conversation),
            "code" => Some(Self::Code),
            "api" => Some(Self::Api),
            "manual" => Some(Self::Manual),
            "tool_output" => Some(Self::ToolOutput),
            "observation" => Some(Self::Observation),
            _ => None,
        }
    }

    /// Initial trust prior as `(alpha, beta)` from the spec.
    ///
    /// Based on the Beta-binomial trust model in `spec/07-epistemic-model.md`.
    pub fn initial_trust(&self) -> (f64, f64) {
        match self {
            Self::Document => (4.0, 1.0),     // 0.80 prior
            Self::ToolOutput => (3.5, 1.5),   // 0.70 prior
            Self::Code => (3.5, 1.5),         // 0.70 prior (same as tool)
            Self::Api => (3.5, 1.5),          // 0.70 prior
            Self::WebPage => (3.0, 2.0),      // 0.60 prior
            Self::Manual => (3.0, 2.0),       // 0.60 prior
            Self::Conversation => (2.5, 2.5), // 0.50 prior
            Self::Observation => (2.0, 3.0),  // 0.40 prior
        }
    }
}

/// Update class for source lifecycle management.
///
/// Determines how updates to a source propagate through the
/// graph. See `spec/05-ingestion.md` for full semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateClass {
    /// Content is only ever appended (e.g., log files).
    AppendOnly,
    /// Content is versioned; new versions supersede old ones.
    Versioned,
    /// A correction to a previous source.
    Correction,
    /// Structural refactor without semantic change.
    Refactor,
    /// Source has been taken down; all derived edges should be
    /// invalidated.
    Takedown,
}

impl UpdateClass {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AppendOnly => "append_only",
            Self::Versioned => "versioned",
            Self::Correction => "correction",
            Self::Refactor => "refactor",
            Self::Takedown => "takedown",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "append_only" => Some(Self::AppendOnly),
            "versioned" => Some(Self::Versioned),
            "correction" => Some(Self::Correction),
            "refactor" => Some(Self::Refactor),
            "takedown" => Some(Self::Takedown),
            _ => None,
        }
    }

    /// Whether this update class requires cascading edge
    /// invalidation.
    pub fn requires_invalidation(&self) -> bool {
        matches!(self, Self::Correction | Self::Takedown)
    }
}

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
    pub domain: Option<String>,
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
    fn source_domain_roundtrip() {
        let domains = [
            SourceDomain::Code,
            SourceDomain::Spec,
            SourceDomain::Design,
            SourceDomain::Research,
            SourceDomain::External,
        ];
        for d in &domains {
            let s = d.as_str();
            let parsed = SourceDomain::from_str_opt(s);
            assert_eq!(parsed, Some(d.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn source_domain_from_str_unknown() {
        assert!(SourceDomain::from_str_opt("unknown").is_none());
        assert!(SourceDomain::from_str_opt("").is_none());
    }

    #[test]
    fn source_domain_serde_roundtrip() {
        let d = SourceDomain::Research;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"research\"");
        let parsed: SourceDomain = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceDomain::Research);
    }

    #[test]
    fn source_new_has_default_project() {
        let source = Source::new(SourceType::Document, vec![0u8; 32]);
        assert_eq!(source.project, "covalence");
        assert!(source.domain.is_none());
    }

    #[test]
    fn source_type_roundtrip() {
        let types = [
            SourceType::Document,
            SourceType::WebPage,
            SourceType::Conversation,
            SourceType::Code,
            SourceType::Api,
            SourceType::Manual,
            SourceType::ToolOutput,
            SourceType::Observation,
        ];
        for st in &types {
            let s = st.as_str();
            let parsed = SourceType::from_str_opt(s);
            assert_eq!(parsed, Some(st.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn source_type_from_str_unknown() {
        assert!(SourceType::from_str_opt("unknown").is_none());
        assert!(SourceType::from_str_opt("").is_none());
        assert!(SourceType::from_str_opt("Document").is_none()); // case-sensitive
    }

    #[test]
    fn source_type_serde_roundtrip() {
        let st = SourceType::WebPage;
        let json = serde_json::to_string(&st).unwrap();
        assert_eq!(json, "\"web_page\"");
        let parsed: SourceType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceType::WebPage);
    }

    #[test]
    fn initial_trust_priors() {
        // Document should have highest trust
        let (a, b) = SourceType::Document.initial_trust();
        assert!((a / (a + b) - 0.80).abs() < 0.01);

        // Conversation should be lower
        let (a, b) = SourceType::Conversation.initial_trust();
        assert!((a / (a + b) - 0.50).abs() < 0.01);

        // Observation should be lowest
        let (a, b) = SourceType::Observation.initial_trust();
        assert!((a / (a + b) - 0.40).abs() < 0.01);
    }

    #[test]
    fn update_class_roundtrip() {
        let classes = [
            UpdateClass::AppendOnly,
            UpdateClass::Versioned,
            UpdateClass::Correction,
            UpdateClass::Refactor,
            UpdateClass::Takedown,
        ];
        for uc in &classes {
            let s = uc.as_str();
            let parsed = UpdateClass::from_str_opt(s);
            assert_eq!(parsed, Some(uc.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn update_class_from_str_unknown() {
        assert!(UpdateClass::from_str_opt("unknown").is_none());
    }

    #[test]
    fn requires_invalidation() {
        assert!(!UpdateClass::AppendOnly.requires_invalidation());
        assert!(!UpdateClass::Versioned.requires_invalidation());
        assert!(UpdateClass::Correction.requires_invalidation());
        assert!(!UpdateClass::Refactor.requires_invalidation());
        assert!(UpdateClass::Takedown.requires_invalidation());
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
