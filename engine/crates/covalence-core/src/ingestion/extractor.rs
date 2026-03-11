//! Stage 6: LLM-driven entity and relationship extraction.
//!
//! Extracts entities, relationships, and co-references from chunks
//! via structured LLM output.

use serde::{Deserialize, Serialize};

/// Contextual information about the source being extracted.
///
/// Passed to extractors to help them produce more accurate
/// extractions by understanding what kind of document they're
/// processing.
#[derive(Debug, Clone, Default)]
pub struct ExtractionContext {
    /// Source type (e.g. "web_page", "document", "code").
    pub source_type: Option<String>,
    /// Source URI (e.g. "https://example.com/page" or "file://path").
    pub source_uri: Option<String>,
    /// Source title from metadata.
    pub source_title: Option<String>,
}

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
    ///
    /// The `context` parameter provides source metadata (type, URI,
    /// title) that extractors can use to improve extraction quality.
    /// Extractors that don't use context (e.g., local NER models)
    /// may ignore it.
    async fn extract(
        &self,
        text: &str,
        context: &ExtractionContext,
    ) -> crate::error::Result<ExtractionResult>;
}

/// A mock extractor that always returns empty results.
pub struct MockExtractor;

#[async_trait::async_trait]
impl Extractor for MockExtractor {
    async fn extract(
        &self,
        _text: &str,
        _context: &ExtractionContext,
    ) -> crate::error::Result<ExtractionResult> {
        Ok(ExtractionResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_extractor_returns_empty() {
        let extractor = MockExtractor;
        let ctx = ExtractionContext::default();
        let result = extractor
            .extract("Some text about Alice.", &ctx)
            .await
            .unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn extraction_context_default_is_empty() {
        let ctx = ExtractionContext::default();
        assert!(ctx.source_type.is_none());
        assert!(ctx.source_uri.is_none());
        assert!(ctx.source_title.is_none());
    }

    #[test]
    fn extraction_context_with_fields() {
        let ctx = ExtractionContext {
            source_type: Some("web_page".to_string()),
            source_uri: Some("https://example.com".to_string()),
            source_title: Some("Example Page".to_string()),
        };
        assert_eq!(ctx.source_type.as_deref(), Some("web_page"));
        assert_eq!(ctx.source_uri.as_deref(), Some("https://example.com"));
        assert_eq!(ctx.source_title.as_deref(), Some("Example Page"));
    }
}
