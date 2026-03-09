//! Typed error types for the Covalence engine.
//!
//! Uses `thiserror` for all library errors. Binary crates use `anyhow` for
//! top-level error handling.

/// Top-level error type for the Covalence engine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database operation failed.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Entity not found.
    #[error("{entity_type} not found: {id}")]
    NotFound {
        /// The type of entity that was not found.
        entity_type: &'static str,
        /// The ID that was looked up.
        id: String,
    },

    /// Invalid input provided.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Serialization/deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Graph operation failed.
    #[error("graph error: {0}")]
    Graph(String),

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// Ingestion pipeline failed.
    #[error("ingestion error: {0}")]
    Ingestion(String),

    /// Search operation failed.
    #[error("search error: {0}")]
    Search(String),

    /// Embedding generation failed.
    #[error("embedding error: {0}")]
    Embedding(String),

    /// Entity resolution failed.
    #[error("entity resolution error: {0}")]
    EntityResolution(String),

    /// Consolidation (batch or deep) failed.
    #[error("consolidation error: {0}")]
    Consolidation(String),

    /// Authentication or authorization error.
    #[error("auth error: {0}")]
    Auth(String),
}

/// Result type alias using the Covalence error.
pub type Result<T> = std::result::Result<T, Error>;
