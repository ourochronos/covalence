//! Statement model -- atomic, self-contained knowledge claim
//! extracted from source text.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{SectionId, SourceId, StatementId};

/// An atomic, self-contained knowledge claim extracted from source
/// text. Statements are the primary retrieval unit in the
/// statement-first pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Statement {
    /// Unique identifier.
    pub id: StatementId,
    /// FK to the source this was extracted from.
    pub source_id: SourceId,
    /// Self-contained text of the knowledge claim.
    pub content: String,
    /// SHA-256 of content for dedup across extraction windows.
    pub content_hash: Vec<u8>,
    /// Embedding vector (dimension matches chunk table).
    pub embedding: Option<Vec<f32>>,
    /// Byte offset in `Source.normalized_content` where the
    /// supporting text starts.
    pub byte_start: i32,
    /// Byte offset in `Source.normalized_content` where the
    /// supporting text ends.
    pub byte_end: i32,
    /// Heading path at the extraction location (e.g.
    /// `"Chapter 2 > Methods"`).
    pub heading_path: Option<String>,
    /// Index of the paragraph within the source section.
    pub paragraph_index: Option<i32>,
    /// Position within the source's statement sequence.
    pub ordinal: i32,
    /// Extraction confidence from the LLM.
    pub confidence: f64,
    /// FK to the section this statement belongs to (set after
    /// clustering).
    pub section_id: Option<SectionId>,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// Whether this statement was evicted during re-extraction
    /// (source text no longer supports it).
    pub is_evicted: bool,
    /// Method used for extraction.
    pub extraction_method: String,
    /// When this statement was created.
    pub created_at: DateTime<Utc>,
}

impl Statement {
    /// Create a new statement.
    pub fn new(
        source_id: SourceId,
        content: String,
        content_hash: Vec<u8>,
        byte_start: i32,
        byte_end: i32,
        ordinal: i32,
    ) -> Self {
        Self {
            id: StatementId::new(),
            source_id,
            content,
            content_hash,
            embedding: None,
            byte_start,
            byte_end,
            heading_path: None,
            paragraph_index: None,
            ordinal,
            confidence: 1.0,
            section_id: None,
            clearance_level: ClearanceLevel::default(),
            is_evicted: false,
            extraction_method: "llm_statement".to_string(),
            created_at: Utc::now(),
        }
    }

    /// Set the heading path and return self for chaining.
    pub fn with_heading_path(mut self, path: String) -> Self {
        self.heading_path = Some(path);
        self
    }

    /// Set the section and return self for chaining.
    pub fn with_section(mut self, section_id: SectionId) -> Self {
        self.section_id = Some(section_id);
        self
    }

    /// Set the confidence and return self for chaining.
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_new_defaults() {
        let source_id = SourceId::new();
        let stmt = Statement::new(
            source_id,
            "Graph databases store relationships as first-class citizens.".into(),
            vec![0u8; 32],
            100,
            200,
            0,
        );

        assert_eq!(stmt.source_id, source_id);
        assert_eq!(
            stmt.content,
            "Graph databases store relationships as first-class citizens."
        );
        assert_eq!(stmt.byte_start, 100);
        assert_eq!(stmt.byte_end, 200);
        assert_eq!(stmt.ordinal, 0);
        assert!((stmt.confidence - 1.0).abs() < 1e-10);
        assert!(stmt.section_id.is_none());
        assert!(stmt.heading_path.is_none());
        assert!(stmt.embedding.is_none());
        assert!(!stmt.is_evicted);
        assert_eq!(stmt.extraction_method, "llm_statement");
        assert_eq!(stmt.clearance_level, ClearanceLevel::default());
    }

    #[test]
    fn statement_builder_chain() {
        let section_id = SectionId::new();
        let stmt = Statement::new(
            SourceId::new(),
            "Test statement".into(),
            vec![0u8; 32],
            0,
            50,
            1,
        )
        .with_heading_path("Chapter 1 > Methods".into())
        .with_section(section_id)
        .with_confidence(0.9);

        assert_eq!(stmt.heading_path, Some("Chapter 1 > Methods".to_string()));
        assert_eq!(stmt.section_id, Some(section_id));
        assert!((stmt.confidence - 0.9).abs() < 1e-10);
    }

    #[test]
    fn statement_serde_roundtrip() {
        let stmt = Statement::new(
            SourceId::new(),
            "Knowledge graphs represent entities as nodes.".into(),
            vec![1, 2, 3],
            0,
            45,
            0,
        );
        let json = serde_json::to_string(&stmt).unwrap();
        let restored: Statement = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.content, stmt.content);
        assert_eq!(restored.byte_start, 0);
        assert_eq!(restored.byte_end, 45);
        assert_eq!(restored.extraction_method, "llm_statement");
    }
}
