//! Integration test for GET /dashboard (tracking#93).
//!
//! Verifies that:
//! * The route returns HTTP 200.
//! * The `Content-Type` header includes `text/html`.
//! * The response body contains the expected landmark text "Covalence Dashboard".

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _; // for `oneshot`

use covalence_engine::api::{AppState, routes};

use super::helpers::{MockLlmClient, setup_pool};

#[tokio::test]
#[serial]
async fn test_dashboard_returns_html_page() {
    // ── Setup ──────────────────────────────────────────────────────────────
    let pool = setup_pool().await;
    let llm: Arc<dyn covalence_engine::worker::llm::LlmClient> = Arc::new(MockLlmClient::new());
    let state = AppState { pool, llm };

    let app = routes::router().with_state(state);

    // ── Execute GET /dashboard ─────────────────────────────────────────────
    let request = Request::builder()
        .method("GET")
        .uri("/dashboard")
        .body(Body::empty())
        .expect("request builder failed");

    let response = app
        .oneshot(request)
        .await
        .expect("dashboard oneshot failed");

    // ── Assert status ──────────────────────────────────────────────────────
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /dashboard should return 200 OK"
    );

    // ── Assert Content-Type ────────────────────────────────────────────────
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    assert!(
        content_type.contains("text/html"),
        "Content-Type should contain 'text/html', got: {content_type}"
    );

    // ── Assert body landmark ───────────────────────────────────────────────
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    let body = std::str::from_utf8(&body_bytes).expect("body is not valid UTF-8");

    assert!(
        body.contains("Covalence Dashboard"),
        "response body should contain 'Covalence Dashboard'"
    );
}
