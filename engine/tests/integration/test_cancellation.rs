//! Integration test for CancellationToken + JoinSet graceful drain (covalence#89).
//!
//! Verifies that `run_with_token` completes in-flight tasks before returning
//! when the token is cancelled — no task is left in the `processing` state.

use std::sync::Arc;
use std::time::Duration;

use serial_test::serial;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use covalence_engine::worker;
use covalence_engine::worker::llm::LlmClient;

use super::helpers::{MockLlmClient, TestFixture};

/// Verify that the queue processor completes an in-flight task when
/// cancellation is requested — the graceful-drain pattern.
///
/// Scenario:
/// 1. Insert a source node and enqueue an `embed` task for it.
/// 2. Create a `CancellationToken`; schedule cancellation after a short delay
///    so the worker has time to claim and start the task.
/// 3. Run `run_with_token` under a timeout; assert it returns cleanly.
/// 4. Assert the task is **not** stuck in `processing` state — the worker must
///    either complete it (`complete`) or leave it re-queued (`pending`/`failed`),
///    but never leave it in `processing` after the worker exits.
#[tokio::test]
#[serial]
async fn test_worker_graceful_drain_on_cancellation() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // ── 1. Insert a source and enqueue an embed task ──────────────────────
    let src = fix
        .insert_source(
            "Cancellation Test Source",
            "Short content for the graceful-drain integration test.",
        )
        .await;

    let task_uuid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
         (id, task_type, node_id, payload, status, priority) \
         VALUES ($1, 'embed', $2, '{}', 'pending', 5)",
    )
    .bind(task_uuid)
    .bind(src)
    .execute(&fix.pool)
    .await
    .expect("queue task insert should succeed");

    // ── 2. Cancel after a short delay ────────────────────────────────────
    let token = CancellationToken::new();
    let cancel_token = token.clone();
    tokio::spawn(async move {
        // Give the worker a chance to claim and process the task before
        // we cancel it.
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel_token.cancel();
    });

    // ── 3. Run the worker; it must return cleanly within the timeout ──────
    let pool_clone = fix.pool.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        worker::run_with_token(pool_clone, llm, token),
    )
    .await;

    assert!(
        result.is_ok(),
        "run_with_token should return within timeout after cancellation"
    );

    // ── 4. Assert the task is not stuck in 'processing' ──────────────────
    let status: String =
        sqlx::query_scalar("SELECT status FROM covalence.slow_path_queue WHERE id = $1")
            .bind(task_uuid)
            .fetch_one(&fix.pool)
            .await
            .expect("queue row should still exist");

    assert_ne!(
        status, "processing",
        "task must not be left in 'processing' state after graceful drain (got: {status})"
    );

    // The task should be in a terminal or retryable state, not still running.
    assert!(
        matches!(status.as_str(), "complete" | "pending" | "failed"),
        "unexpected task status after cancellation: {status}"
    );

    fix.cleanup().await;
}

/// Verify that with no tasks queued, cancellation causes the worker to return
/// immediately (within one poll interval) without hanging.
#[tokio::test]
#[serial]
async fn test_worker_exits_cleanly_with_no_tasks() {
    let fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let token = CancellationToken::new();
    let cancel_token = token.clone();

    // Cancel almost immediately — no tasks enqueued, so the worker should
    // notice the cancellation during its first sleep and return.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_token.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        worker::run_with_token(fix.pool.clone(), llm, token),
    )
    .await;

    assert!(
        result.is_ok(),
        "run_with_token should return within timeout when no tasks are present"
    );

    fix.cleanup().await;
}
