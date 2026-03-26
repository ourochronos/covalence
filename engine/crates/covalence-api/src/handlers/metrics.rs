//! Prometheus metrics endpoint handler.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Render Prometheus text-format metrics.
///
/// Mounted at `/metrics` (root level, not under `/api/v1`) following
/// Prometheus convention.
pub async fn prometheus_metrics(State(state): State<AppState>) -> Response {
    match &state.prometheus_handle {
        Some(handle) => {
            let body = handle.render();
            (
                StatusCode::OK,
                [("content-type", "text/plain; charset=utf-8")],
                body,
            )
                .into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics recorder not installed",
        )
            .into_response(),
    }
}
