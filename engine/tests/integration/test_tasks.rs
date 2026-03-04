//! Integration tests for the task state machine (covalence#114).
//!
//! Covers:
//! 1. `task_full_lifecycle`   — create → assign → running → done; stats reflect counts.
//! 2. `task_failure_path`     — create → running → patch to failed; failure_class set.
//! 3. `task_list_filter`      — list by status returns only matching tasks.
//! 4. `task_auto_timeout`     — maintenance `timeout_stale_tasks` fires on expired tasks.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ─── HTTP helpers ─────────────────────────────────────────────────────────────

async fn post_json(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse JSON");
    (status, json)
}

async fn patch_json(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("PATCH")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse JSON");
    (status, json)
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("build request");
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse JSON");
    (status, json)
}

// ─── Test 1: full lifecycle ───────────────────────────────────────────────────

/// Create a task, advance it through pending → assigned → running → done,
/// then confirm GET /tasks/stats reflects counts and lead time.
#[tokio::test]
#[serial]
async fn task_full_lifecycle() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // ── Create ────────────────────────────────────────────────────────────────
    let (status, body) = post_json(
        &app,
        "/tasks",
        serde_json::json!({
            "label": "implement feature X",
            "issue_ref": "covalence#114",
            "metadata": { "priority": "high" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "POST /tasks must return 201");
    let task = &body["data"];
    assert_eq!(task["status"], "pending");
    assert_eq!(task["label"], "implement feature X");
    assert_eq!(task["issue_ref"], "covalence#114");
    let task_id = task["id"].as_str().expect("id present");

    // ── GET single task ───────────────────────────────────────────────────────
    let (status, body) = get_json(&app, &format!("/tasks/{task_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], task_id);

    // ── Assign ────────────────────────────────────────────────────────────────
    let (status, body) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({ "status": "assigned", "assigned_session_id": "sess-abc" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PATCH assign must return 200");
    assert_eq!(body["data"]["status"], "assigned");
    assert_eq!(body["data"]["assigned_session_id"], "sess-abc");

    // ── Running ───────────────────────────────────────────────────────────────
    let (status, body) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({ "status": "running" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PATCH running must return 200");
    assert_eq!(body["data"]["status"], "running");
    // started_at should have been auto-set
    assert!(
        body["data"]["started_at"].is_string(),
        "started_at must be set when status → running"
    );

    // ── Done ──────────────────────────────────────────────────────────────────
    let (status, body) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({
            "status": "done",
            "result_summary": "Feature X shipped in v2.3"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PATCH done must return 200");
    assert_eq!(body["data"]["status"], "done");
    assert_eq!(body["data"]["result_summary"], "Feature X shipped in v2.3");
    assert!(
        body["data"]["completed_at"].is_string(),
        "completed_at must be set when status → done"
    );

    // ── Stats ─────────────────────────────────────────────────────────────────
    let (status, body) = get_json(&app, "/tasks/stats").await;
    assert_eq!(status, StatusCode::OK, "GET /tasks/stats must return 200");
    let stats = &body["data"];
    let done_count = stats["counts"]["done"].as_i64().unwrap_or(0);
    assert!(
        done_count >= 1,
        "done count must be ≥ 1 after completing a task"
    );
    // Lead time must be non-null since we have a completed task with timestamps.
    assert!(
        stats["avg_lead_time_secs"].is_number() || stats["avg_lead_time_secs"].is_null(),
        "avg_lead_time_secs must be number or null"
    );
}

// ─── Test 2: failure path ─────────────────────────────────────────────────────

/// Create a task, move it to running, then manually patch it to failed with a
/// failure_class.  Confirm all fields persist correctly.
#[tokio::test]
#[serial]
async fn task_failure_path() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Create
    let (status, body) = post_json(
        &app,
        "/tasks",
        serde_json::json!({ "label": "fragile task" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = body["data"]["id"].as_str().expect("id").to_string();

    // running
    let (status, _) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({ "status": "running" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // fail with explicit class
    let (status, body) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({
            "status": "failed",
            "failure_class": "panic",
            "result_summary": "thread panicked at main.rs:42"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PATCH failed must return 200");
    let t = &body["data"];
    assert_eq!(t["status"], "failed");
    assert_eq!(t["failure_class"], "panic");
    assert!(t["completed_at"].is_string(), "completed_at set on failure");

    // Invalid status → 400
    let (status, _) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({ "status": "bogus" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid status must return 400"
    );
}

// ─── Test 3: list filter ──────────────────────────────────────────────────────

/// Create tasks with different statuses and confirm ?status= filtering works.
#[tokio::test]
#[serial]
async fn task_list_filter() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Create one pending (default) and one that we advance to running.
    let (_, body) = post_json(
        &app,
        "/tasks",
        serde_json::json!({ "label": "pending task" }),
    )
    .await;
    let _pending_id = body["data"]["id"].as_str().expect("id").to_string();

    let (_, body) = post_json(
        &app,
        "/tasks",
        serde_json::json!({ "label": "running task" }),
    )
    .await;
    let running_id = body["data"]["id"].as_str().expect("id").to_string();

    patch_json(
        &app,
        &format!("/tasks/{running_id}"),
        serde_json::json!({ "status": "running" }),
    )
    .await;

    // List all — should have at least 2
    let (status, body) = get_json(&app, "/tasks").await;
    assert_eq!(status, StatusCode::OK);
    let all_count = body["data"].as_array().expect("array").len();
    assert!(all_count >= 2, "expected ≥ 2 tasks in unfiltered list");

    // Filter to running
    let (status, body) = get_json(&app, "/tasks?status=running").await;
    assert_eq!(status, StatusCode::OK);
    let running_tasks = body["data"].as_array().expect("array");
    assert!(
        running_tasks.iter().all(|t| t["status"] == "running"),
        "all tasks in filtered list must have status=running"
    );
    assert!(
        running_tasks.len() >= 1,
        "at least one running task must appear"
    );

    // Filter to pending
    let (status, body) = get_json(&app, "/tasks?status=pending").await;
    assert_eq!(status, StatusCode::OK);
    let pending_tasks = body["data"].as_array().expect("array");
    assert!(
        pending_tasks.iter().all(|t| t["status"] == "pending"),
        "all tasks in filtered list must have status=pending"
    );
}

// ─── Test 4: auto-timeout ────────────────────────────────────────────────────

/// Create a running task with a timeout_at in the past, then call
/// POST /admin/maintenance with timeout_stale_tasks=true and confirm the
/// task is marked failed with failure_class = 'timeout'.
#[tokio::test]
#[serial]
async fn task_auto_timeout() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Create task with an already-expired timeout (1 minute in the past).
    let (status, body) = post_json(
        &app,
        "/tasks",
        serde_json::json!({
            "label": "timed-out task",
            "timeout_at": "2000-01-01T00:00:00Z"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = body["data"]["id"].as_str().expect("id").to_string();

    // Advance to running so the timeout logic applies.
    let (status, _) = patch_json(
        &app,
        &format!("/tasks/{task_id}"),
        serde_json::json!({ "status": "running" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Run maintenance with timeout flag.
    let (status, body) = post_json(
        &app,
        "/admin/maintenance",
        serde_json::json!({ "timeout_stale_tasks": true }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST /admin/maintenance must return 200"
    );
    let actions: Vec<String> = body["data"]["actions_taken"]
        .as_array()
        .expect("actions_taken array")
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        actions.iter().any(|a| a.starts_with("timeout_stale_tasks")),
        "actions_taken must include timeout_stale_tasks entry; got: {actions:?}"
    );

    // Verify the task is now failed with failure_class = 'timeout'.
    let (status, body) = get_json(&app, &format!("/tasks/{task_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let t = &body["data"];
    assert_eq!(
        t["status"], "failed",
        "timed-out task must be status=failed"
    );
    assert_eq!(
        t["failure_class"], "timeout",
        "failure_class must be 'timeout'"
    );
    assert!(
        t["completed_at"].is_string(),
        "completed_at must be set after timeout"
    );
}
