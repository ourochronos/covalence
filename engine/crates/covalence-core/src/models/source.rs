//! Source model -- provenance record for ingested material.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::SourceId;

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
        }
    }

    /// Recompute cached reliability score from alpha and beta.
    pub fn recompute_reliability(&mut self) {
        self.reliability_score = self.trust_alpha / (self.trust_alpha + self.trust_beta);
    }
}
