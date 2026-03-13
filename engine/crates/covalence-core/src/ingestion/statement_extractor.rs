//! Statement extraction — atomic, self-contained knowledge claims
//! from source text.
//!
//! Statements are the primary retrieval unit in the statement-first
//! pipeline. Each statement is self-contained (no pronouns or
//! anaphora), carries byte-level source location, and has a heading
//! path for structural context.

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// An atomic statement extracted from source text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedStatement {
    /// Self-contained text of the knowledge claim.
    pub content: String,
    /// Byte offset where the supporting text starts in the source
    /// window.
    pub byte_start: usize,
    /// Byte offset where the supporting text ends in the source
    /// window.
    pub byte_end: usize,
    /// Heading path at the extraction location.
    pub heading_path: Option<String>,
    /// Index of the paragraph within the section.
    pub paragraph_index: Option<i32>,
    /// Extraction confidence in [0.0, 1.0].
    pub confidence: f64,
}

/// Result of statement extraction from a source text window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatementExtractionResult {
    /// Extracted statements.
    pub statements: Vec<ExtractedStatement>,
}

/// Trait for extracting atomic statements from text.
#[async_trait::async_trait]
pub trait StatementExtractor: Send + Sync {
    /// Extract atomic, self-contained statements from the given text
    /// window.
    ///
    /// `window_offset` is the byte offset of this window within the
    /// full source text. Returned byte positions should be relative
    /// to the window start; the caller adjusts them to source-level
    /// positions using `window_offset`.
    async fn extract(
        &self,
        text: &str,
        source_title: Option<&str>,
    ) -> Result<StatementExtractionResult>;
}

/// A mock statement extractor that always returns empty results.
pub struct MockStatementExtractor;

#[async_trait::async_trait]
impl StatementExtractor for MockStatementExtractor {
    async fn extract(
        &self,
        _text: &str,
        _source_title: Option<&str>,
    ) -> Result<StatementExtractionResult> {
        Ok(StatementExtractionResult::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_statement_extractor_returns_empty() {
        let extractor = MockStatementExtractor;
        let result = extractor
            .extract("Some text about knowledge graphs.", None)
            .await
            .unwrap();
        assert!(result.statements.is_empty());
    }

    #[test]
    fn extracted_statement_fields() {
        let stmt = ExtractedStatement {
            content: "Knowledge graphs store entities as nodes.".into(),
            byte_start: 100,
            byte_end: 200,
            heading_path: Some("Chapter 1 > Introduction".into()),
            paragraph_index: Some(2),
            confidence: 0.95,
        };
        assert_eq!(stmt.byte_start, 100);
        assert_eq!(stmt.byte_end, 200);
        assert!((stmt.confidence - 0.95).abs() < 1e-10);
    }

    #[test]
    fn extraction_result_serde_roundtrip() {
        let result = StatementExtractionResult {
            statements: vec![ExtractedStatement {
                content: "Test statement.".into(),
                byte_start: 0,
                byte_end: 15,
                heading_path: None,
                paragraph_index: None,
                confidence: 0.9,
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: StatementExtractionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.statements.len(), 1);
        assert_eq!(restored.statements[0].content, "Test statement.");
    }
}
