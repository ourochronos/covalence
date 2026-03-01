//! OpenAI API client for embeddings and completions.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::llm::LlmClient;

/// OpenAI-compatible LLM client.
/// Works with OpenAI API and any compatible endpoint (e.g., LiteLLM, vLLM).
pub struct OpenAiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    embed_model: String,
    chat_model: String,
}

impl OpenAiClient {
    pub fn new(api_key: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            base_url: "https://api.openai.com/v1".into(),
            embed_model: "text-embedding-3-small".into(),
            chat_model: "gpt-4.1-mini".into(),
        }
    }

    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    pub fn with_embed_model(mut self, model: String) -> Self {
        self.embed_model = model;
        self
    }

    pub fn with_chat_model(mut self, model: String) -> Self {
        self.chat_model = model;
        self
    }
}

// ─── Request/Response types ───────────────────────────────────────────────────

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedDatum>,
}

#[derive(Deserialize)]
struct EmbedDatum {
    embedding: Vec<f32>,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessageOut,
}

#[derive(Deserialize)]
struct ChatMessageOut {
    content: Option<String>,
}

// ─── LlmClient impl ──────────────────────────────────────────────────────────

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(&self, prompt: &str, max_tokens: u32) -> anyhow::Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatRequest {
            model: self.chat_model.clone(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: prompt.to_string(),
            }],
            max_tokens,
            temperature: 0.3,
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI chat API error {status}: {text}");
        }

        let parsed: ChatResponse = resp.json().await?;
        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| anyhow::anyhow!("OpenAI returned empty response"))
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let url = format!("{}/embeddings", self.base_url);

        // No truncation — callers (tree_index pipeline) are responsible for
        // sizing content to fit model context windows. text-embedding-3-small
        // supports 8191 tokens (~28K chars). Large content is handled by
        // embed_long_section() in tree_index.rs which uses sliding windows.
        let body = EmbedRequest {
            model: self.embed_model.clone(),
            input: text.to_string(),
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI embed API error {status}: {text}");
        }

        let parsed: EmbedResponse = resp.json().await?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| anyhow::anyhow!("OpenAI returned no embedding data"))
    }
}
