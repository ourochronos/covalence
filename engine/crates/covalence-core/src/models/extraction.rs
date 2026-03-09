//! Extraction model -- provenance link from chunk to graph entity.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ids::{ChunkId, ExtractionId};

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

/// Links a graph element back to the chunk it was extracted from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extraction {
    /// Unique identifier.
    pub id: ExtractionId,
    /// FK to the chunk this was extracted from.
    pub chunk_id: ChunkId,
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
    /// Create a new extraction record.
    pub fn new(
        chunk_id: ChunkId,
        entity_type: ExtractedEntityType,
        entity_id: uuid::Uuid,
        extraction_method: String,
        confidence: f64,
    ) -> Self {
        Self {
            id: ExtractionId::new(),
            chunk_id,
            entity_type: entity_type.as_str().to_string(),
            entity_id,
            extraction_method,
            confidence,
            is_superseded: false,
            extracted_at: Utc::now(),
        }
    }
}
