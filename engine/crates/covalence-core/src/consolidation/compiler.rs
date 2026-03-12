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
            "temperature": 0.3
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
            .unwrap_or("{}");

        let parsed: serde_json::Value = serde_json::from_str(content).unwrap_or_default();

        let title = parsed["title"]
            .as_str()
            .unwrap_or(&format!(
                "Community {} — Compiled Summary",
                input.community_id
            ))
            .to_string();

        let body = parsed["body"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| combined.clone());

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
}
