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

    /// Retry queue operation failed.
    #[error("queue error: {0}")]
    Queue(String),

    /// Lifecycle hook call failed.
    #[error("hook error: {0}")]
    Hook(String),
}

/// Result type alias using the Covalence error.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_display() {
        let err = Error::NotFound {
            entity_type: "node",
            id: "abc-123".into(),
        };
        assert_eq!(err.to_string(), "node not found: abc-123");
    }

    #[test]
    fn invalid_input_display() {
        let err = Error::InvalidInput("bad data".into());
        assert_eq!(err.to_string(), "invalid input: bad data");
    }

    #[test]
    fn graph_error_display() {
        let err = Error::Graph("cycle detected".into());
        assert_eq!(err.to_string(), "graph error: cycle detected");
    }

    #[test]
    fn config_error_display() {
        let err = Error::Config("missing key".into());
        assert_eq!(err.to_string(), "config error: missing key");
    }

    #[test]
    fn ingestion_error_display() {
        let err = Error::Ingestion("parse failed".into());
        assert_eq!(err.to_string(), "ingestion error: parse failed");
    }

    #[test]
    fn search_error_display() {
        let err = Error::Search("timeout".into());
        assert_eq!(err.to_string(), "search error: timeout");
    }

    #[test]
    fn embedding_error_display() {
        let err = Error::Embedding("dim mismatch".into());
        assert_eq!(err.to_string(), "embedding error: dim mismatch");
    }

    #[test]
    fn entity_resolution_error_display() {
        let err = Error::EntityResolution("ambiguous".into());
        assert_eq!(err.to_string(), "entity resolution error: ambiguous");
    }

    #[test]
    fn consolidation_error_display() {
        let err = Error::Consolidation("stale lock".into());
        assert_eq!(err.to_string(), "consolidation error: stale lock");
    }

    #[test]
    fn auth_error_display() {
        let err = Error::Auth("unauthorized".into());
        assert_eq!(err.to_string(), "auth error: unauthorized");
    }

    #[test]
    fn hook_error_display() {
        let err = Error::Hook("timeout after 2000ms".into());
        assert_eq!(err.to_string(), "hook error: timeout after 2000ms");
    }

    #[test]
    fn from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let err: Error = json_err.into();
        assert!(matches!(err, Error::Serialization(_)));
        assert!(err.to_string().contains("serialization error"));
    }
}
