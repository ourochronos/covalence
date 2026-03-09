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

    /// Embed chunks from a single document with context awareness.
    ///
    /// Providers that support contextual chunk embeddings (e.g.,
    /// Voyage AI `voyage-context-3`) produce embeddings where each
    /// chunk vector reflects the surrounding document context. This
    /// prevents the "orphan chunk" problem where a chunk about
    /// "it" loses the referent.
    ///
    /// Providers without contextual support fall back to
    /// independent per-chunk embeddings via [`embed`].
    async fn embed_document_chunks(
        &self,
        chunks: &[String],
    ) -> crate::error::Result<Vec<Vec<f64>>> {
        self.embed(chunks).await
    }

    /// The dimensionality of the embedding vectors produced.
    fn dimension(&self) -> usize;

    /// The name of the underlying model.
    fn model_name(&self) -> &str;
}

/// Truncate an embedding to `target_dim` and validate the result.
///
/// Combines [`truncate_embedding`] with a dimension check so that
/// callers never accidentally pass an oversized vector to the
/// database. Returns an error if the truncated vector is still
/// larger than `target_dim` (should never happen, but guards
/// against logic errors).
pub fn truncate_and_validate(
    embedding: &[f64],
    target_dim: usize,
    table: &str,
) -> crate::error::Result<Vec<f64>> {
    let result = truncate_embedding(embedding, target_dim);
    if result.len() != target_dim && !embedding.is_empty() && embedding.len() > target_dim {
        return Err(crate::error::Error::Embedding(format!(
            "dimension mismatch for {table}: expected {target_dim}, got {}",
            result.len(),
        )));
    }
    Ok(result)
}

/// Truncate an embedding vector to `target_dim` dimensions and
/// L2-normalize the result.
///
/// For matryoshka-style models (OpenAI `text-embedding-3-*`, Jina v3),
/// the first N dimensions capture progressively less information,
/// so truncation + renormalization preserves quality.
///
/// Returns the original vector unchanged if it is already at or
/// below the target dimension.
pub fn truncate_embedding(embedding: &[f64], target_dim: usize) -> Vec<f64> {
    if embedding.len() <= target_dim {
        return embedding.to_vec();
    }
    let truncated = &embedding[..target_dim];
    let norm: f64 = truncated.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm < 1e-12 {
        return truncated.to_vec();
    }
    truncated.iter().map(|v| v / norm).collect()
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

    #[test]
    fn truncate_embedding_reduces_dims() {
        let v = vec![0.6, 0.8, 0.1, 0.2];
        let t = truncate_embedding(&v, 2);
        assert_eq!(t.len(), 2);
        // Should be L2-normalized
        let norm: f64 = t.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 1e-10);
    }

    #[test]
    fn truncate_embedding_noop_when_at_target() {
        let v = vec![0.5, 0.5];
        let t = truncate_embedding(&v, 2);
        assert_eq!(t, v);
    }

    #[test]
    fn truncate_embedding_noop_when_below_target() {
        let v = vec![1.0];
        let t = truncate_embedding(&v, 5);
        assert_eq!(t, v);
    }

    #[test]
    fn truncate_embedding_zero_vector() {
        let v = vec![0.0, 0.0, 0.0, 0.0];
        let t = truncate_embedding(&v, 2);
        assert_eq!(t, vec![0.0, 0.0]);
    }

    #[test]
    fn truncate_and_validate_ok() {
        let v = vec![0.6, 0.8, 0.1, 0.2];
        let result = truncate_and_validate(&v, 2, "chunks");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[test]
    fn truncate_and_validate_noop_at_target() {
        let v = vec![0.5, 0.5];
        let result = truncate_and_validate(&v, 2, "chunks");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), v);
    }

    #[test]
    fn truncate_and_validate_below_target() {
        // Embedding shorter than target is allowed (noop).
        let v = vec![1.0];
        let result = truncate_and_validate(&v, 5, "nodes");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), v);
    }
}
