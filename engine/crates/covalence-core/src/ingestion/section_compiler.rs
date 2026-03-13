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

/// Trait for compiling section summaries into a source-level summary.
#[async_trait::async_trait]
pub trait SourceSummaryCompiler: Send + Sync {
    /// Compile section summaries into a source-level summary.
    async fn compile_source_summary(&self, input: &SourceSummaryInput) -> Result<String>;
}

// ── LLM implementation ──────────────────────────────────────────

const SECTION_SYSTEM_PROMPT: &str = r#"You are a knowledge synthesis assistant. Given a set of atomic knowledge claims (statements) that belong to a single topic cluster, produce a coherent section with a title and summary.

Return a JSON object with this exact schema:
{
  "title": "A concise, descriptive title for this section (3-8 words)",
  "summary": "A well-written paragraph that synthesizes all the statements into a coherent narrative. Preserve technical precision. Do not add information not present in the statements."
}

Rules:
- The title should be specific and descriptive, not generic (e.g., "Gradient Descent Optimization" not "Methods").
- The summary should be 2-6 sentences that flow naturally.
- Preserve all specific numbers, names, and terminology from the statements.
- Do NOT add information beyond what the statements contain.
- Do NOT include meta-commentary about the statements themselves.
- Return valid JSON only, no markdown fences or extra text."#;

const SOURCE_SUMMARY_SYSTEM_PROMPT: &str = r#"You are a knowledge synthesis assistant. Given a set of section summaries from a single source document, produce a concise overall summary of the entire source.

Rules:
- Write 2-4 sentences that capture the key themes and contributions.
- Preserve technical precision and specific terminology.
- Do NOT list sections — synthesize across them.
- Do NOT add information beyond what the sections contain.
- Return the summary as plain text (not JSON)."#;

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

        let content = self
            .backend
            .chat(SECTION_SYSTEM_PROMPT, &user_content, true, 0.3)
            .await?;

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
    async fn compile_source_summary(&self, input: &SourceSummaryInput) -> Result<String> {
        if input.section_summaries.is_empty() {
            return Ok(String::new());
        }

        let mut user_content = String::new();
        if let Some(ref title) = input.source_title {
            user_content.push_str(&format!("Source: {title}\n\n"));
        }

        user_content.push_str("Section summaries:\n\n");
        for entry in &input.section_summaries {
            user_content.push_str(&format!("## {}\n{}\n\n", entry.title, entry.summary));
        }

        let content = self
            .backend
            .chat(SOURCE_SUMMARY_SYSTEM_PROMPT, &user_content, false, 0.3)
            .await?;

        Ok(content.trim().to_string())
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
    async fn compile_source_summary(&self, input: &SourceSummaryInput) -> Result<String> {
        Ok(input
            .section_summaries
            .iter()
            .map(|s| s.summary.clone())
            .collect::<Vec<_>>()
            .join(" "))
    }
}

// ── Helpers ─────────────────────────────────────────────────────

/// Strip markdown code fences (```json ... ```) from LLM output.
fn strip_markdown_fences(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let after_tag = if let Some(pos) = rest.find('\n') {
            &rest[pos + 1..]
        } else {
            rest
        };
        let body = if let Some(stripped) = after_tag.strip_suffix("```") {
            stripped
        } else {
            after_tag
        };
        body.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Escape invalid LaTeX backslash sequences inside JSON string
/// values. LLMs sometimes emit raw LaTeX (`\omega`, `\ddot{u}`)
/// inside JSON strings, which are not valid JSON escapes.
///
/// Strategy: replace any `\X` where X is not a valid JSON escape
/// char (`"`, `\`, `/`, `b`, `f`, `n`, `r`, `t`, `u`) with `\\X`.
fn sanitize_latex_in_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 32);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if matches!(next, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') {
                    // Valid JSON escape — pass through.
                    out.push('\\');
                } else {
                    // Invalid escape — double the backslash.
                    out.push('\\');
                    out.push('\\');
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
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
        assert_eq!(result, "Summary A. Summary B.");
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

    #[test]
    fn sanitize_latex_escapes_basic() {
        let input = r#"{"title": "\omega_{x|y} values"}"#;
        let sanitized = sanitize_latex_in_json(input);
        // \o and \_ should be double-escaped; the rest stays
        let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
        assert!(parsed["title"].as_str().unwrap().contains("omega"));
    }

    #[test]
    fn sanitize_latex_escapes_preserves_valid() {
        let input = r#"{"title": "line1\nline2", "tab": "\t"}"#;
        let sanitized = sanitize_latex_in_json(input);
        assert_eq!(sanitized, input, "valid escapes must be unchanged");
    }

    #[test]
    fn sanitize_latex_ddot_and_overline() {
        let input = r#"{"summary": "The value \ddot{u} for \overline{y}"}"#;
        let sanitized = sanitize_latex_in_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
        let summary = parsed["summary"].as_str().unwrap();
        assert!(summary.contains("ddot"));
        assert!(summary.contains("overline"));
    }

    #[test]
    fn sanitize_latex_trailing_backslash() {
        let input = r#"{"x": "end\"#;
        let sanitized = sanitize_latex_in_json(input);
        // Trailing backslash stays single — JSON will still fail
        // but we don't mangle the rest.
        assert!(sanitized.ends_with('\\'));
    }
}
