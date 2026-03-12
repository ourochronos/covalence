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

    /// Whether this reranker is a no-op pass-through.
    ///
    /// When true, the search service skips score blending to avoid
    /// distorting fusion scores with artificial reranker scores.
    fn is_noop(&self) -> bool {
        false
    }
}

/// Pass-through reranker that preserves original ordering.
///
/// Used when no reranker API key is configured. Returns uniform
/// scores so score blending doesn't distort fusion results.
pub struct NoopReranker;

#[async_trait::async_trait]
impl Reranker for NoopReranker {
    async fn rerank(
        &self,
        _query: &str,
        documents: &[String],
    ) -> crate::error::Result<Vec<RerankedResult>> {
        Ok((0..documents.len())
            .map(|i| RerankedResult {
                index: i,
                relevance_score: 1.0,
            })
            .collect())
    }

    fn is_noop(&self) -> bool {
        true
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

        // Filter out empty/whitespace documents — Voyage rerank-2.5
        // returns HTTP 400 if any document is an empty string.
        // Track original indices so we can map scores back.
        let indexed_docs: Vec<(usize, &String)> = documents
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.trim().is_empty())
            .collect();

        if indexed_docs.is_empty() {
            // All documents were empty — return identity ordering.
            return Ok(documents
                .iter()
                .enumerate()
                .map(|(i, _)| RerankedResult {
                    index: i,
                    relevance_score: 0.0,
                })
                .collect());
        }

        let filtered_docs: Vec<String> = indexed_docs.iter().map(|(_, d)| (*d).clone()).collect();
        let original_indices: Vec<usize> = indexed_docs.iter().map(|(i, _)| *i).collect();

        let top_k = self.config.top_k.min(filtered_docs.len());
        let url = format!("{}/rerank", self.config.base_url);
        let body = RerankRequest {
            model: &self.config.model,
            query,
            documents: &filtered_docs,
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

        // Map filtered indices back to original document indices.
        let mut results: Vec<RerankedResult> = parsed
            .data
            .into_iter()
            .filter_map(|r| {
                original_indices
                    .get(r.index)
                    .map(|&orig_idx| RerankedResult {
                        index: orig_idx,
                        relevance_score: r.relevance_score,
                    })
            })
            .collect();

        // Append empty documents at the end with zero relevance so
        // callers that expect one result per input still work.
        let returned: std::collections::HashSet<usize> = results.iter().map(|r| r.index).collect();
        for i in 0..documents.len() {
            if !returned.contains(&i) {
                results.push(RerankedResult {
                    index: i,
                    relevance_score: 0.0,
                });
            }
        }

        Ok(results)
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

        // All scores should be uniform (1.0) so blending is a no-op.
        for r in &results {
            assert!((r.relevance_score - 1.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn noop_reranker_is_noop() {
        assert!(NoopReranker.is_noop());
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

    #[test]
    fn empty_doc_filter_preserves_indices() {
        // Simulate the filtering logic used in HttpReranker::rerank.
        let documents = vec![
            "first".to_string(),
            "".to_string(),
            "third".to_string(),
            "   ".to_string(),
        ];
        let indexed_docs: Vec<(usize, &String)> = documents
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.trim().is_empty())
            .collect();
        let original_indices: Vec<usize> = indexed_docs.iter().map(|(i, _)| *i).collect();

        assert_eq!(original_indices, vec![0, 2]);
    }

    #[test]
    fn all_empty_docs_returns_identity() {
        let documents: Vec<String> = vec!["".to_string(), "   ".to_string(), "\n".to_string()];
        let indexed_docs: Vec<(usize, &String)> = documents
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.trim().is_empty())
            .collect();
        assert!(indexed_docs.is_empty());
    }
}
