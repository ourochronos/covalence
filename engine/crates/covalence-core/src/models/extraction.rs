//! Extraction model -- provenance link from chunk to graph entity.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ids::{ChunkId, ExtractionId, StatementId};

/// The type of entity an extraction links to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractedEntityType {
    /// Extraction produced a graph node.
    Node,
    /// Extraction produced a graph edge.
    Edge,
}

impl ExtractedEntityType {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Edge => "edge",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "node" => Some(Self::Node),
            "edge" => Some(Self::Edge),
            _ => None,
        }
    }
}

/// Links a graph element back to the chunk or statement it was
/// extracted from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    /// Unique identifier.
    pub id: ExtractionId,
    /// FK to the chunk this was extracted from (None for
    /// statement-based extractions).
    pub chunk_id: Option<ChunkId>,
    /// FK to the statement this was extracted from (None for
    /// chunk-based extractions).
    pub statement_id: Option<StatementId>,
    /// Whether this extraction produced a node or edge.
    pub entity_type: String,
    /// ID of the extracted node or edge.
    pub entity_id: uuid::Uuid,
    /// Method used for extraction (e.g. `"llm_gpt4"`, `"ner_spacy"`).
    pub extraction_method: String,
    /// Confidence of the extraction.
    pub confidence: f64,
    /// Whether this extraction has been superseded by a newer one.
    pub is_superseded: bool,
    /// When the extraction was performed.
    pub extracted_at: DateTime<Utc>,
}

impl Extraction {
    /// Create a new extraction record from a chunk.
    pub fn new(
        chunk_id: ChunkId,
        entity_type: ExtractedEntityType,
        entity_id: uuid::Uuid,
        extraction_method: String,
        confidence: f64,
    ) -> Self {
        Self {
            id: ExtractionId::new(),
            chunk_id: Some(chunk_id),
            statement_id: None,
            entity_type: entity_type.as_str().to_string(),
            entity_id,
            extraction_method,
            confidence,
            is_superseded: false,
            extracted_at: Utc::now(),
        }
    }

    /// Create a new extraction record from a statement.
    pub fn from_statement(
        statement_id: StatementId,
        entity_type: ExtractedEntityType,
        entity_id: uuid::Uuid,
        extraction_method: String,
        confidence: f64,
    ) -> Self {
        Self {
            id: ExtractionId::new(),
            chunk_id: None,
            statement_id: Some(statement_id),
            entity_type: entity_type.as_str().to_string(),
            entity_id,
            extraction_method,
            confidence,
            is_superseded: false,
            extracted_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_type_roundtrip() {
        assert_eq!(
            ExtractedEntityType::from_str_opt("node"),
            Some(ExtractedEntityType::Node)
        );
        assert_eq!(
            ExtractedEntityType::from_str_opt("edge"),
            Some(ExtractedEntityType::Edge)
        );
        assert_eq!(ExtractedEntityType::from_str_opt("other"), None);
    }

    #[test]
    fn entity_type_as_str() {
        assert_eq!(ExtractedEntityType::Node.as_str(), "node");
        assert_eq!(ExtractedEntityType::Edge.as_str(), "edge");
    }

    #[test]
    fn extraction_new_defaults() {
        let chunk_id = ChunkId::new();
        let entity_id = uuid::Uuid::new_v4();
        let ext = Extraction::new(
            chunk_id,
            ExtractedEntityType::Node,
            entity_id,
            "llm_gpt4".into(),
            0.95,
        );

        assert_eq!(ext.chunk_id, Some(chunk_id));
        assert!(ext.statement_id.is_none());
        assert_eq!(ext.entity_type, "node");
        assert_eq!(ext.entity_id, entity_id);
        assert_eq!(ext.extraction_method, "llm_gpt4");
        assert!((ext.confidence - 0.95).abs() < 1e-10);
        assert!(!ext.is_superseded);
    }

    #[test]
    fn extraction_from_statement_defaults() {
        let statement_id = StatementId::new();
        let entity_id = uuid::Uuid::new_v4();
        let ext = Extraction::from_statement(
            statement_id,
            ExtractedEntityType::Node,
            entity_id,
            "llm_statement".into(),
            0.9,
        );

        assert!(ext.chunk_id.is_none());
        assert_eq!(ext.statement_id, Some(statement_id));
        assert_eq!(ext.entity_type, "node");
        assert_eq!(ext.extraction_method, "llm_statement");
    }

    #[test]
    fn extraction_serde_roundtrip() {
        let ext = Extraction::new(
            ChunkId::new(),
            ExtractedEntityType::Edge,
            uuid::Uuid::new_v4(),
            "ner_spacy".into(),
            0.8,
        );
        let json = serde_json::to_string(&ext).unwrap();
        let restored: Extraction = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.entity_type, "edge");
        assert_eq!(restored.extraction_method, "ner_spacy");
    }
}
