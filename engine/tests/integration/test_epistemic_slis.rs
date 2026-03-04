//! Integration tests for GET /admin/epistemic (covalence#88).
//!
//! Verifies that:
//! * The endpoint returns HTTP 200.
//! * The response body contains all six expected SLI keys.
//! * Each SLI has `value` (f64/number), `target` (f64/number), and `healthy`
//!   (bool) fields with valid values.
//! * On a clean (empty) database, the coverage/freshness/connectivity
//!   metrics default to healthy (no articles → trivially at 100 %).

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ── helper ────────────────────────────────────────────────────────────────────

/// GET /admin/epistemic and return the inner `data` object.
async fn fetch_epistemic(app: &axum::Router) -> serde_json::Value {
    let req = Request::builder()
        .method("GET")
        .uri("/admin/epistemic")
        .body(Body::empty())
        .expect("epistemic request builder");

    let resp = app.clone().oneshot(req).await.expect("epistemic oneshot");

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /admin/epistemic should return 200"
    );

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read epistemic body");

    let json: serde_json::Value =
        serde_json::from_slice(&bytes).expect("epistemic body is not JSON");

    json["data"].clone()
}

/// Assert that `field` inside `data` has all three required sub-keys
/// (`value`, `target`, `healthy`) with sensible types.
fn assert_sli(data: &serde_json::Value, field: &str) {
    let sli = &data[field];
    assert!(
        !sli.is_null(),
        "SLI field '{field}' is missing from response: {data}"
    );

    let value = &sli["value"];
    assert!(
        value.is_number(),
        "SLI '{field}.value' must be a number, got: {sli}"
    );
    let v = value.as_f64().expect("value as f64");
    assert!(
        v.is_finite() && v >= 0.0,
        "SLI '{field}.value' must be finite and non-negative, got: {v}"
    );

    let target = &sli["target"];
    assert!(
        target.is_number(),
        "SLI '{field}.target' must be a number, got: {sli}"
    );

    let healthy = &sli["healthy"];
    assert!(
        healthy.is_boolean(),
        "SLI '{field}.healthy' must be a bool, got: {sli}"
    );
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// All six SLI keys must be present and structurally valid on a fresh DB.
#[tokio::test]
#[serial]
async fn epistemic_all_six_keys_present() {
    let pool = setup_pool().await;
    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let data = fetch_epistemic(&app).await;

    // Each of the six Phase-1 SLIs must appear.
    for field in &[
        "embedding_coverage",
        "knowledge_freshness",
        "graph_connectivity",
        "confidence_health",
        "contention_rate",
        "queue_health",
    ] {
        assert_sli(&data, field);
    }
}

/// On an empty database the coverage/rate metrics must report healthy defaults:
/// - coverage/freshness/connectivity → 1.0 (trivially 100 % with 0 articles)
/// - contention_rate / queue_health  → 0.0 (no contentions, no failures)
#[tokio::test]
#[serial]
async fn epistemic_empty_db_defaults_healthy() {
    let pool = setup_pool().await;
    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let data = fetch_epistemic(&app).await;

    // Trivially-healthy coverage metrics.
    for field in &[
        "embedding_coverage",
        "knowledge_freshness",
        "graph_connectivity",
    ] {
        let v = data[field]["value"].as_f64().unwrap();
        assert!(
            (v - 1.0_f64).abs() < f64::EPSILON * 10.0,
            "expected {field}.value ≈ 1.0 on empty DB, got {v}"
        );
        assert!(
            data[field]["healthy"].as_bool().unwrap(),
            "expected {field}.healthy = true on empty DB"
        );
    }

    // Trivially-healthy rate metrics.
    for field in &["contention_rate", "queue_health"] {
        let v = data[field]["value"].as_f64().unwrap();
        assert!(
            v < 0.05_f64,
            "expected {field}.value < 0.05 on empty DB, got {v}"
        );
        assert!(
            data[field]["healthy"].as_bool().unwrap(),
            "expected {field}.healthy = true on empty DB"
        );
    }
}

/// After inserting one article, `embedding_coverage` must reflect the
/// missing embedding (value < 1.0, healthy = false at ≥ 0.98 target).
#[tokio::test]
#[serial]
async fn epistemic_embedding_coverage_tracks_missing_embeddings() {
    let pool = setup_pool().await;

    // Insert one article without an embedding.
    sqlx::query(
        "INSERT INTO covalence.nodes (id, node_type, status, title, content, metadata) \
         VALUES (gen_random_uuid(), 'article', 'active', 'SLI Test', 'content', '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("insert article");

    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let data = fetch_epistemic(&app).await;

    let cov = data["embedding_coverage"]["value"].as_f64().unwrap();
    assert!(
        cov < 1.0,
        "embedding_coverage should be < 1.0 when an article lacks an embedding; got {cov}"
    );
    assert!(
        !data["embedding_coverage"]["healthy"].as_bool().unwrap(),
        "embedding_coverage should be unhealthy when articles are missing embeddings"
    );
}

/// `contention_rate` must be positive when a detected contention exists.
#[tokio::test]
#[serial]
async fn epistemic_contention_rate_nonzero_with_active_contention() {
    let pool = setup_pool().await;

    // Insert two nodes (article + source) and a detected contention.
    let article_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes (id, node_type, status, title, content, metadata) \
         VALUES (gen_random_uuid(), 'article', 'active', 'Art', 'body', '{}'::jsonb) \
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("insert article");

    let source_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes (id, node_type, status, title, content, metadata) \
         VALUES (gen_random_uuid(), 'source', 'active', 'Src', 'body', '{}'::jsonb) \
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("insert source");

    sqlx::query(
        "INSERT INTO covalence.contentions \
         (node_id, source_node_id, type, description, severity, status, materiality) \
         VALUES ($1, $2, 'contradiction', 'test', 'high', 'detected', 0.9)",
    )
    .bind(article_id)
    .bind(source_id)
    .execute(&pool)
    .await
    .expect("insert contention");

    let state = make_test_state(pool).await;
    let app = routes::router().with_state(state);

    let data = fetch_epistemic(&app).await;

    let rate = data["contention_rate"]["value"].as_f64().unwrap();
    assert!(
        rate > 0.0,
        "contention_rate should be > 0 with an active contention; got {rate}"
    );
}
