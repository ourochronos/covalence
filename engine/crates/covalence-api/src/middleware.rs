//! Authentication middleware for API key validation.
//!
//! When `COVALENCE_API_KEY` is configured, all requests must include
//! an `Authorization: Bearer <key>` header — except public endpoints
//! like `/health`, `/docs`, and `/openapi.json`.

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::state::AppState;

/// Axum middleware that enforces Bearer token authentication.
///
/// If `config.api_key` is `None`, all requests pass through
/// (development mode). Otherwise, requests to non-public paths
/// must carry a matching `Authorization: Bearer <key>` header.
pub async fn require_api_key(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(ref expected_key) = state.config.api_key else {
        // No API key configured — development mode, allow all.
        return next.run(request).await;
    };

    // Public endpoints that skip authentication.
    let path = request.uri().path();
    if is_public_path(path) {
        return next.run(request).await;
    }

    // Extract and validate the Bearer token.
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header.and_then(|v| v.strip_prefix("Bearer ")) {
        Some(token) => {
            if token == expected_key {
                next.run(request).await
            } else {
                unauthorized_response("invalid API key")
            }
        }
        None if auth_header.is_some() => {
            unauthorized_response("invalid authorization scheme, expected Bearer")
        }
        None => unauthorized_response("missing Authorization header"),
    }
}

/// Returns `true` for paths that do not require authentication.
fn is_public_path(path: &str) -> bool {
    matches!(path, "/health" | "/openapi.json") || path.starts_with("/docs")
}

/// Build a 401 Unauthorized JSON response.
fn unauthorized_response(message: &str) -> Response {
    let body = json!({
        "error": {
            "code": "auth_error",
            "message": message
        }
    });

    (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_is_public() {
        assert!(is_public_path("/health"));
    }

    #[test]
    fn openapi_json_is_public() {
        assert!(is_public_path("/openapi.json"));
    }

    #[test]
    fn docs_root_is_public() {
        assert!(is_public_path("/docs"));
    }

    #[test]
    fn docs_trailing_slash_is_public() {
        assert!(is_public_path("/docs/"));
    }

    #[test]
    fn docs_assets_are_public() {
        assert!(is_public_path("/docs/swagger-ui.css"));
        assert!(is_public_path("/docs/swagger-ui-bundle.js"));
    }

    #[test]
    fn api_routes_are_not_public() {
        assert!(!is_public_path("/api/v1/search"));
        assert!(!is_public_path("/api/v1/sources"));
        assert!(!is_public_path("/api/v1/admin/health"));
    }
}
