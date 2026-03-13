//! Section model -- a cluster of related statements compiled into a
//! coherent summary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{SectionId, SourceId, StatementId};

/// A cluster of related statements compiled into a summary. Sections
/// provide mid-level retrieval granularity between individual
/// statements and full source summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// Unique identifier.
    pub id: SectionId,
    /// FK to the source these statements came from.
    pub source_id: SourceId,
    /// Section title (LLM-generated from cluster content).
    pub title: String,
    /// Compiled summary of the clustered statements.
    pub summary: String,
    /// SHA-256 of the summary for change detection.
    pub content_hash: Vec<u8>,
    /// Embedding vector (dimension matches chunk table).
    pub embedding: Option<Vec<f32>>,
    /// IDs of the statements in this cluster.
    pub statement_ids: Vec<StatementId>,
    /// Optional cluster label (from HAC or topic model).
    pub cluster_label: Option<String>,
    /// Position within the source's section sequence.
    pub ordinal: i32,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// When this section was created.
    pub created_at: DateTime<Utc>,
    /// When this section was last updated (e.g. recompiled).
    pub updated_at: DateTime<Utc>,
}

impl Section {
    /// Create a new section.
    pub fn new(
        source_id: SourceId,
        title: String,
        summary: String,
        content_hash: Vec<u8>,
        statement_ids: Vec<StatementId>,
        ordinal: i32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: SectionId::new(),
            source_id,
            title,
            summary,
            content_hash,
            embedding: None,
            statement_ids,
            cluster_label: None,
            ordinal,
            clearance_level: ClearanceLevel::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Set the cluster label and return self for chaining.
    pub fn with_cluster_label(mut self, label: String) -> Self {
        self.cluster_label = Some(label);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_new_defaults() {
        let source_id = SourceId::new();
        let stmt_ids = vec![StatementId::new(), StatementId::new()];
        let section = Section::new(
            source_id,
            "Graph Storage Approaches".into(),
            "This section covers property graphs and RDF stores.".into(),
            vec![0u8; 32],
            stmt_ids.clone(),
            0,
        );

        assert_eq!(section.source_id, source_id);
        assert_eq!(section.title, "Graph Storage Approaches");
        assert_eq!(
            section.summary,
            "This section covers property graphs and RDF stores."
        );
        assert_eq!(section.statement_ids.len(), 2);
        assert_eq!(section.ordinal, 0);
        assert!(section.embedding.is_none());
        assert!(section.cluster_label.is_none());
        assert_eq!(section.clearance_level, ClearanceLevel::default());
    }

    #[test]
    fn section_builder_chain() {
        let section = Section::new(
            SourceId::new(),
            "Title".into(),
            "Summary".into(),
            vec![0u8; 32],
            vec![],
            1,
        )
        .with_cluster_label("knowledge_representation".into());

        assert_eq!(
            section.cluster_label,
            Some("knowledge_representation".to_string())
        );
    }

    #[test]
    fn section_serde_roundtrip() {
        let section = Section::new(
            SourceId::new(),
            "Test Section".into(),
            "A compiled summary of statements.".into(),
            vec![1, 2, 3],
            vec![StatementId::new()],
            0,
        );
        let json = serde_json::to_string(&section).unwrap();
        let restored: Section = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.title, "Test Section");
        assert_eq!(restored.summary, "A compiled summary of statements.");
        assert_eq!(restored.statement_ids.len(), 1);
    }
}
