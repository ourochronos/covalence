//! LLM-driven atomic statement extractor.
//!
//! Uses a [`ChatBackend`] to extract self-contained knowledge claims
//! from source text windows.

use std::sync::Arc;

use serde::Deserialize;

use crate::error::Result;
use crate::ingestion::chat_backend::ChatBackend;
use crate::ingestion::statement_extractor::{
    ExtractedStatement, StatementExtractionResult, StatementExtractor,
};
use crate::ingestion::utils::{sanitize_latex_in_json, strip_markdown_fences};

const SYSTEM_PROMPT: &str = r#"You are a knowledge statement extractor. Given a passage of text, extract every atomic, self-contained knowledge claim as a separate statement.

Return a JSON object with this exact schema:
{
  "statements": [
    {
      "content": "A complete, self-contained sentence expressing one knowledge claim.",
      "byte_start": 0,
      "byte_end": 100,
      "heading_path": "Section > Subsection or null",
      "paragraph_index": 0,
      "confidence": 0.0-1.0
    }
  ]
}

Rules:
- Each statement must be SELF-CONTAINED: a reader should understand it without any other context. Replace all pronouns, anaphora, and abbreviations with their full referents. For example, "It uses gradient descent" should become "The BERT model uses gradient descent for optimization."
- Each statement should express exactly ONE atomic knowledge claim. Split compound sentences.
- byte_start and byte_end should indicate the approximate character positions in the input text that support this statement. They do not need to be exact but should locate the relevant passage.
- heading_path should capture any visible section/subsection headings near the statement. Use " > " as separator. Set to null if no headings are visible.
- paragraph_index should be the 0-based index of the paragraph containing this claim within the visible text.
- confidence reflects how clearly the text supports the claim (1.0 = explicitly stated, 0.5 = implied).
- Do NOT extract from bibliographic references, citations, author blocks, or boilerplate.
- Do NOT extract hypothetical examples or placeholder scenarios.
- Do NOT extract metadata (page numbers, headers/footers, table of contents entries).
- Preserve technical precision: keep specific numbers, names, and terminology exactly as they appear.
- Return valid JSON only, no markdown fences or extra text."#;

/// An LLM-driven statement extractor backed by [`ChatBackend`].
pub struct LlmStatementExtractor {
    backend: Arc<dyn ChatBackend>,
}

impl LlmStatementExtractor {
    /// Create a new LLM statement extractor with a chat backend.
    pub fn with_backend(backend: Arc<dyn ChatBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl StatementExtractor for LlmStatementExtractor {
    async fn extract(
        &self,
        text: &str,
        source_title: Option<&str>,
    ) -> Result<StatementExtractionResult> {
        if text.trim().is_empty() {
            return Ok(StatementExtractionResult::default());
        }

        let user_content = if let Some(title) = source_title {
            format!("Source: {title}\n\n{text}")
        } else {
            text.to_string()
        };

        let content = self
            .backend
            .chat(SYSTEM_PROMPT, &user_content, true, 0.0)
            .await?;

        // Strip markdown code fences if the LLM wrapped the JSON.
        let cleaned = strip_markdown_fences(&content);
        // Sanitize LaTeX escapes that break JSON parsing.
        let cleaned = sanitize_latex_in_json(&cleaned);

        let raw: RawStatementResult = match serde_json::from_str(&cleaned) {
            Ok(r) => r,
            Err(e) => {
                let preview: String = content.chars().take(500).collect();
                tracing::warn!(
                    error = %e,
                    raw_output = %preview,
                    "statement extraction JSON parse failed — returning empty"
                );
                return Ok(StatementExtractionResult::default());
            }
        };

        let statements: Vec<ExtractedStatement> = raw
            .statements
            .into_iter()
            .filter(|s| {
                let content_trimmed = s.content.trim();
                if content_trimmed.is_empty() {
                    tracing::debug!("dropping empty statement");
                    return false;
                }
                if content_trimmed.len() < 10 {
                    tracing::debug!(content = content_trimmed, "dropping very short statement");
                    return false;
                }
                true
            })
            .map(|s| ExtractedStatement {
                content: s.content.trim().to_string(),
                byte_start: s.byte_start,
                byte_end: s.byte_end.max(s.byte_start),
                heading_path: s.heading_path,
                paragraph_index: s.paragraph_index,
                confidence: s.confidence.clamp(0.0, 1.0),
            })
            .collect();

        tracing::debug!(count = statements.len(), "statement extraction complete");

        Ok(StatementExtractionResult { statements })
    }
}

// ── Response deserialization ────────────────────────────────────

#[derive(Deserialize)]
struct RawStatementResult {
    #[serde(default)]
    statements: Vec<RawStatement>,
}

#[derive(Deserialize)]
struct RawStatement {
    #[serde(default)]
    content: String,
    #[serde(default)]
    byte_start: usize,
    #[serde(default)]
    byte_end: usize,
    heading_path: Option<String>,
    paragraph_index: Option<i32>,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

fn default_confidence() -> f64 {
    0.9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_statement_deserialization() {
        let json = r#"{
            "statements": [
                {
                    "content": "The BERT model uses masked language modeling for pre-training.",
                    "byte_start": 0,
                    "byte_end": 60,
                    "heading_path": "Pre-training > Objectives",
                    "paragraph_index": 1,
                    "confidence": 0.95
                },
                {
                    "content": "Transformer architectures use self-attention mechanisms.",
                    "byte_start": 61,
                    "byte_end": 120,
                    "heading_path": null,
                    "paragraph_index": null,
                    "confidence": 0.9
                }
            ]
        }"#;

        let result: RawStatementResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.statements.len(), 2);
        assert_eq!(result.statements[0].byte_start, 0);
        assert_eq!(result.statements[0].byte_end, 60);
        assert!(result.statements[0].heading_path.is_some());
        assert!(result.statements[1].heading_path.is_none());
    }

    #[test]
    fn raw_statement_empty_json() {
        let json = "{}";
        let result: RawStatementResult = serde_json::from_str(json).unwrap();
        assert!(result.statements.is_empty());
    }

    #[test]
    fn raw_statement_missing_fields_uses_defaults() {
        let json = r#"{
            "statements": [{"content": "Test claim."}]
        }"#;

        let result: RawStatementResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.statements.len(), 1);
        assert_eq!(result.statements[0].byte_start, 0);
        assert_eq!(result.statements[0].byte_end, 0);
        assert!((result.statements[0].confidence - 0.9).abs() < 1e-10);
    }
}
