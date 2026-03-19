//! Section and source summary compilation via LLM.
//!
//! After statements are clustered into groups, each cluster is
//! compiled into a coherent section (title + summary) by an LLM.
//! All section summaries are then compiled into a source-level
//! summary. Uses a [`ChatBackend`] for transport abstraction.

use std::sync::Arc;

use serde::Deserialize;

use crate::error::Result;
use crate::ingestion::chat_backend::ChatBackend;
use crate::ingestion::utils::{sanitize_latex_in_json, strip_markdown_fences};

/// Input for section compilation.
#[derive(Debug, Clone)]
pub struct SectionCompilationInput {
    /// Statement contents in this cluster.
    pub statements: Vec<String>,
    /// Source title for framing.
    pub source_title: Option<String>,
    /// Optional raw source text window around the referenced
    /// statement locations (for verification/context).
    pub context_window: Option<String>,
}

/// Output of section compilation.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct SectionCompilationOutput {
    /// Generated section title.
    pub title: String,
    /// Compiled summary of the clustered statements.
    pub summary: String,
}

/// Trait for compiling a cluster of statements into a section.
#[async_trait::async_trait]
pub trait SectionCompiler: Send + Sync {
    /// Compile a cluster of statements into a section title and
    /// summary.
    async fn compile_section(
        &self,
        input: &SectionCompilationInput,
    ) -> Result<SectionCompilationOutput>;
}

/// Input for source summary compilation.
#[derive(Debug, Clone)]
pub struct SourceSummaryInput {
    /// Section summaries to compile.
    pub section_summaries: Vec<SectionSummaryEntry>,
    /// Source title for framing.
    pub source_title: Option<String>,
}

/// A section summary entry for source-level compilation.
#[derive(Debug, Clone)]
pub struct SectionSummaryEntry {
    /// Section title.
    pub title: String,
    /// Section summary.
    pub summary: String,
}

/// Result of source summary compilation with provider attribution.
#[derive(Debug, Clone)]
pub struct CompilationOutput {
    /// The compiled summary text.
    pub text: String,
    /// Which LLM provider handled the request.
    pub provider: String,
}

/// Trait for compiling section summaries into a source-level summary.
#[async_trait::async_trait]
pub trait SourceSummaryCompiler: Send + Sync {
    /// Compile section summaries into a source-level summary.
    async fn compile_source_summary(
        &self,
        input: &SourceSummaryInput,
    ) -> Result<CompilationOutput>;
}

// ── LLM implementation ──────────────────────────────────────────

/// Section compilation system prompt — loaded from
/// `engine/prompts/section_compilation.md`.
fn section_system_prompt() -> &'static str {
    crate::services::prompts::section_compilation_template()
}

/// Source summary system prompt — loaded from
/// `engine/prompts/source_summary.md`.
fn source_summary_system_prompt() -> &'static str {
    crate::services::prompts::source_summary_template()
}

/// LLM-driven section compiler backed by [`ChatBackend`].
pub struct LlmSectionCompiler {
    backend: Arc<dyn ChatBackend>,
}

impl LlmSectionCompiler {
    /// Create a new LLM section compiler with a chat backend.
    pub fn with_backend(backend: Arc<dyn ChatBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl SectionCompiler for LlmSectionCompiler {
    async fn compile_section(
        &self,
        input: &SectionCompilationInput,
    ) -> Result<SectionCompilationOutput> {
        if input.statements.is_empty() {
            return Ok(SectionCompilationOutput {
                title: "Empty Section".to_string(),
                summary: String::new(),
            });
        }

        let mut user_content = String::new();
        if let Some(ref title) = input.source_title {
            user_content.push_str(&format!("Source: {title}\n\n"));
        }

        user_content.push_str("Statements:\n");
        for (i, stmt) in input.statements.iter().enumerate() {
            user_content.push_str(&format!("{}. {}\n", i + 1, stmt));
        }

        if let Some(ref ctx) = input.context_window {
            // Truncate context to 4000 chars to stay within token
            // limits.
            let truncated: String = ctx.chars().take(4000).collect();
            user_content.push_str(&format!(
                "\nSource context (for verification):\n{truncated}"
            ));
        }

        let chat_resp = self
            .backend
            .chat(section_system_prompt(), &user_content, true, 0.3)
            .await?;
        let content = chat_resp.text;

        // Strip markdown code fences if the LLM wrapped the JSON.
        let cleaned = strip_markdown_fences(&content);
        // Sanitize LaTeX escapes that break JSON parsing.
        let cleaned = sanitize_latex_in_json(&cleaned);

        let raw: RawSectionOutput = match serde_json::from_str(&cleaned) {
            Ok(r) => r,
            Err(e) => {
                let preview: String = content.chars().take(500).collect();
                tracing::warn!(
                    error = %e,
                    raw_output = %preview,
                    "section compilation JSON parse failed — using fallback"
                );
                // Fallback: use first statement as title, concat all
                // as summary.
                return Ok(SectionCompilationOutput {
                    title: input
                        .statements
                        .first()
                        .map(|s| s.chars().take(60).collect::<String>())
                        .unwrap_or_default(),
                    summary: input.statements.join(" "),
                });
            }
        };

        Ok(SectionCompilationOutput {
            title: raw.title.trim().to_string(),
            summary: raw.summary.trim().to_string(),
        })
    }
}

#[async_trait::async_trait]
impl SourceSummaryCompiler for LlmSectionCompiler {
    async fn compile_source_summary(
        &self,
        input: &SourceSummaryInput,
    ) -> Result<CompilationOutput> {
        if input.section_summaries.is_empty() {
            return Ok(CompilationOutput {
                text: String::new(),
                provider: "none".to_string(),
            });
        }

        let mut user_content = String::new();
        if let Some(ref title) = input.source_title {
            user_content.push_str(&format!("Source: {title}\n\n"));
        }

        user_content.push_str("Section summaries:\n\n");
        for entry in &input.section_summaries {
            user_content.push_str(&format!("## {}\n{}\n\n", entry.title, entry.summary));
        }

        let chat_resp = self
            .backend
            .chat(source_summary_system_prompt(), &user_content, false, 0.3)
            .await?;

        Ok(CompilationOutput {
            text: chat_resp.text.trim().to_string(),
            provider: chat_resp.provider,
        })
    }
}

/// Mock section compiler for testing.
pub struct MockSectionCompiler;

#[async_trait::async_trait]
impl SectionCompiler for MockSectionCompiler {
    async fn compile_section(
        &self,
        input: &SectionCompilationInput,
    ) -> Result<SectionCompilationOutput> {
        let title = format!("Section ({})", input.statements.len());
        let summary = input.statements.join(". ");
        Ok(SectionCompilationOutput { title, summary })
    }
}

#[async_trait::async_trait]
impl SourceSummaryCompiler for MockSectionCompiler {
    async fn compile_source_summary(
        &self,
        input: &SourceSummaryInput,
    ) -> Result<CompilationOutput> {
        Ok(CompilationOutput {
            text: input
                .section_summaries
                .iter()
                .map(|s| s.summary.clone())
                .collect::<Vec<_>>()
                .join(" "),
            provider: "mock".to_string(),
        })
    }
}

// ── Response deserialization ────────────────────────────────────

#[derive(Deserialize)]
struct RawSectionOutput {
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_section_output_deserialization() {
        let json = r#"{
            "title": "Graph Storage Approaches",
            "summary": "Property graphs store relationships as first-class entities."
        }"#;
        let result: RawSectionOutput = serde_json::from_str(json).unwrap();
        assert_eq!(result.title, "Graph Storage Approaches");
        assert!(!result.summary.is_empty());
    }

    #[test]
    fn raw_section_output_empty_json() {
        let json = "{}";
        let result: RawSectionOutput = serde_json::from_str(json).unwrap();
        assert!(result.title.is_empty());
        assert!(result.summary.is_empty());
    }

    #[tokio::test]
    async fn mock_section_compiler() {
        let compiler = MockSectionCompiler;
        let input = SectionCompilationInput {
            statements: vec!["Claim A".to_string(), "Claim B".to_string()],
            source_title: Some("Test Source".to_string()),
            context_window: None,
        };
        let output = compiler.compile_section(&input).await.unwrap();
        assert_eq!(output.title, "Section (2)");
        assert_eq!(output.summary, "Claim A. Claim B");
    }

    #[tokio::test]
    async fn mock_source_summary_compiler() {
        let compiler = MockSectionCompiler;
        let input = SourceSummaryInput {
            section_summaries: vec![
                SectionSummaryEntry {
                    title: "Section 1".to_string(),
                    summary: "Summary A.".to_string(),
                },
                SectionSummaryEntry {
                    title: "Section 2".to_string(),
                    summary: "Summary B.".to_string(),
                },
            ],
            source_title: Some("Test".to_string()),
        };
        let result = compiler.compile_source_summary(&input).await.unwrap();
        assert_eq!(result.text, "Summary A. Summary B.");
        assert_eq!(result.provider, "mock");
    }

    #[tokio::test]
    async fn mock_section_compiler_empty() {
        let compiler = MockSectionCompiler;
        let input = SectionCompilationInput {
            statements: vec![],
            source_title: None,
            context_window: None,
        };
        let output = compiler.compile_section(&input).await.unwrap();
        assert_eq!(output.title, "Section (0)");
        assert!(output.summary.is_empty());
    }
}
