//! Integration tests for the standing concerns API (tracking#96).
//!
//! Exercises POST /admin/concerns and GET /admin/concerns.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::setup_pool;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn make_app() -> axum::Router {
    let pool = setup_pool().await;
    let state = super::helpers::make_test_state(pool).await;
    routes::router().with_state(state)
}

async fn post_concerns(
    app: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/admin/concerns")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request builder failed");

    let resp = app.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("failed to read body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn get_concerns(app: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/admin/concerns")
        .body(Body::empty())
        .expect("request builder failed");

    let resp = app.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("failed to read body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

// ── test 1: upsert creates concerns ──────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_upsert_concerns_creates() {
    let app = make_app().await;

    let payload = serde_json::json!([
        { "name": "database",   "status": "green",  "notes": "all replicas healthy" },
        { "name": "queue",      "status": "yellow", "notes": "backlog growing" },
        { "name": "embeddings", "status": "red",    "notes": "OpenAI unreachable" }
    ]);

    let (post_status, post_body) = post_concerns(app.clone(), payload).await;
    assert_eq!(
        post_status,
        StatusCode::OK,
        "POST should return 200: {post_body}"
    );

    let posted = post_body["data"].as_array().expect("data should be array");
    assert_eq!(posted.len(), 3, "should have 3 upserted concerns");

    let (get_status, get_body) = get_concerns(app).await;
    assert_eq!(
        get_status,
        StatusCode::OK,
        "GET should return 200: {get_body}"
    );

    let listed = get_body["data"].as_array().expect("data should be array");
    assert_eq!(listed.len(), 3, "GET should return all 3 concerns");

    // Verify ordering is alphabetical (database, embeddings, queue).
    assert_eq!(listed[0]["name"], "database");
    assert_eq!(listed[0]["status"], "green");
    assert_eq!(listed[1]["name"], "embeddings");
    assert_eq!(listed[1]["status"], "red");
    assert_eq!(listed[2]["name"], "queue");
    assert_eq!(listed[2]["status"], "yellow");
}

// ── test 2: upsert updates existing concern ───────────────────────────────────

#[tokio::test]
#[serial]
async fn test_upsert_concerns_updates() {
    let app = make_app().await;

    // First write: green
    let first = serde_json::json!([
        { "name": "api-gateway", "status": "green", "notes": "nominal" }
    ]);
    let (s1, b1) = post_concerns(app.clone(), first).await;
    assert_eq!(s1, StatusCode::OK, "first POST failed: {b1}");

    // Second write: same name, different status/notes
    let second = serde_json::json!([
        { "name": "api-gateway", "status": "red", "notes": "latency spike detected" }
    ]);
    let (s2, b2) = post_concerns(app.clone(), second).await;
    assert_eq!(s2, StatusCode::OK, "second POST failed: {b2}");

    // GET should return only one row with the latest values.
    let (get_status, get_body) = get_concerns(app).await;
    assert_eq!(get_status, StatusCode::OK, "GET failed: {get_body}");

    let listed = get_body["data"].as_array().expect("data should be array");
    assert_eq!(
        listed.len(),
        1,
        "should be exactly one concern after upsert"
    );
    assert_eq!(listed[0]["name"], "api-gateway");
    assert_eq!(
        listed[0]["status"], "red",
        "status should reflect latest upsert"
    );
    assert_eq!(listed[0]["notes"], "latency spike detected");
}

// ── test 3: GET returns empty array when no concerns exist ────────────────────

#[tokio::test]
#[serial]
async fn test_concerns_returns_empty() {
    // setup_pool() truncates all tables including standing_concerns, so the
    // table is empty when this test runs.
    let app = make_app().await;

    let (status, body) = get_concerns(app).await;
    assert_eq!(status, StatusCode::OK, "GET should return 200: {body}");

    let data = body["data"].as_array().expect("data should be an array");
    assert!(data.is_empty(), "expected empty array, got {data:?}");
}
