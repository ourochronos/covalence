//! API key authentication middleware.
//!
//! Behaviour:
//! - `GET /health` is always allowed (no key required).
//! - When `COVALENCE_API_KEY` is **not** set the server runs in dev mode and
//!   every request is allowed through unchanged.
//! - When the env var **is** set, every request must supply the matching key
//!   via one of:
//!     - `Authorization: Bearer <key>`  (checked first)
//!     - `X-Api-Key: <key>`
//!
//!   A missing or wrong key → `401 {"error":"unauthorized"}`.

use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::AppState;

/// Axum middleware function.  Wire up with:
///
/// ```rust,ignore
/// .layer(axum::middleware::from_fn_with_state(state.clone(), require_api_key))
/// ```
pub async fn require_api_key(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Health endpoint is always public.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    // Dev mode: no key configured → allow everything.
    let Some(ref expected) = state.api_key else {
        return next.run(req).await;
    };

    // Try `Authorization: Bearer <key>` first, then `X-Api-Key: <key>`.
    let provided = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .or_else(|| req.headers().get("x-api-key").and_then(|v| v.to_str().ok()));

    match provided {
        Some(key) if key == expected.as_str() => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        )
            .into_response(),
    }
}
