//! Integration tests for the `process_queue` block inside
//! `AdminService::maintenance()` (covalence#149).
//!
//! The cleanup runs three sequential passes:
//!
//! 0. **Stale-processing timeout** — rows stuck in `status='processing'` for
//!    more than 10 minutes are flipped to `'failed'`.
//!
//! A. **Orphaned failed embed cleanup** — failed `embed` tasks whose target
//!    node *already has* an embedding (later satisfied by an embed-all run)
//!    are deleted when they are older than 1 hour (`COALESCE(completed_at,
//!    started_at, created_at) < now() - interval '1 hour'`).
//!
//! B. **Null-node stale cleanup** — failed tasks with a `NULL node_id` older
//!    than 24 hours are deleted (they reference no valid target and can never
//!    be retried successfully).
//!
//! Each test seeds representative rows, calls `maintenance({ process_queue:
//! true })`, then asserts that stale rows were removed while healthy rows
//! were left untouched.
//!
//! # Regression note
//! The cleanup SQL was broken once because it referenced a nonexistent
//! `updated_at` column on `slow_path_queue`.  These tests would have caught
//! that regression immediately.

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::admin_service::{AdminService, MaintenanceRequest};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Insert a row into `slow_path_queue` with an explicit status and an optional
/// `node_id`.  Returns the generated UUID.
async fn insert_queue_row(
    fix: &TestFixture,
    task_type: &str,
    node_id: Option<Uuid>,
    status: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
         (id, task_type, node_id, payload, status, priority) \
         VALUES ($1, $2, $3, '{}', $4, 0)",
    )
    .bind(id)
    .bind(task_type)
    .bind(node_id)
    .bind(status)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_queue_row({task_type}, {status}) failed: {e}"));
    id
}

/// Backdates `COALESCE(completed_at, started_at, created_at)` for a queue row
/// by setting `completed_at` to `now() - interval`.
///
/// The cleanup SQL uses `COALESCE(completed_at, started_at, created_at)`, so
/// setting `completed_at` ensures the row appears old regardless of the other
/// columns.
async fn backdate_queue_row(fix: &TestFixture, task_id: Uuid, interval: &str) {
    sqlx::query(&format!(
        "UPDATE covalence.slow_path_queue \
         SET completed_at = now() - interval '{interval}' \
         WHERE id = $1"
    ))
    .bind(task_id)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("backdate_queue_row({task_id}, {interval}) failed: {e}"));
}

/// Set `started_at` on a queue row to a past time (simulates a stale
/// processing job) without touching `completed_at`.
async fn backdate_started_at(fix: &TestFixture, task_id: Uuid, interval: &str) {
    sqlx::query(&format!(
        "UPDATE covalence.slow_path_queue \
         SET started_at = now() - interval '{interval}' \
         WHERE id = $1"
    ))
    .bind(task_id)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("backdate_started_at({task_id}, {interval}) failed: {e}"));
}

/// Return the current `status` for a queue row, or `None` if the row is gone.
async fn queue_status(fix: &TestFixture, task_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM covalence.slow_path_queue WHERE id = $1")
        .bind(task_id)
        .fetch_optional(&fix.pool)
        .await
        .unwrap_or(None)
}

/// Return `true` if a queue row with the given `id` still exists.
async fn queue_row_exists(fix: &TestFixture, task_id: Uuid) -> bool {
    queue_status(fix, task_id).await.is_some()
}

// ─── Pass A: orphaned failed embed tasks ─────────────────────────────────────

/// Stale failed `embed` tasks for nodes that already have embeddings are
/// deleted by Pass A.
///
/// Scenario:
/// - Node A has an embedding in `node_embeddings`.
/// - Failed embed task for Node A, completed 2 hours ago → **should be deleted**.
/// - Failed embed task for Node A, completed 30 minutes ago → should NOT be
///   deleted (too recent).
/// - Failed embed task for Node B (no embedding), completed 2 hours ago →
///   should NOT be deleted (embedding still missing).
/// - Non-failed (`'pending'`) embed task for Node A, 2 hours old → should NOT
///   be deleted (only failed rows are cleaned up).
#[tokio::test]
#[serial]
async fn test_process_queue_pass_a_deletes_orphaned_embed_failures() {
    let mut fix = TestFixture::new().await;

    // ── Seed nodes ────────────────────────────────────────────────────────
    let node_with_emb = fix
        .insert_source("Node With Embedding", "some content A")
        .await;
    let node_without_emb = fix
        .insert_source("Node Without Embedding", "some content B")
        .await;

    // Give node_with_emb an embedding row.
    fix.insert_embedding(node_with_emb).await;

    // ── Seed queue rows ───────────────────────────────────────────────────

    // Row 1: stale failed embed for embedded node → SHOULD be deleted
    let stale_with_emb = insert_queue_row(&fix, "embed", Some(node_with_emb), "failed").await;
    backdate_queue_row(&fix, stale_with_emb, "2 hours").await;

    // Row 2: fresh failed embed for embedded node → should NOT be deleted
    let fresh_with_emb = insert_queue_row(&fix, "embed", Some(node_with_emb), "failed").await;
    // No backdating: completed_at is NULL, so COALESCE falls back to
    // created_at which is `now()` — well within the 1-hour window.

    // Row 3: stale failed embed for node WITHOUT embedding → should NOT be deleted
    let stale_no_emb = insert_queue_row(&fix, "embed", Some(node_without_emb), "failed").await;
    backdate_queue_row(&fix, stale_no_emb, "3 hours").await;

    // Row 4: stale but pending (not failed) for embedded node → should NOT be deleted
    let pending_with_emb = insert_queue_row(&fix, "embed", Some(node_with_emb), "pending").await;
    backdate_queue_row(&fix, pending_with_emb, "2 hours").await;

    // ── Run maintenance ───────────────────────────────────────────────────
    let svc = AdminService::new(fix.pool.clone());
    let result = svc
        .maintenance(MaintenanceRequest {
            process_queue: Some(true),
            ..Default::default()
        })
        .await
        .expect("maintenance should succeed");

    // Verify the action log mentions the Pass A cleanup.
    let pass_a_action = result
        .actions_taken
        .iter()
        .find(|a| a.contains("stale embed"))
        .cloned();
    assert!(
        pass_a_action.is_some(),
        "actions_taken must include the Pass A embed-cleanup entry; got: {:?}",
        result.actions_taken
    );

    // ── Assert outcomes ───────────────────────────────────────────────────

    // Row 1 must be gone: stale failed embed for embedded node.
    assert!(
        !queue_row_exists(&fix, stale_with_emb).await,
        "stale failed embed task for a node that now has an embedding must be deleted (Pass A)"
    );

    // Row 2 must still exist: too fresh.
    assert!(
        queue_row_exists(&fix, fresh_with_emb).await,
        "fresh failed embed task (< 1 hour old) must NOT be deleted by Pass A"
    );

    // Row 3 must still exist: node has no embedding.
    assert!(
        queue_row_exists(&fix, stale_no_emb).await,
        "stale failed embed task for a node WITHOUT embedding must NOT be deleted by Pass A"
    );

    // Row 4 must still exist: not a failed row.
    assert!(
        queue_row_exists(&fix, pending_with_emb).await,
        "pending (non-failed) embed task must NOT be deleted by Pass A"
    );

    fix.cleanup().await;
}

// ─── Pass B: null-node stale failed tasks ────────────────────────────────────

/// Stale failed tasks with `node_id IS NULL` older than 24 hours are deleted
/// by Pass B.
///
/// Scenario:
/// - Failed compile task, null node_id, 2 days old → **should be deleted**.
/// - Failed compile task, null node_id, 12 hours old → should NOT be deleted
///   (too recent).
/// - Failed compile task, non-null node_id, 2 days old → should NOT be deleted
///   (only null-node rows are targeted).
/// - Pending compile task, null node_id, 2 days old → should NOT be deleted
///   (only failed rows are targeted).
#[tokio::test]
#[serial]
async fn test_process_queue_pass_b_deletes_stale_null_node_failures() {
    let mut fix = TestFixture::new().await;

    // We need at least one node for the non-null-node row.
    let some_node = fix
        .insert_source("Stub Node For Pass B", "stub content")
        .await;

    // ── Seed queue rows ───────────────────────────────────────────────────

    // Row 1: old null-node failed task → SHOULD be deleted
    let old_null_node = insert_queue_row(&fix, "compile", None, "failed").await;
    backdate_queue_row(&fix, old_null_node, "48 hours").await;

    // Row 2: fresh null-node failed task → should NOT be deleted (< 24 h)
    let fresh_null_node = insert_queue_row(&fix, "compile", None, "failed").await;
    // No backdating: COALESCE will use created_at = now()

    // Row 3: old non-null-node failed task → should NOT be deleted (has node_id)
    let old_with_node = insert_queue_row(&fix, "compile", Some(some_node), "failed").await;
    backdate_queue_row(&fix, old_with_node, "48 hours").await;

    // Row 4: old null-node PENDING task → should NOT be deleted (not failed)
    let old_null_pending = insert_queue_row(&fix, "compile", None, "pending").await;
    backdate_queue_row(&fix, old_null_pending, "48 hours").await;

    // ── Run maintenance ───────────────────────────────────────────────────
    let svc = AdminService::new(fix.pool.clone());
    let result = svc
        .maintenance(MaintenanceRequest {
            process_queue: Some(true),
            ..Default::default()
        })
        .await
        .expect("maintenance should succeed");

    // Verify the action log mentions the Pass B cleanup.
    let pass_b_action = result
        .actions_taken
        .iter()
        .find(|a| a.contains("null-node failed jobs"))
        .cloned();
    assert!(
        pass_b_action.is_some(),
        "actions_taken must include the Pass B null-node cleanup entry; got: {:?}",
        result.actions_taken
    );

    // ── Assert outcomes ───────────────────────────────────────────────────

    // Row 1 must be gone: old null-node failed task.
    assert!(
        !queue_row_exists(&fix, old_null_node).await,
        "old null-node failed task (> 24 h) must be deleted by Pass B"
    );

    // Row 2 must still exist: too fresh.
    assert!(
        queue_row_exists(&fix, fresh_null_node).await,
        "fresh null-node failed task (< 24 h) must NOT be deleted by Pass B"
    );

    // Row 3 must still exist: has a node_id.
    assert!(
        queue_row_exists(&fix, old_with_node).await,
        "old failed task with a non-null node_id must NOT be deleted by Pass B"
    );

    // Row 4 must still exist: not a failed row.
    assert!(
        queue_row_exists(&fix, old_null_pending).await,
        "old null-node PENDING task must NOT be deleted by Pass B"
    );

    fix.cleanup().await;
}

// ─── Pass 0: stale processing timeout ────────────────────────────────────────

/// The maintenance pass flips `status='processing'` rows whose `started_at`
/// is more than 10 minutes old to `status='failed'`.  This prevents stuck
/// processing rows from blocking queue progress indefinitely.
///
/// Scenario:
/// - Processing embed task, started 20 minutes ago → **should become failed**.
/// - Processing embed task, started 2 minutes ago  → should stay processing
///   (not yet timed out).
/// - Failed embed task with old started_at → should stay failed (already terminal).
#[tokio::test]
#[serial]
async fn test_process_queue_times_out_stale_processing_rows() {
    let mut fix = TestFixture::new().await;

    let node_a = fix.insert_source("Timeout Node A", "content A").await;
    let node_b = fix.insert_source("Timeout Node B", "content B").await;
    let node_c = fix.insert_source("Timeout Node C", "content C").await;

    // ── Seed queue rows ───────────────────────────────────────────────────

    // Row 1: processing, started 20 minutes ago → SHOULD be timed out to failed
    let stale_processing = insert_queue_row(&fix, "embed", Some(node_a), "processing").await;
    backdate_started_at(&fix, stale_processing, "20 minutes").await;

    // Row 2: processing, started 2 minutes ago → should stay processing
    let fresh_processing = insert_queue_row(&fix, "embed", Some(node_b), "processing").await;
    backdate_started_at(&fix, fresh_processing, "2 minutes").await;

    // Row 3: already failed, started a long time ago → should stay failed (no change)
    let already_failed = insert_queue_row(&fix, "embed", Some(node_c), "failed").await;
    backdate_started_at(&fix, already_failed, "30 minutes").await;

    // ── Run maintenance ───────────────────────────────────────────────────
    let svc = AdminService::new(fix.pool.clone());
    let result = svc
        .maintenance(MaintenanceRequest {
            process_queue: Some(true),
            ..Default::default()
        })
        .await
        .expect("maintenance should succeed");

    // The timeout action is logged as "timed out N stale queue jobs".
    let timeout_action = result
        .actions_taken
        .iter()
        .find(|a| a.contains("timed out"))
        .cloned();
    assert!(
        timeout_action.is_some(),
        "actions_taken must include the stale-processing timeout entry; got: {:?}",
        result.actions_taken
    );

    // ── Assert outcomes ───────────────────────────────────────────────────

    // Row 1 must now be 'failed'.
    assert_eq!(
        queue_status(&fix, stale_processing).await.as_deref(),
        Some("failed"),
        "processing row started 20 min ago must be timed out to 'failed'"
    );

    // Row 2 must still be 'processing' (not yet 10-minute threshold).
    assert_eq!(
        queue_status(&fix, fresh_processing).await.as_deref(),
        Some("processing"),
        "processing row started 2 min ago must NOT be timed out yet"
    );

    // Row 3 must still be 'failed' (terminal state, not touched by timeout).
    assert_eq!(
        queue_status(&fix, already_failed).await.as_deref(),
        Some("failed"),
        "already-failed row must remain 'failed' and not be changed by the timeout pass"
    );

    fix.cleanup().await;
}
