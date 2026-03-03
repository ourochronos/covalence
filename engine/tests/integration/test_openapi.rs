//! Integration tests for GET /openapi.json and GET /docs (covalence#46).
//!
//! Verifies that:
//! * GET /openapi.json returns HTTP 200 with Content-Type application/json
//! * GET /openapi.json body contains "Covalence Knowledge Engine"
//! * GET /docs returns HTTP 200 with Content-Type text/html

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::setup_pool;

#[tokio::test]
#[serial]
async fn test_openapi_json_returns_200_with_application_json() {
    let pool = setup_pool().await;
    let state = super::helpers::make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let request = Request::builder()
        .method("GET")
        .uri("/openapi.json")
        .body(Body::empty())
        .expect("request builder failed");

    let response = app.oneshot(request).await.expect("oneshot failed");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /openapi.json should return 200 OK"
    );

    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    assert!(
        content_type.contains("application/json"),
        "Content-Type should contain 'application/json', got: {content_type}"
    );
}

#[tokio::test]
#[serial]
async fn test_openapi_json_body_contains_title() {
    let pool = setup_pool().await;
    let state = super::helpers::make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let request = Request::builder()
        .method("GET")
        .uri("/openapi.json")
        .body(Body::empty())
        .expect("request builder failed");

    let response = app.oneshot(request).await.expect("oneshot failed");

    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to collect body");

    let body_str = std::str::from_utf8(&bytes).expect("body is not valid UTF-8");

    assert!(
        body_str.contains("Covalence Knowledge Engine"),
        "Response body should contain 'Covalence Knowledge Engine'"
    );
}

#[tokio::test]
#[serial]
async fn test_docs_returns_html() {
    let pool = setup_pool().await;
    let state = super::helpers::make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let request = Request::builder()
        .method("GET")
        .uri("/docs")
        .body(Body::empty())
        .expect("request builder failed");

    let response = app.oneshot(request).await.expect("oneshot failed");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "GET /docs should return 200 OK"
    );

    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    assert!(
        content_type.contains("text/html"),
        "Content-Type should contain 'text/html', got: {content_type}"
    );
}
