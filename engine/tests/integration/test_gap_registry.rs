//! Integration tests for the persistent gap registry (covalence#100).
//!
//! Three tests:
//! 1. `gap_log_written_after_search` — a POST /search call causes a row to be
//!    written to `covalence.gap_log`.
//! 2. `compute_gaps_populates_registry` — after inserting ≥3 gap_log rows for
//!    the same query, POST /admin/maintenance with `compute_gaps=true` upserts
//!    a row into `gap_registry`.
//! 3. `get_admin_gaps_descending_order` — GET /admin/gaps returns topics in
//!    gap_score descending order.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn post_search(app: &axum::Router, query: &str) -> serde_json::Value {
    let body = serde_json::json!({ "query": query, "limit": 5 }).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/search")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("build search request");
    let resp = app.clone().oneshot(req).await.expect("search oneshot");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read search body");
    serde_json::from_slice(&bytes).expect("search response JSON")
}

async fn post_maintenance(app: &axum::Router, payload: serde_json::Value) -> serde_json::Value {
    let req = Request::builder()
        .method("POST")
        .uri("/admin/maintenance")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("build maintenance request");
    let resp = app.clone().oneshot(req).await.expect("maintenance oneshot");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read maintenance body");
    serde_json::from_slice(&bytes).expect("maintenance response JSON")
}

async fn get_admin_gaps(app: &axum::Router) -> serde_json::Value {
    let req = Request::builder()
        .method("GET")
        .uri("/admin/gaps")
        .body(Body::empty())
        .expect("build gaps request");
    let resp = app.clone().oneshot(req).await.expect("gaps oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /admin/gaps must return 200"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read gaps body");
    serde_json::from_slice(&bytes).expect("gaps response JSON")
}

// ─── Test 1 ───────────────────────────────────────────────────────────────────

/// A POST /search call must result in a row being written to `gap_log`.
///
/// Because the INSERT is fire-and-forget (spawned), we retry with a brief
/// backoff to allow the background task to complete before asserting.
#[tokio::test]
#[serial]
async fn gap_log_written_after_search() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let unique_query = format!("gap-log-test-{}", uuid::Uuid::new_v4());

    // POST /search — knowledge base is empty so results = 0, but the log still fires.
    let _resp = post_search(&app, &unique_query).await;

    // Give the fire-and-forget task up to 2 s to land.
    let mut count: i64 = 0;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM covalence.gap_log WHERE query = $1")
                .bind(&unique_query)
                .fetch_one(&pool)
                .await
                .expect("count gap_log");
        if count > 0 {
            break;
        }
    }

    assert_eq!(
        count, 1,
        "gap_log must have exactly one row for the search query"
    );
}

// ─── Test 2 ───────────────────────────────────────────────────────────────────

/// After inserting ≥3 gap_log rows for the same query, running
/// compute_gaps=true must upsert a row into gap_registry.
#[tokio::test]
#[serial]
async fn compute_gaps_populates_registry() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let topic = format!("persistent-gap-topic-{}", uuid::Uuid::new_v4());

    // Insert 4 gap_log rows directly — all with low top_score (simulating misses).
    for _ in 0..4 {
        sqlx::query(
            "INSERT INTO covalence.gap_log (query, top_score, result_count, namespace) \
             VALUES ($1, 0.05, 1, 'default')",
        )
        .bind(&topic)
        .execute(&pool)
        .await
        .expect("insert gap_log row");
    }

    // Trigger gap computation via maintenance endpoint.
    let resp = post_maintenance(&app, serde_json::json!({ "compute_gaps": true })).await;
    let actions = &resp["data"]["actions_taken"];
    assert!(
        actions.to_string().contains("compute_gaps"),
        "maintenance response must mention compute_gaps; got: {resp}"
    );

    // Verify gap_registry row was created.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM covalence.gap_registry WHERE topic = $1")
            .bind(&topic)
            .fetch_one(&pool)
            .await
            .expect("count gap_registry");

    assert_eq!(count, 1, "gap_registry must contain one row for the topic");

    // Verify gap_score is reasonable (non-null, > 0).
    let gap_score: Option<f64> =
        sqlx::query_scalar("SELECT gap_score FROM covalence.gap_registry WHERE topic = $1")
            .bind(&topic)
            .fetch_one(&pool)
            .await
            .expect("fetch gap_score");

    let score = gap_score.expect("gap_score must not be NULL");
    assert!(
        score > 0.0 && score <= 1.0,
        "gap_score must be in (0, 1]; got {score}"
    );
}

// ─── Test 3 ───────────────────────────────────────────────────────────────────

/// GET /admin/gaps must return topics in gap_score descending order.
#[tokio::test]
#[serial]
async fn get_admin_gaps_descending_order() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Insert two gap_registry rows with known gap_scores.
    sqlx::query(
        "INSERT INTO covalence.gap_registry \
         (id, topic, namespace, query_count, avg_top_score, gap_score, status) \
         VALUES \
         (gen_random_uuid(), 'low-gap-topic',  'default', 5, 0.80, 0.20, 'open'), \
         (gen_random_uuid(), 'high-gap-topic', 'default', 5, 0.10, 0.90, 'open')",
    )
    .execute(&pool)
    .await
    .expect("insert gap_registry rows");

    let resp = get_admin_gaps(&app).await;
    let data = &resp["data"];
    assert!(
        data.is_array(),
        "GET /admin/gaps data must be an array; got: {resp}"
    );

    let items = data.as_array().expect("data array");
    assert!(items.len() >= 2, "must have at least 2 gap entries");

    // Find our two inserted topics.
    let high_pos = items
        .iter()
        .position(|v| v["topic"].as_str() == Some("high-gap-topic"))
        .expect("high-gap-topic not found");
    let low_pos = items
        .iter()
        .position(|v| v["topic"].as_str() == Some("low-gap-topic"))
        .expect("low-gap-topic not found");

    assert!(
        high_pos < low_pos,
        "high-gap-topic (score=0.90) must appear before low-gap-topic (score=0.20) \
         when sorted by gap_score DESC; positions: high={high_pos}, low={low_pos}"
    );
}
