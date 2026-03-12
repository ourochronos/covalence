//! Article compiler — synthesizes articles from chunk content via LLM.
//!
//! Provides a trait for article compilation and an LLM-backed
//! implementation that sends source content to an OpenAI-compatible
//! API for summarization.

use crate::error::Result;

/// Input to the article compilation step.
#[derive(Debug, Clone)]
pub struct CompilationInput {
    /// Community/topic identifier.
    pub community_id: usize,
    /// Chunk texts belonging to this cluster.
    pub chunks: Vec<String>,
    /// Entity names relevant to this cluster.
    pub entity_names: Vec<String>,
}

/// Output from article compilation.
#[derive(Debug, Clone)]
pub struct CompilationOutput {
    /// Generated article title.
    pub title: String,
    /// Generated Markdown body.
    pub body: String,
    /// Short summary (used for embeddings and search snippets).
    pub summary: String,
}

/// Trait for compiling articles from clustered chunks.
#[async_trait::async_trait]
pub trait ArticleCompiler: Send + Sync {
    /// Compile a single article from the given input.
    async fn compile(&self, input: &CompilationInput) -> Result<CompilationOutput>;
}

/// A simple compiler that concatenates chunks without LLM.
///
/// Used as a fallback when no LLM is configured.
pub struct ConcatCompiler;

#[async_trait::async_trait]
impl ArticleCompiler for ConcatCompiler {
    async fn compile(&self, input: &CompilationInput) -> Result<CompilationOutput> {
        let title = format!("Community {} — Compiled Summary", input.community_id);

        let mut body = String::new();
        for (i, chunk) in input.chunks.iter().enumerate() {
            if i > 0 {
                body.push_str("\n\n---\n\n");
            }
            body.push_str(chunk);
        }

        // Build a simple summary from the first ~200 characters.
        // Use char boundary to avoid panicking on multi-byte UTF-8.
        let summary = if body.len() > 200 {
            let end = body
                .char_indices()
                .take_while(|&(i, _)| i < 200)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(body.len());
            format!("{}...", &body[..end])
        } else {
            body.clone()
        };

        Ok(CompilationOutput {
            title,
            body,
            summary,
        })
    }
}

/// An LLM-backed article compiler using an OpenAI-compatible API.
pub struct LlmCompiler {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmCompiler {
    /// Create a new LLM compiler.
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
        }
    }
}

#[async_trait::async_trait]
impl ArticleCompiler for LlmCompiler {
    async fn compile(&self, input: &CompilationInput) -> Result<CompilationOutput> {
        let combined = input.chunks.join("\n\n---\n\n");
        let entity_list = if input.entity_names.is_empty() {
            String::new()
        } else {
            format!("\n\nKey entities: {}", input.entity_names.join(", "))
        };

        let system_prompt = "You are a knowledge synthesis assistant. \
            Given source material, produce a well-structured Markdown \
            article that summarizes the key information. Include a \
            clear title, organized sections, and preserve important \
            facts and relationships. Output valid JSON with fields: \
            title (string), body (string, Markdown), \
            summary (string, 1-2 sentences).";

        let user_prompt = format!(
            "Synthesize the following source material into a coherent \
            article:{entity_list}\n\n## Source Material\n\n{combined}"
        );

        let request_body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt}
            ],
            "response_format": {"type": "json_object"},
            "temperature": 0.3,
            "max_tokens": 16384
        });

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                crate::error::Error::Consolidation(format!("LLM API request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body: String = resp
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(crate::error::Error::Consolidation(format!(
                "LLM API error {status}: {body}"
            )));
        }

        let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
            crate::error::Error::Consolidation(format!("failed to parse LLM response: {e}"))
        })?;

        // Extract the content from the chat completion response
        let content = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                crate::error::Error::Consolidation(
                    "LLM response missing choices[0].message.content".to_string(),
                )
            })?;

        // Strip markdown code fences if the model wrapped the JSON.
        let content = content
            .trim()
            .strip_prefix("```json")
            .or_else(|| content.trim().strip_prefix("```"))
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(content)
            .trim();

        let parsed: serde_json::Value =
            serde_json::from_str(content).or_else(|e| {
                // Attempt repair: if the JSON is truncated, try closing
                // open strings/braces. This handles the common case where
                // the model hits max_tokens mid-body.
                tracing::warn!(
                    community_id = input.community_id,
                    "LLM returned invalid JSON ({e}), attempting repair"
                );
                repair_truncated_json(content).ok_or_else(|| {
                    crate::error::Error::Consolidation(format!(
                        "LLM returned invalid JSON: {e}"
                    ))
                })
            })?;

        let title = parsed["title"]
            .as_str()
            .unwrap_or(&format!(
                "Community {} — Compiled Summary",
                input.community_id
            ))
            .to_string();

        // Body is required — don't silently fall back to raw
        // concatenation, which produces junk articles.
        let body = parsed["body"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                crate::error::Error::Consolidation(
                    "LLM response JSON missing 'body' field".to_string(),
                )
            })?;

        let summary = parsed["summary"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if body.len() > 200 {
                    let end = body
                        .char_indices()
                        .take_while(|&(i, _)| i < 200)
                        .last()
                        .map(|(i, c)| i + c.len_utf8())
                        .unwrap_or(body.len());
                    format!("{}...", &body[..end])
                } else {
                    body.clone()
                }
            });

        Ok(CompilationOutput {
            title,
            body,
            summary,
        })
    }
}

/// Attempt to repair truncated JSON from an LLM response.
///
/// When the model hits `max_tokens`, the JSON is often cut mid-string.
/// This function tries to close open strings and braces to make it
/// parseable. Returns `None` if repair fails.
fn repair_truncated_json(raw: &str) -> Option<serde_json::Value> {
    let mut s = raw.to_string();

    // If we're inside an open string, close it.
    let quote_count = s.chars().filter(|&c| c == '"').count()
        - s.chars()
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|w| w[0] == '\\' && w[1] == '"')
            .count();
    if quote_count % 2 != 0 {
        s.push('"');
    }

    // Count open braces/brackets and close them.
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut in_string = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && in_string {
            i += 2; // skip escaped char
            continue;
        }
        match chars[i] {
            '"' => in_string = !in_string,
            '{' if !in_string => brace_depth += 1,
            '}' if !in_string => brace_depth -= 1,
            '[' if !in_string => bracket_depth += 1,
            ']' if !in_string => bracket_depth -= 1,
            _ => {}
        }
        i += 1;
    }

    for _ in 0..bracket_depth {
        s.push(']');
    }
    for _ in 0..brace_depth {
        s.push('}');
    }

    serde_json::from_str(&s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concat_compiler_single_chunk() {
        let compiler = ConcatCompiler;
        let input = CompilationInput {
            community_id: 0,
            chunks: vec!["Hello world".to_string()],
            entity_names: vec![],
        };
        let output = compiler.compile(&input).await.unwrap();
        assert!(output.title.contains("Community 0"));
        assert_eq!(output.body, "Hello world");
    }

    #[tokio::test]
    async fn concat_compiler_multiple_chunks() {
        let compiler = ConcatCompiler;
        let input = CompilationInput {
            community_id: 42,
            chunks: vec!["First part".to_string(), "Second part".to_string()],
            entity_names: vec!["Alice".to_string()],
        };
        let output = compiler.compile(&input).await.unwrap();
        assert!(output.body.contains("First part"));
        assert!(output.body.contains("Second part"));
        assert!(output.body.contains("---"));
    }

    #[tokio::test]
    async fn concat_compiler_multibyte_summary_truncation() {
        let compiler = ConcatCompiler;
        // Build body with multi-byte chars that would panic on naive
        // &body[..200] slicing.
        let emoji_body = "🎉".repeat(100); // 4 bytes each = 400 bytes
        let input = CompilationInput {
            community_id: 0,
            chunks: vec![emoji_body],
            entity_names: vec![],
        };
        let output = compiler.compile(&input).await.unwrap();
        // Should truncate without panicking and end with "..."
        assert!(output.summary.ends_with("..."));
        // Summary must be valid UTF-8 (implicit — String type)
        assert!(output.summary.len() < output.body.len());
    }

    #[tokio::test]
    async fn concat_compiler_empty_input() {
        let compiler = ConcatCompiler;
        let input = CompilationInput {
            community_id: 1,
            chunks: vec![],
            entity_names: vec![],
        };
        let output = compiler.compile(&input).await.unwrap();
        assert!(output.body.is_empty());
    }

    #[test]
    fn repair_valid_json_unchanged() {
        let json = r#"{"title": "Test", "body": "Hello", "summary": "Hi"}"#;
        let parsed = repair_truncated_json(json).unwrap();
        assert_eq!(parsed["title"], "Test");
    }

    #[test]
    fn repair_truncated_string() {
        // Truncated mid-string in body field
        let json = r#"{"title": "Test", "body": "Hello wor"#;
        let parsed = repair_truncated_json(json).unwrap();
        assert_eq!(parsed["title"], "Test");
        assert!(parsed["body"].as_str().unwrap().starts_with("Hello wor"));
    }

    #[test]
    fn repair_truncated_missing_closing_brace() {
        let json = r#"{"title": "Test", "body": "Hello""#;
        let parsed = repair_truncated_json(json).unwrap();
        assert_eq!(parsed["title"], "Test");
        assert_eq!(parsed["body"], "Hello");
    }

    #[test]
    fn repair_with_markdown_fences() {
        // Some models wrap JSON in ```json ... ```
        let json = "```json\n{\"title\": \"Test\"}\n```";
        // This is handled by the caller (strip_prefix), not repair.
        // But repair should handle valid JSON fine.
        let parsed = repair_truncated_json(
            &json
                .trim()
                .strip_prefix("```json")
                .unwrap()
                .strip_suffix("```")
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(parsed["title"], "Test");
    }

    #[test]
    fn repair_with_escaped_quotes() {
        let json = r#"{"title": "A \"quoted\" title", "body": "text"}"#;
        let parsed = repair_truncated_json(json).unwrap();
        assert_eq!(parsed["title"], "A \"quoted\" title");
    }

    #[test]
    fn repair_deeply_truncated_returns_none() {
        // So truncated that closing strings/braces still can't
        // produce valid JSON (no key-value pair).
        let json = r#"{"ti"#;
        let result = repair_truncated_json(json);
        assert!(result.is_none());
    }

    #[test]
    fn repair_nested_brackets() {
        let json = r#"{"title": "Test", "items": ["a", "b"#;
        let parsed = repair_truncated_json(json).unwrap();
        assert_eq!(parsed["title"], "Test");
    }
}
