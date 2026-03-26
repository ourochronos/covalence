//! API error handling — maps core errors to HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use validator::ValidationErrors;

/// Wrapper around the core error type for HTTP response conversion.
pub struct ApiError(pub covalence_core::error::Error);

/// Validate a request DTO, converting validation errors into an
/// `ApiError` with a 400 Bad Request response.
pub fn validate_request<T: validator::Validate>(req: &T) -> Result<(), ApiError> {
    req.validate().map_err(|e| {
        ApiError::from(covalence_core::error::Error::InvalidInput(
            format_validation_errors(&e),
        ))
    })
}

/// Format validation errors into a human-readable message.
fn format_validation_errors(errors: &ValidationErrors) -> String {
    let mut parts = Vec::new();
    for (field, field_errors) in errors.field_errors() {
        for error in field_errors {
            let msg = error
                .message
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| format!("{field}: validation failed ({:?})", error.code));
            parts.push(msg);
        }
    }
    if parts.is_empty() {
        "validation failed".to_string()
    } else {
        parts.join("; ")
    }
}

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
            Error::Queue(_) => (StatusCode::INTERNAL_SERVER_ERROR, "queue_error"),
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
