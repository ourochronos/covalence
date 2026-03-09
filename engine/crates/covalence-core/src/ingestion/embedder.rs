//! Stage 5: Generate embeddings with contextual prefix.
//!
//! Each chunk receives a short context prefix summarizing the
//! document-level context before embedding. Prevents the "orphan chunk"
//! problem.

/// Trait for embedding text into vector representations.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Generate embeddings for a batch of texts.
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f64>>>;

    /// The dimensionality of the embedding vectors produced.
    fn dimension(&self) -> usize;

    /// The name of the underlying model.
    fn model_name(&self) -> &str;
}

/// A mock embedder that returns zero vectors of a configured dimension.
pub struct MockEmbedder {
    dim: usize,
    model: String,
}

impl MockEmbedder {
    /// Create a new mock embedder with the given dimension.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            model: "mock".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f64>>> {
        Ok(texts.iter().map(|_| vec![0.0; self.dim]).collect())
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embedder_returns_correct_dimensions() {
        let embedder = MockEmbedder::new(2048);
        let texts = vec!["hello".to_string(), "world".to_string()];
        let result = embedder.embed(&texts).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 2048);
        assert_eq!(result[1].len(), 2048);
    }

    #[tokio::test]
    async fn mock_embedder_returns_zeros() {
        let embedder = MockEmbedder::new(3);
        let texts = vec!["test".to_string()];
        let result = embedder.embed(&texts).await.unwrap();
        assert_eq!(result[0], vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn mock_embedder_metadata() {
        let embedder = MockEmbedder::new(2048);
        assert_eq!(embedder.dimension(), 2048);
        assert_eq!(embedder.model_name(), "mock");
    }
}
