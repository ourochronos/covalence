//! LLM client trait and stub implementation.
//! Real implementations (OpenAI, Anthropic, Ollama) will be added in v1.

use async_trait::async_trait;

/// Trait for language model interactions used by the background worker.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Generate a completion for the given prompt.
    async fn complete(&self, prompt: &str, max_tokens: u32) -> anyhow::Result<String>;

    /// Embed text into a dense vector.
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}

/// Stub implementation that returns placeholder responses without calling any API.
/// Used during development and testing before real LLM integration.
pub struct StubLlmClient;

#[async_trait]
impl LlmClient for StubLlmClient {
    async fn complete(&self, prompt: &str, max_tokens: u32) -> anyhow::Result<String> {
        tracing::debug!(
            prompt_len = prompt.len(),
            max_tokens,
            "StubLlmClient::complete — returning placeholder"
        );
        Ok(format!(
            "[STUB] LLM completion placeholder (max_tokens={}). Integrate OpenAI/Anthropic in v1.",
            max_tokens
        ))
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        // TODO: Replace with real embedding API call (e.g., OpenAI text-embedding-3-small).
        // The real implementation should return a 1536-dim vector for ada-002 or
        // a 1024/3072-dim vector for text-embedding-3-small/large.
        tracing::debug!(
            text_len = text.len(),
            "StubLlmClient::embed — returning zero vector placeholder"
        );
        // Return a 1536-dimensional zero vector as placeholder
        Ok(vec![0.0f32; 1536])
    }
}
