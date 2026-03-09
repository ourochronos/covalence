//! Chunker layer evaluator.
//!
//! Evaluates the chunking stage by measuring coverage, chunk counts,
//! and size distributions against expected output.

use covalence_core::ingestion::{ChunkLevel, ChunkOutput, chunk_document};

use crate::LayerEvaluator;
use crate::metrics::ChunkerMetrics;

/// Configuration for the chunker evaluator.
#[derive(Debug, Clone)]
pub struct ChunkerEval {
    /// Maximum chunk size in bytes before paragraph splitting.
    pub max_chunk_size: usize,
}

impl ChunkerEval {
    /// Create a new chunker evaluator with the given max chunk size.
    pub fn new(max_chunk_size: usize) -> Self {
        Self { max_chunk_size }
    }
}

impl Default for ChunkerEval {
    fn default() -> Self {
        Self {
            max_chunk_size: 500,
        }
    }
}

/// Input to the chunker evaluator.
#[derive(Debug, Clone)]
pub struct ChunkerInput {
    /// Raw document text to chunk.
    pub text: String,
}

/// Output from the chunker evaluator.
#[derive(Debug, Clone)]
pub struct ChunkerOutput {
    /// Chunks produced by the chunker.
    pub chunks: Vec<ChunkOutput>,
    /// Original source text length in bytes.
    pub source_len: usize,
}

impl LayerEvaluator for ChunkerEval {
    type Input = ChunkerInput;
    type Output = ChunkerOutput;
    type Metrics = ChunkerMetrics;

    fn evaluate(&self, input: &Self::Input) -> Self::Output {
        let chunks = chunk_document(&input.text, self.max_chunk_size, 0);
        ChunkerOutput {
            chunks,
            source_len: input.text.len(),
        }
    }

    fn score(&self, output: &Self::Output, _expected: &Self::Output) -> Self::Metrics {
        compute_chunker_metrics(output)
    }
}

/// Compute chunker metrics from output (standalone function for
/// reuse outside the trait).
pub fn compute_chunker_metrics(output: &ChunkerOutput) -> ChunkerMetrics {
    let non_doc_chunks: Vec<&ChunkOutput> = output
        .chunks
        .iter()
        .filter(|c| c.level != ChunkLevel::Document)
        .collect();

    let chunk_sizes: Vec<usize> = non_doc_chunks.iter().map(|c| c.text.len()).collect();

    let total_chunk_bytes: usize = chunk_sizes.iter().sum();
    let coverage = if output.source_len > 0 {
        // Coverage can exceed 1.0 because sections overlap
        // with paragraphs. Clamp to 1.0 for reporting.
        let raw = total_chunk_bytes as f64 / output.source_len as f64;
        raw.min(1.0)
    } else {
        0.0
    };

    let avg_chunk_size = if chunk_sizes.is_empty() {
        0.0
    } else {
        total_chunk_bytes as f64 / chunk_sizes.len() as f64
    };

    let min_chunk_size = chunk_sizes.iter().copied().min().unwrap_or(0);
    let max_chunk_size = chunk_sizes.iter().copied().max().unwrap_or(0);

    let document_chunks = output
        .chunks
        .iter()
        .filter(|c| c.level == ChunkLevel::Document)
        .count();
    let section_chunks = output
        .chunks
        .iter()
        .filter(|c| c.level == ChunkLevel::Section)
        .count();
    let paragraph_chunks = output
        .chunks
        .iter()
        .filter(|c| c.level == ChunkLevel::Paragraph)
        .count();

    ChunkerMetrics {
        coverage,
        chunk_count: output.chunks.len(),
        avg_chunk_size,
        min_chunk_size,
        max_chunk_size,
        document_chunks,
        section_chunks,
        paragraph_chunks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_produces_zero_coverage() {
        let eval = ChunkerEval::default();
        let input = ChunkerInput {
            text: String::new(),
        };
        let output = eval.evaluate(&input);
        let metrics = eval.score(&output, &output);
        assert_eq!(metrics.coverage, 0.0);
        assert_eq!(metrics.chunk_count, 0);
    }

    #[test]
    fn single_section_coverage() {
        let eval = ChunkerEval::new(5000);
        let input = ChunkerInput {
            text: "# Title\n\nSome content here.".to_string(),
        };
        let output = eval.evaluate(&input);
        let metrics = eval.score(&output, &output);
        assert!(metrics.coverage > 0.0);
        assert!(metrics.section_chunks >= 1);
        assert_eq!(metrics.document_chunks, 1);
    }

    #[test]
    fn paragraph_splitting_increases_count() {
        let long_text = format!(
            "# Section\n\n{}\n\n{}",
            "word ".repeat(200),
            "other ".repeat(200),
        );
        let small = ChunkerEval::new(50);
        let large = ChunkerEval::new(50000);

        let input = ChunkerInput { text: long_text };

        let small_output = small.evaluate(&input);
        let large_output = large.evaluate(&input);

        let small_metrics = small.score(&small_output, &small_output);
        let large_metrics = large.score(&large_output, &large_output);

        assert!(
            small_metrics.chunk_count >= large_metrics.chunk_count,
            "smaller max_chunk_size should produce >= chunks"
        );
    }

    #[test]
    fn metrics_counts_by_level() {
        let md = "# A\n\nContent A.\n\n# B\n\nContent B.";
        let eval = ChunkerEval::new(5000);
        let input = ChunkerInput {
            text: md.to_string(),
        };
        let output = eval.evaluate(&input);
        let metrics = eval.score(&output, &output);
        assert_eq!(metrics.document_chunks, 1);
        assert_eq!(metrics.section_chunks, 2);
        assert_eq!(metrics.paragraph_chunks, 0);
    }

    #[test]
    fn avg_chunk_size_computed_correctly() {
        let eval = ChunkerEval::new(5000);
        let input = ChunkerInput {
            text: "# Title\n\nHello world.".to_string(),
        };
        let output = eval.evaluate(&input);
        let metrics = eval.score(&output, &output);
        assert!(metrics.avg_chunk_size > 0.0);
        assert!(metrics.min_chunk_size <= metrics.max_chunk_size);
    }
}
