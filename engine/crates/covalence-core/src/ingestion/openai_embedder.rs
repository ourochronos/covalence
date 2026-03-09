//! OpenAI-compatible embedding client.
//!
//! Posts text batches to the `/embeddings` endpoint and parses the
//! response into `Vec<Vec<f64>>` vectors.

use serde::{Deserialize, Serialize};

use crate::config::EmbeddingConfig;
use crate::error::{Error, Result};
use crate::ingestion::embedder::Embedder;

/// An embedder that calls an OpenAI-compatible `/embeddings` endpoint.
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dimensions: usize,
    batch_size: usize,
}

impl OpenAiEmbedder {
    /// Create a new embedder from configuration.
    ///
    /// `base_url` defaults to `https://api.openai.com/v1` when `None`.
    pub fn new(config: &EmbeddingConfig, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        Self {
            client: reqwest::Client::new(),
            base_url: base.trim_end_matches('/').to_string(),
            api_key,
            model: config.model.clone(),
            dimensions: config.dimensions,
            batch_size: config.batch_size,
        }
    }
}

/// Request body sent to the embeddings endpoint.
#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

/// A single embedding datum in the response.
#[derive(Deserialize)]
struct EmbedDatum {
    embedding: Vec<f64>,
}

/// Top-level response from the embeddings endpoint.
#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedDatum>,
}

#[async_trait::async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f64>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        for batch in texts.chunks(self.batch_size) {
            let body = EmbedRequest {
                model: &self.model,
                input: batch,
            };

            let resp = self
                .client
                .post(format!("{}/embeddings", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| Error::Embedding(e.to_string()))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(Error::Embedding(format!("API returned {status}: {text}")));
            }

            let parsed: EmbedResponse = resp
                .json()
                .await
                .map_err(|e| Error::Embedding(e.to_string()))?;

            for datum in parsed.data {
                all_embeddings.push(datum.embedding);
            }
        }

        Ok(all_embeddings)
    }

    fn dimension(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_request_serialization() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        let req = EmbedRequest {
            model: "text-embedding-3-small",
            input: &texts,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "text-embedding-3-small");
        assert_eq!(json["input"], serde_json::json!(["hello", "world"]));
    }

    #[test]
    fn embed_response_deserialization() {
        let json = serde_json::json!({
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0, "object": "embedding"},
                {"embedding": [0.4, 0.5, 0.6], "index": 1, "object": "embedding"}
            ],
            "model": "text-embedding-3-small",
            "usage": {"prompt_tokens": 4, "total_tokens": 4}
        });
        let resp: EmbedResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.data[1].embedding, vec![0.4, 0.5, 0.6]);
    }

    #[test]
    fn batch_splitting_logic() {
        // Verify that chunks() correctly splits
        let texts: Vec<String> = (0..7).map(|i| format!("text_{i}")).collect();
        let batch_size = 3;
        let batches: Vec<&[String]> = texts.chunks(batch_size).collect();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 3);
        assert_eq!(batches[1].len(), 3);
        assert_eq!(batches[2].len(), 1);
    }

    #[test]
    fn constructor_defaults() {
        let config = EmbeddingConfig::default();
        let embedder = OpenAiEmbedder::new(&config, "sk-test".to_string(), None);
        assert_eq!(embedder.base_url, "https://api.openai.com/v1");
        assert_eq!(embedder.model, "voyage-context-3");
        assert_eq!(embedder.dimensions, 2048);
        assert_eq!(embedder.batch_size, 64);
    }

    #[test]
    fn constructor_custom_base_url() {
        let config = EmbeddingConfig::default();
        let embedder = OpenAiEmbedder::new(
            &config,
            "sk-test".to_string(),
            Some("http://localhost:8080/v1/".to_string()),
        );
        assert_eq!(embedder.base_url, "http://localhost:8080/v1");
    }

    #[tokio::test]
    async fn embed_empty_input() {
        let config = EmbeddingConfig::default();
        let embedder = OpenAiEmbedder::new(&config, "sk-test".to_string(), None);
        let result = embedder.embed(&[]).await.unwrap();
        assert!(result.is_empty());
    }
}
