//! Typed errors for the evaluation harness.

/// Errors that can occur during evaluation.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// Failed to read or parse a fixture file.
    #[error("fixture error: {0}")]
    Fixture(String),

    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// I/O error reading files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid configuration for an evaluator.
    #[error("config error: {0}")]
    Config(String),
}

/// Result type alias for evaluation operations.
pub type Result<T> = std::result::Result<T, EvalError>;
