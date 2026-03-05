//! Integration test for the slow-path worker's concurrency cap (covalence#149).
//!
//! `MAX_CONCURRENT_LLM_TASKS = 6` is enforced inside `run_with_token` via a
//! `JoinSet` size check.  This test verifies that the DB never shows more than
//! 6 rows in `status = 'processing'` simultaneously, even when 10 `embed`
//! tasks are queued.
//!
//! # Strategy
//!
//! A `ParkingLlmClient` keeps every `llm.embed()` call suspended by spinning
//! on an `AtomicBool` gate.  With all tasks blocked in the LLM layer the
//! JoinSet fills up, letting us observe the steady-state processing count in
//! the DB before the gate is released.
//!
//! Timing (worst case, 2-second `POLL_INTERVAL`):
//! - t= 0 s: worker spawns, claims task 1
//! - t= 2 s: claims task 2
//! - t=10 s: claims task 6 (JoinSet at cap)
//! - t=12 s: cap reached; no further claims
//! - t=20 s: observation window closes, gate released, worker cancelled

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serial_test::serial;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use covalence_engine::worker;
use covalence_engine::worker::llm::LlmClient;

use super::helpers::TestFixture;

// ─── Parking mock LLM ────────────────────────────────────────────────────────

/// LLM client whose `embed()` spins on an `AtomicBool` gate.
///
/// While `parked` is `true` the call yields every 50 ms so that the tokio
/// scheduler can continue running the worker's poll loop in the same thread
/// pool.  When `parked` is set to `false` the call returns a fixed unit
/// vector immediately.
struct ParkingLlmClient {
    parked: Arc<AtomicBool>,
}

#[async_trait]
impl LlmClient for ParkingLlmClient {
    async fn complete(&self, _prompt: &str, _max_tokens: u32) -> anyhow::Result<String> {
        // Not expected to be called in the embed-only scenario.
        Ok(
            r#"{"title":"t","content":"c","epistemic_type":"semantic","source_relationships":[]}"#
                .to_string(),
        )
    }

    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        // Yield until the test releases the gate.
        while self.parked.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Return a unit vector (all equal components, L2-normalised).
        let dims = 1536usize;
        let val = 1.0_f32 / (dims as f32).sqrt();
        Ok(vec![val; dims])
    }
}

// ─── Test ─────────────────────────────────────────────────────────────────────

/// The slow-path worker must not spawn more than `MAX_CONCURRENT_LLM_TASKS`
/// simultaneous LLM tasks even when more tasks are pending in the queue.
///
/// We seed **10** `embed` tasks (> 6 cap), park the LLM so every in-flight
/// call suspends, observe the DB `processing` count every 200 ms for 20 s,
/// then assert the maximum observed count never exceeded 6.
#[tokio::test]
#[serial]
async fn test_worker_respects_max_concurrent_llm_tasks() {
    let mut fix = TestFixture::new().await;

    // ── 1. Seed 10 source nodes and one `embed` task each ─────────────────
    // Content is deliberately short (<700 chars) so the worker takes the
    // simple embed path (direct llm.embed call) rather than the tree_index
    // path.
    for i in 0..10u32 {
        let node_id = fix
            .insert_source(
                &format!("Concurrency Cap Source {i}"),
                &format!("Short content item {i} for the concurrency cap regression test."),
            )
            .await;

        sqlx::query(
            "INSERT INTO covalence.slow_path_queue \
             (id, task_type, node_id, payload, status, priority) \
             VALUES ($1, 'embed', $2, '{}', 'pending', 5)",
        )
        .bind(Uuid::new_v4())
        .bind(node_id)
        .execute(&fix.pool)
        .await
        .expect("seed embed task");
    }

    // ── 2. Build the parking LLM and launch the worker ────────────────────
    let parked = Arc::new(AtomicBool::new(true));
    let llm: Arc<dyn LlmClient> = Arc::new(ParkingLlmClient {
        parked: parked.clone(),
    });

    let token = CancellationToken::new();
    let cancel = token.clone();
    let pool_bg = fix.pool.clone();

    let worker_handle = tokio::spawn(async move {
        worker::run_with_token(pool_bg, llm, cancel).await;
    });

    // ── 3. Observe the DB processing count every 200 ms for 20 s ──────────
    //
    // With POLL_INTERVAL = 2 s, the cap of 6 is reached after ~10 s (one
    // task claimed per cycle).  The 20-second window is generous enough to
    // catch the steady-state without being flaky on slow CI machines.
    let mut max_concurrent: i64 = 0;
    for _ in 0..100u32 {
        tokio::time::sleep(Duration::from_millis(200)).await;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) \
             FROM covalence.slow_path_queue \
             WHERE status = 'processing'",
        )
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);

        if count > max_concurrent {
            max_concurrent = count;
        }
    }

    // ── 4. Assert the cap was never violated ──────────────────────────────
    // Note: this assertion intentionally runs BEFORE we release the gate so
    // that the observed count reflects the true steady-state during the
    // parked period rather than the transient completion storm.
    assert!(
        max_concurrent <= 6,
        "worker violated MAX_CONCURRENT_LLM_TASKS=6: \
         observed {max_concurrent} simultaneous 'processing' rows (cap is 6)"
    );

    // ── 5. Release gate, cancel worker, and drain cleanly ─────────────────
    parked.store(false, Ordering::Release);
    token.cancel();

    // Allow the worker's graceful drain to finish so no 'processing' rows
    // remain for the next test's truncation step.
    tokio::time::timeout(Duration::from_secs(10), worker_handle)
        .await
        .expect("worker should drain within timeout")
        .expect("worker task should not panic");

    fix.cleanup().await;
}
