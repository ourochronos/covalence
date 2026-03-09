//! AuditLog model -- tracks system decisions for transparency.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ids::AuditLogId;

/// Standard actions recorded in the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuditAction {
    /// Two nodes merged into one during entity resolution.
    MergeNodes,
    /// A node was split into multiple nodes.
    SplitNode,
    /// An entity was pruned via Bayesian Model Reduction.
    PruneBmr,
    /// Content was taken down (explicit retraction).
    Takedown,
    /// Clearance level was promoted.
    ClearancePromote,
    /// Clearance level was demoted.
    ClearanceDemote,
    /// An article was recompiled during consolidation.
    ArticleRecompile,
    /// Source trust was updated.
    TrustUpdate,
    /// A new source was ingested.
    SourceIngest,
    /// An edge was created.
    EdgeCreate,
    /// Confidence was recalculated for an entity.
    ConfidenceUpdate,
}

impl AuditAction {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MergeNodes => "MERGE_NODES",
            Self::SplitNode => "SPLIT_NODE",
            Self::PruneBmr => "PRUNE_BMR",
            Self::Takedown => "TAKEDOWN",
            Self::ClearancePromote => "CLEARANCE_PROMOTE",
            Self::ClearanceDemote => "CLEARANCE_DEMOTE",
            Self::ArticleRecompile => "ARTICLE_RECOMPILE",
            Self::TrustUpdate => "TRUST_UPDATE",
            Self::SourceIngest => "SOURCE_INGEST",
            Self::EdgeCreate => "EDGE_CREATE",
            Self::ConfidenceUpdate => "CONFIDENCE_UPDATE",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "MERGE_NODES" => Some(Self::MergeNodes),
            "SPLIT_NODE" => Some(Self::SplitNode),
            "PRUNE_BMR" => Some(Self::PruneBmr),
            "TAKEDOWN" => Some(Self::Takedown),
            "CLEARANCE_PROMOTE" => Some(Self::ClearancePromote),
            "CLEARANCE_DEMOTE" => Some(Self::ClearanceDemote),
            "ARTICLE_RECOMPILE" => Some(Self::ArticleRecompile),
            "TRUST_UPDATE" => Some(Self::TrustUpdate),
            "SOURCE_INGEST" => Some(Self::SourceIngest),
            "EDGE_CREATE" => Some(Self::EdgeCreate),
            "CONFIDENCE_UPDATE" => Some(Self::ConfidenceUpdate),
            _ => None,
        }
    }
}

/// A record of a system decision (merge, split, prune, takedown, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    /// Unique identifier.
    pub id: AuditLogId,
    /// The action that was performed.
    pub action: String,
    /// Who or what performed the action (e.g. `"system:deep_consolidation"`).
    pub actor: String,
    /// Type of the target entity (`"node"`, `"edge"`, `"source"`, `"article"`).
    pub target_type: Option<String>,
    /// ID of the target entity.
    pub target_id: Option<uuid::Uuid>,
    /// Decision rationale, before/after state.
    pub payload: serde_json::Value,
    /// When the action was recorded.
    pub created_at: DateTime<Utc>,
}

impl AuditLog {
    /// Create a new audit log entry.
    pub fn new(action: AuditAction, actor: String, payload: serde_json::Value) -> Self {
        Self {
            id: AuditLogId::new(),
            action: action.as_str().to_string(),
            actor,
            target_type: None,
            target_id: None,
            payload,
            created_at: Utc::now(),
        }
    }

    /// Set the target and return self for chaining.
    pub fn with_target(mut self, target_type: &str, target_id: uuid::Uuid) -> Self {
        self.target_type = Some(target_type.to_string());
        self.target_id = Some(target_id);
        self
    }
}
