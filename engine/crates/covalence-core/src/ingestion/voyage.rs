//! Voyage AI embedding client.
//!
//! Implements the `Embedder` trait using the Voyage AI API.
//! Voyage's API is OpenAI-compatible with extensions for
//! contextualized chunk embeddings and `input_type` hints.
//!
//! Default model: `voyage-3-large` (2048 dimensions).
//! Supports Matryoshka truncation to 512d or 1024d.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::ingestion::embedder::Embedder;

/// Voyage AI embedder configuration.
#[derive(Debug, Clone)]
pub struct VoyageConfig {
    /// API key for Voyage AI.
    pub api_key: String,
    /// Base URL (default: `https://api.voyageai.com/v1`).
    pub base_url: String,
    /// Model name (default: `voyage-3-large`).
    pub model: String,
    /// Output dimensionality (default: 2048).
    pub dimensions: usize,
    /// Maximum batch size (Voyage allows up to 128).
    pub batch_size: usize,
    /// Input type hint for the Voyage API.
    ///
    /// Use `"document"` when embedding content for storage (ingestion)
    /// and `"query"` when embedding search queries. This helps the
    /// model produce better representations for asymmetric retrieval.
    /// Defaults to `"document"`.
    pub input_type: String,
}

impl Default for VoyageConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.voyageai.com/v1".to_string(),
            model: "voyage-3-large".to_string(),
            dimensions: 2048,
            batch_size: 128,
            input_type: "document".to_string(),
        }
    }
}

/// A single embedding datum in the Voyage response.
#[derive(Deserialize)]
struct VoyageDatum {
    embedding: Vec<f64>,
}

/// Top-level response from the Voyage embeddings endpoint.
#[derive(Deserialize)]
struct VoyageResponse {
    data: Vec<VoyageDatum>,
}

/// A single result in the Voyage contextual embeddings response.
#[derive(Deserialize)]
struct ContextualResult {
    embeddings: Vec<Vec<f64>>,
}

/// Response from the `/contextualizedembeddings` endpoint.
#[derive(Deserialize)]
struct ContextualResponse {
    results: Vec<ContextualResult>,
}

/// Voyage AI embedding client.
pub struct VoyageEmbedder {
    config: VoyageConfig,
    client: reqwest::Client,
}

impl VoyageEmbedder {
    /// Create a new Voyage embedder.
    pub fn new(config: VoyageConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    /// Whether the configured model supports the contextual
    /// chunk embeddings endpoint.
    fn supports_contextual(&self) -> bool {
        self.config.model.contains("context")
    }
}

#[async_trait::async_trait]
impl Embedder for VoyageEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f64>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        for batch in texts.chunks(self.config.batch_size) {
            let body = serde_json::json!({
                "model": self.config.model,
                "input": batch,
                "input_type": self.config.input_type,
                "output_dimension": self.config.dimensions,
            });

            let resp = self
                .client
                .post(format!("{}/embeddings", self.config.base_url))
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| Error::Embedding(format!("Voyage API request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(Error::Embedding(format!(
                    "Voyage API error {status}: {text}"
                )));
            }

            let parsed: VoyageResponse = resp
                .json()
                .await
                .map_err(|e| Error::Embedding(format!("Failed to parse Voyage response: {e}")))?;

            for datum in parsed.data {
                all_embeddings.push(datum.embedding);
            }
        }

        Ok(all_embeddings)
    }

    /// Embed chunks from a single document using the Voyage
    /// contextual chunk embeddings endpoint.
    ///
    /// When the model supports it (e.g., `voyage-context-3`), all
    /// chunks are sent as a single inner list to the
    /// `/contextualizedembeddings` endpoint. Each chunk's embedding
    /// reflects the surrounding document context (late chunking).
    ///
    /// Falls back to regular per-chunk embeddings for models that
    /// don't support the contextual endpoint.
    async fn embed_document_chunks(&self, chunks: &[String]) -> Result<Vec<Vec<f64>>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        if !self.supports_contextual() {
            return self.embed(chunks).await;
        }

        // Voyage contextual endpoint: inputs is a list of lists.
        // Each inner list is all chunks from one document.
        let mut body = serde_json::json!({
            "model": self.config.model,
            "inputs": [chunks],
            "input_type": self.config.input_type,
        });

        if self.config.dimensions > 0 {
            body["output_dimension"] = self.config.dimensions.into();
        }

        let resp = self
            .client
            .post(format!("{}/contextualizedembeddings", self.config.base_url))
            .bearer_auth(&self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Embedding(format!("Voyage contextual API request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Embedding(format!(
                "Voyage contextual API error {status}: {text}"
            )));
        }

        let parsed: ContextualResponse = resp.json().await.map_err(|e| {
            Error::Embedding(format!("Failed to parse Voyage contextual response: {e}"))
        })?;

        // We sent one inner list, so we expect one result.
        let embeddings = parsed
            .results
            .into_iter()
            .next()
            .map(|r| r.embeddings)
            .unwrap_or_default();

        Ok(embeddings)
    }

    fn dimension(&self) -> usize {
        self.config.dimensions
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voyage_config_defaults() {
        let config = VoyageConfig::default();
        assert_eq!(config.model, "voyage-3-large");
        assert_eq!(config.dimensions, 2048);
        assert_eq!(config.batch_size, 128);
        assert_eq!(config.base_url, "https://api.voyageai.com/v1");
        assert_eq!(config.input_type, "document");
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn voyage_embedder_metadata() {
        let config = VoyageConfig {
            model: "voyage-3-large".to_string(),
            dimensions: 1024,
            ..VoyageConfig::default()
        };
        let embedder = VoyageEmbedder::new(config);
        assert_eq!(embedder.dimension(), 1024);
        assert_eq!(embedder.model_name(), "voyage-3-large");
    }

    #[test]
    fn voyage_config_input_type_override() {
        let config = VoyageConfig {
            input_type: "query".to_string(),
            ..VoyageConfig::default()
        };
        assert_eq!(config.input_type, "query");
        assert_eq!(config.model, "voyage-3-large");
    }

    #[test]
    fn supports_contextual_detection() {
        let default = VoyageEmbedder::new(VoyageConfig::default());
        assert!(!default.supports_contextual());

        let contextual = VoyageEmbedder::new(VoyageConfig {
            model: "voyage-context-3".to_string(),
            ..VoyageConfig::default()
        });
        assert!(contextual.supports_contextual());
    }

    #[tokio::test]
    async fn voyage_embed_empty_input() {
        let embedder = VoyageEmbedder::new(VoyageConfig::default());
        let result = embedder.embed(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn voyage_embed_document_chunks_empty() {
        let embedder = VoyageEmbedder::new(VoyageConfig::default());
        let result = embedder.embed_document_chunks(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn contextual_response_deserialization() {
        let json = serde_json::json!({
            "results": [
                {
                    "embeddings": [
                        [0.1, 0.2, 0.3],
                        [0.4, 0.5, 0.6]
                    ],
                    "texts": ["chunk1", "chunk2"],
                    "index": 0
                }
            ],
            "total_tokens": 10
        });
        let resp: ContextualResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].embeddings.len(), 2);
        assert_eq!(resp.results[0].embeddings[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.results[0].embeddings[1], vec![0.4, 0.5, 0.6]);
    }

    #[test]
    fn voyage_response_deserialization() {
        let json = serde_json::json!({
            "data": [
                {
                    "embedding": [0.1, 0.2, 0.3],
                    "index": 0,
                    "object": "embedding"
                },
                {
                    "embedding": [0.4, 0.5, 0.6],
                    "index": 1,
                    "object": "embedding"
                }
            ],
            "model": "voyage-3-large",
            "usage": {
                "prompt_tokens": 4,
                "total_tokens": 4
            }
        });
        let resp: VoyageResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.data[1].embedding, vec![0.4, 0.5, 0.6]);
    }
}
