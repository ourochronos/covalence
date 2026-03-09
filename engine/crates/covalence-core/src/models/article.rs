//! Article model -- compiled summary produced during batch consolidation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{ArticleId, NodeId};
use crate::types::opinion::Opinion;

/// A synthesized summary produced during batch consolidation.
///
/// Articles are optimal retrieval units (200-4000 tokens).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    /// Unique identifier.
    pub id: ArticleId,
    /// Article title.
    pub title: String,
    /// Compiled Markdown body.
    pub body: String,
    /// Aggregate Bayesian confidence.
    pub confidence: f64,
    /// Subjective Logic opinion tuple.
    pub confidence_breakdown: Option<Opinion>,
    /// Topic hierarchy path (e.g. `["AI", "Knowledge Graphs"]`).
    pub domain_path: Vec<String>,
    /// Version number, increments on recompilation.
    pub version: i32,
    /// SHA-256 of compiled content.
    pub content_hash: Vec<u8>,
    /// Node IDs this article was compiled from.
    pub source_node_ids: Vec<NodeId>,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// When the article was first created.
    pub created_at: DateTime<Utc>,
    /// When the article was last recompiled.
    pub updated_at: DateTime<Utc>,
}

impl Article {
    /// Create a new article.
    pub fn new(
        title: String,
        body: String,
        content_hash: Vec<u8>,
        source_node_ids: Vec<NodeId>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: ArticleId::new(),
            title,
            body,
            confidence: 1.0,
            confidence_breakdown: None,
            domain_path: Vec::new(),
            version: 1,
            content_hash,
            source_node_ids,
            clearance_level: ClearanceLevel::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Set the domain path and return self for chaining.
    pub fn with_domain_path(mut self, path: Vec<String>) -> Self {
        self.domain_path = path;
        self
    }
}
