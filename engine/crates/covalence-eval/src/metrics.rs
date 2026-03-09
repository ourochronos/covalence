//! Metric types produced by each layer evaluator.

use serde::{Deserialize, Serialize};

/// Metrics for the chunking layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkerMetrics {
    /// Fraction of source text covered by chunks (0.0..=1.0).
    pub coverage: f64,
    /// Total number of chunks produced.
    pub chunk_count: usize,
    /// Average chunk size in bytes.
    pub avg_chunk_size: f64,
    /// Minimum chunk size in bytes.
    pub min_chunk_size: usize,
    /// Maximum chunk size in bytes.
    pub max_chunk_size: usize,
    /// Number of document-level chunks.
    pub document_chunks: usize,
    /// Number of section-level chunks.
    pub section_chunks: usize,
    /// Number of paragraph-level chunks.
    pub paragraph_chunks: usize,
}

/// Metrics for the extraction layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractorMetrics {
    /// Precision: fraction of predicted entities in the gold set.
    pub precision: f64,
    /// Recall: fraction of gold entities that were predicted.
    pub recall: f64,
    /// F1 score: harmonic mean of precision and recall.
    pub f1: f64,
    /// Number of entities predicted.
    pub predicted_count: usize,
    /// Number of entities in the gold set.
    pub gold_count: usize,
    /// Number of correctly predicted entities (true positives).
    pub true_positives: usize,
}

/// Metrics for the search layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetrics {
    /// Precision at K.
    pub precision_at_k: f64,
    /// Normalized Discounted Cumulative Gain.
    pub ndcg: f64,
    /// Mean Reciprocal Rank.
    pub mrr: f64,
    /// Number of results returned.
    pub result_count: usize,
    /// The K value used for precision@K.
    pub k: usize,
}
