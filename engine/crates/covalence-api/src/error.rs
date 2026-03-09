//! API error handling — maps core errors to HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Wrapper around the core error type for HTTP response conversion.
pub struct ApiError(pub covalence_core::error::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        use covalence_core::error::Error;

        let (status, code) = match &self.0 {
            Error::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
            Error::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid_input"),
            Error::Auth(_) => (StatusCode::UNAUTHORIZED, "auth_error"),
            Error::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database_error"),
            Error::Serialization(_) => (StatusCode::BAD_REQUEST, "serialization_error"),
            Error::Graph(_) => (StatusCode::INTERNAL_SERVER_ERROR, "graph_error"),
            Error::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "config_error"),
            Error::Ingestion(_) => (StatusCode::INTERNAL_SERVER_ERROR, "ingestion_error"),
            Error::Search(_) => (StatusCode::INTERNAL_SERVER_ERROR, "search_error"),
            Error::Embedding(_) => (StatusCode::INTERNAL_SERVER_ERROR, "embedding_error"),
            Error::EntityResolution(_) => (StatusCode::INTERNAL_SERVER_ERROR, "resolution_error"),
            Error::Consolidation(_) => (StatusCode::INTERNAL_SERVER_ERROR, "consolidation_error"),
        };

        let body = json!({
            "error": {
                "code": code,
                "message": self.0.to_string()
            }
        });

        (status, axum::Json(body)).into_response()
    }
}

impl From<covalence_core::error::Error> for ApiError {
    fn from(err: covalence_core::error::Error) -> Self {
        Self(err)
    }
}
