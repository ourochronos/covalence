//! Stage 6: LLM-driven entity and relationship extraction.
//!
//! Extracts entities, relationships, and co-references from chunks
//! via structured LLM output.

use serde::{Deserialize, Serialize};

/// An entity extracted from text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    /// Name of the entity as it appears in the text.
    pub name: String,
    /// Type classification (e.g. "person", "organization").
    pub entity_type: String,
    /// Optional description of the entity.
    pub description: Option<String>,
    /// Extraction confidence score in [0.0, 1.0].
    pub confidence: f64,
}

/// A relationship extracted between two entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedRelationship {
    /// Name of the source entity.
    pub source_name: String,
    /// Name of the target entity.
    pub target_name: String,
    /// Relationship type (e.g. "works_at", "is_part_of").
    pub rel_type: String,
    /// Optional description of the relationship.
    pub description: Option<String>,
    /// Extraction confidence score in [0.0, 1.0].
    pub confidence: f64,
}

/// Combined extraction result from a text chunk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractionResult {
    /// Extracted entities.
    pub entities: Vec<ExtractedEntity>,
    /// Extracted relationships.
    pub relationships: Vec<ExtractedRelationship>,
}

/// Trait for extracting entities and relationships from text.
#[async_trait::async_trait]
pub trait Extractor: Send + Sync {
    /// Extract entities and relationships from the given text.
    async fn extract(&self, text: &str) -> crate::error::Result<ExtractionResult>;
}

/// A mock extractor that always returns empty results.
pub struct MockExtractor;

#[async_trait::async_trait]
impl Extractor for MockExtractor {
    async fn extract(&self, _text: &str) -> crate::error::Result<ExtractionResult> {
        Ok(ExtractionResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_extractor_returns_empty() {
        let extractor = MockExtractor;
        let result = extractor.extract("Some text about Alice.").await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }
}
