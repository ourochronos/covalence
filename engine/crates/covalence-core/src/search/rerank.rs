//! Voyage rerank-2.5 integration.
//!
//! Cross-encoder reranking of top-k RRF results using the
//! Voyage rerank API. $0.05/M tokens, first 200M free.

use serde::{Deserialize, Serialize};

/// Configuration for the reranker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankConfig {
    /// API key for Voyage rerank.
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
}
