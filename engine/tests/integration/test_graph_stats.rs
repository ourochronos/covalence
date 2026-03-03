//! Integration tests for GET /admin/graph/stats (covalence#49).
//!
//! Verifies that:
//! * The route returns HTTP 200 with a JSON body containing `data.node_count`
//!   and `data.edge_count`.
//! * Creating an edge via POST /edges increments the edge count.
//! * Deleting an edge via DELETE /edges/{id} decrements the edge count.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ── helpers ───────────────────────────────────────────────────────────────────

/// POST a source with the given content and return its node UUID string.
async fn create_test_source(app: &axum::Router, content: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/sources")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "content": content,
                "source_type": "document",
                "title": content
            })
            .to_string(),
        ))
        .expect("source request builder failed");

    let resp = app
        .clone()
        .oneshot(req)
        .await
        .expect("source create oneshot failed");

    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "source creation should return 201"
    );

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("failed to read source body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("source body is not JSON");

    json["data"]["id"]
        .as_str()
        .expect("source id missing")
        .to_string()
}

/// GET /admin/graph/stats and return the `data` object.
async fn fetch_graph_stats(app: &axum::Router) -> serde_json::Value {
    let req = Request::builder()
        .method("GET")
        .uri("/admin/graph/stats")
        .body(Body::empty())
        .expect("stats request builder failed");

    let resp = app
        .clone()
        .oneshot(req)
        .await
        .expect("graph stats oneshot failed");

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /admin/graph/stats should return 200"
    );

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("failed to read stats body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("stats body is not JSON");

    json["data"].clone()
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Fresh (truncated) DB: the endpoint must return numeric node_count and
/// edge_count (both 0 after a clean truncate).
#[tokio::test]
#[serial]
async fn graph_stats_returns_counts() {
    let pool = setup_pool().await;
    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let data = fetch_graph_stats(&app).await;

    assert!(
        data["node_count"].is_number(),
        "node_count should be a number, got: {data}"
    );
    assert!(
        data["edge_count"].is_number(),
        "edge_count should be a number, got: {data}"
    );
}

/// After POST /edges the in-memory graph edge count must increase.
#[tokio::test]
#[serial]
async fn graph_stats_reflect_edge_creation() {
    let pool = setup_pool().await;
    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    // Create two source nodes so we have valid UUIDs to link.
    let s1 = create_test_source(&app, "graph stats source one").await;
    let s2 = create_test_source(&app, "graph stats source two").await;

    let before = fetch_graph_stats(&app).await;
    let edge_before = before["edge_count"].as_i64().unwrap_or(0);

    // Create an edge.
    let edge_req = Request::builder()
        .method("POST")
        .uri("/edges")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "from_node_id": s1,
                "to_node_id":   s2,
                "label":        "RELATES_TO"
            })
            .to_string(),
        ))
        .expect("edge create request builder failed");

    let edge_resp = app
        .clone()
        .oneshot(edge_req)
        .await
        .expect("edge create oneshot failed");

    assert_eq!(
        edge_resp.status(),
        StatusCode::CREATED,
        "POST /edges should return 201"
    );

    let after = fetch_graph_stats(&app).await;
    let edge_after = after["edge_count"].as_i64().unwrap_or(0);

    assert!(
        edge_after > edge_before,
        "edge_count should increase after edge creation (before={edge_before}, after={edge_after})"
    );
}

/// After DELETE /edges/{id} the in-memory graph edge count must decrease.
#[tokio::test]
#[serial]
async fn graph_stats_reflect_edge_deletion() {
    let pool = setup_pool().await;
    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    // Create two source nodes.
    let s1 = create_test_source(&app, "del source one").await;
    let s2 = create_test_source(&app, "del source two").await;

    // Create an edge and capture its id.
    let edge_req = Request::builder()
        .method("POST")
        .uri("/edges")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "from_node_id": s1,
                "to_node_id":   s2,
                "label":        "RELATES_TO"
            })
            .to_string(),
        ))
        .expect("edge create request builder failed");

    let edge_resp = app
        .clone()
        .oneshot(edge_req)
        .await
        .expect("edge create oneshot failed");

    assert_eq!(edge_resp.status(), StatusCode::CREATED);

    let edge_bytes = axum::body::to_bytes(edge_resp.into_body(), usize::MAX)
        .await
        .expect("failed to read edge body");
    let edge_json: serde_json::Value =
        serde_json::from_slice(&edge_bytes).expect("edge body is not JSON");
    let edge_id = edge_json["data"]["id"]
        .as_str()
        .expect("edge id missing")
        .to_string();

    let before = fetch_graph_stats(&app).await;
    let edge_before = before["edge_count"].as_i64().unwrap_or(0);

    // Delete the edge.
    let del_req = Request::builder()
        .method("DELETE")
        .uri(&format!("/edges/{edge_id}"))
        .body(Body::empty())
        .expect("edge delete request builder failed");

    let del_resp = app
        .clone()
        .oneshot(del_req)
        .await
        .expect("edge delete oneshot failed");

    assert_eq!(del_resp.status(), StatusCode::NO_CONTENT);

    let after = fetch_graph_stats(&app).await;
    let edge_after = after["edge_count"].as_i64().unwrap_or(0);

    assert!(
        edge_after < edge_before,
        "edge_count should decrease after edge deletion (before={edge_before}, after={edge_after})"
    );
}
