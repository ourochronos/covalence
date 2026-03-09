//! Reranker implementations — cross-encoder reranking of RRF results.
//!
//! Supports Voyage, Jina, Cohere, and any OpenAI-compatible rerank
//! endpoint via the generic `HttpReranker`.

use serde::{Deserialize, Serialize};

/// Configuration for the reranker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankConfig {
    /// API key for the rerank provider.
    pub api_key: String,
    /// Base URL (default: <https://api.voyageai.com/v1>).
    pub base_url: String,
    /// Model name (default: rerank-2.5).
    pub model: String,
    /// Number of top results to rerank.
    pub top_k: usize,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.voyageai.com/v1".to_string(),
            model: "rerank-2.5".to_string(),
            top_k: 20,
        }
    }
}

/// A reranked result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankedResult {
    /// Original index in the input list.
    pub index: usize,
    /// Reranking relevance score.
    pub relevance_score: f64,
}

/// Trait for reranking search results.
#[async_trait::async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank documents against a query.
    ///
    /// Returns results sorted by relevance (highest first).
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> crate::error::Result<Vec<RerankedResult>>;
}

/// Pass-through reranker that preserves original ordering.
///
/// Used when no reranker API key is configured.
pub struct NoopReranker;

#[async_trait::async_trait]
impl Reranker for NoopReranker {
    async fn rerank(
        &self,
        _query: &str,
        documents: &[String],
    ) -> crate::error::Result<Vec<RerankedResult>> {
        let len = documents.len();
        Ok((0..len)
            .map(|i| {
                let score = if len <= 1 {
                    1.0
                } else {
                    1.0 - (i as f64 / len as f64)
                };
                RerankedResult {
                    index: i,
                    relevance_score: score,
                }
            })
            .collect())
    }
}

/// HTTP-based reranker that calls an OpenAI-compatible rerank endpoint.
///
/// Works with Voyage, Jina, Cohere, and other providers that accept
/// a POST request with `query` and `documents` fields and return
/// scored results.
pub struct HttpReranker {
    /// The HTTP client.
    client: reqwest::Client,
    /// Configuration for the reranker.
    config: RerankConfig,
}

/// Request body for the rerank API.
#[derive(Debug, Serialize)]
struct RerankRequest<'a> {
    /// The model to use for reranking.
    model: &'a str,
    /// The query to rerank against.
    query: &'a str,
    /// The documents to rerank.
    documents: &'a [String],
    /// Maximum number of results to return.
    top_k: usize,
}

/// A single result from the rerank API response.
#[derive(Debug, Deserialize)]
struct RerankApiResult {
    /// Original index of the document.
    index: usize,
    /// Relevance score assigned by the model.
    relevance_score: f64,
}

/// Response body from the rerank API.
#[derive(Debug, Deserialize)]
struct RerankResponse {
    /// The reranked results.
    #[serde(alias = "results")]
    data: Vec<RerankApiResult>,
}

impl HttpReranker {
    /// Create a new HTTP-based reranker.
    pub fn new(config: RerankConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

#[async_trait::async_trait]
impl Reranker for HttpReranker {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> crate::error::Result<Vec<RerankedResult>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let top_k = self.config.top_k.min(documents.len());
        let url = format!("{}/rerank", self.config.base_url);
        let body = RerankRequest {
            model: &self.config.model,
            query,
            documents,
            top_k,
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::Error::Search(format!("rerank HTTP error: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_else(|_| "unknown".into());
            return Err(crate::error::Error::Search(format!(
                "rerank API returned {status}: {body}"
            )));
        }

        let parsed: RerankResponse = response.json().await.map_err(|e| {
            crate::error::Error::Search(format!("rerank response parse error: {e}"))
        })?;

        Ok(parsed
            .data
            .into_iter()
            .map(|r| RerankedResult {
                index: r.index,
                relevance_score: r.relevance_score,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_reranker_preserves_order() {
        let reranker = NoopReranker;
        let docs = vec![
            "first doc".to_string(),
            "second doc".to_string(),
            "third doc".to_string(),
        ];

        let results = reranker.rerank("query", &docs).await.ok();
        let results = results.unwrap_or_default();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].index, 0);
        assert_eq!(results[1].index, 1);
        assert_eq!(results[2].index, 2);

        // Scores should be descending
        assert!(results[0].relevance_score > results[1].relevance_score);
        assert!(results[1].relevance_score > results[2].relevance_score);
    }

    #[test]
    fn rerank_config_defaults() {
        let config = RerankConfig::default();
        assert_eq!(config.model, "rerank-2.5");
        assert_eq!(config.top_k, 20);
        assert_eq!(config.base_url, "https://api.voyageai.com/v1");
        assert!(config.api_key.is_empty());
    }

    #[tokio::test]
    async fn noop_reranker_empty_docs() {
        let reranker = NoopReranker;
        let docs: Vec<String> = vec![];

        let results = reranker.rerank("query", &docs).await.ok();
        let results = results.unwrap_or_default();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn noop_reranker_single_doc() {
        let reranker = NoopReranker;
        let docs = vec!["only doc".to_string()];

        let results = reranker.rerank("query", &docs).await.ok();
        let results = results.unwrap_or_default();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].index, 0);
        assert!((results[0].relevance_score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn http_reranker_creates_with_config() {
        let config = RerankConfig {
            api_key: "test-key".to_string(),
            base_url: "http://localhost:9999".to_string(),
            model: "test-model".to_string(),
            top_k: 5,
        };
        let _reranker = HttpReranker::new(config);
    }
}
