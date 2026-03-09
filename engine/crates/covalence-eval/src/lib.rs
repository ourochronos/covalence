//! Layer-by-layer evaluation harness for the Covalence pipeline.
//!
//! Provides a framework for independently evaluating each stage of
//! the ingestion and search pipeline: chunking, extraction, and
//! search. Each layer evaluator takes standard inputs and produces
//! typed metrics that can be compared across parameter sweeps.

pub mod chunker_eval;
pub mod error;
pub mod extractor_eval;
pub mod fixtures;
pub mod metrics;
pub mod search_eval;

pub use chunker_eval::ChunkerEval;
pub use error::{EvalError, Result};
pub use extractor_eval::ExtractorEval;
pub use metrics::{ChunkerMetrics, ExtractorMetrics, SearchMetrics};
pub use search_eval::SearchEval;

/// Trait for evaluating a pipeline layer.
///
/// Each layer evaluator converts an input into an output, then
/// scores the output against an expected baseline to produce
/// typed metrics.
pub trait LayerEvaluator {
    /// The input to the layer being evaluated.
    type Input;
    /// The output produced by the layer.
    type Output;
    /// The metrics produced by scoring output vs expected.
    type Metrics;

    /// Run the layer on the given input, producing output.
    fn evaluate(&self, input: &Self::Input) -> Self::Output;

    /// Score the actual output against the expected output.
    fn score(&self, output: &Self::Output, expected: &Self::Output) -> Self::Metrics;
}
