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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_article_defaults() {
        let node_ids = vec![NodeId::new(), NodeId::new()];
        let article = Article::new(
            "Test Article".into(),
            "Body text".into(),
            vec![0u8; 32],
            node_ids.clone(),
        );

        assert_eq!(article.title, "Test Article");
        assert_eq!(article.body, "Body text");
        assert_eq!(article.confidence, 1.0);
        assert!(article.confidence_breakdown.is_none());
        assert!(article.domain_path.is_empty());
        assert_eq!(article.version, 1);
        assert_eq!(article.source_node_ids.len(), 2);
        assert_eq!(article.clearance_level, ClearanceLevel::default());
        assert_eq!(article.created_at, article.updated_at);
    }

    #[test]
    fn with_domain_path() {
        let article = Article::new("T".into(), "B".into(), vec![], vec![])
            .with_domain_path(vec!["AI".into(), "Knowledge Graphs".into()]);

        assert_eq!(
            article.domain_path,
            vec!["AI".to_string(), "Knowledge Graphs".to_string()]
        );
    }

    #[test]
    fn serde_roundtrip() {
        let article = Article::new("Title".into(), "Body".into(), vec![1, 2], vec![])
            .with_domain_path(vec!["Science".into()]);
        let json = serde_json::to_string(&article).unwrap();
        let restored: Article = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.title, "Title");
        assert_eq!(restored.domain_path, vec!["Science"]);
    }
}
