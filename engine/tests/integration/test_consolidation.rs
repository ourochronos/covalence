//! Integration tests for expanding-interval article recompilation schedule
//! (covalence#67).
//!
//! Verifies that:
//! 1. A compiled article gets `next_consolidation_at` set ~1h from now.
//! 2. The `consolidate_article` handler increments `consolidation_count` and
//!    schedules the next pass with an appropriate `execute_after`.
//! 3. Tasks with `execute_after` in the future are not claimed by the worker's
//!    polling query.
//! 4. Pass 3 completion sets `next_consolidation_at = NULL` (all passes done).

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::consolidation::handle_consolidate_article;
use covalence_engine::worker::{handle_compile, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── Test 1 ───────────────────────────────────────────────────────────────────

/// After a `compile` task produces an article, `next_consolidation_at` must be
/// set to approximately one hour from now and `consolidation_count` must be 0.
#[tokio::test]
#[serial]
async fn test_compiled_article_gets_consolidation_scheduled() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src1 = fix
        .insert_source("Alpha Source", "Content about alpha systems.")
        .await;
    let src2 = fix
        .insert_source("Beta Source", "Content about beta configurations.")
        .await;

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src1, src2]);

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src1.to_string(), src2.to_string()],
            "title_hint": "Consolidation Schedule Test Article"
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap())
        .expect("article_id should be a valid UUID");
    fix.track(article_id);

    // next_consolidation_at must be set and within ~2 hours of now.
    let next_at: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .expect("article row should exist");

    let next_at = next_at.expect("next_consolidation_at should be non-NULL after compile");
    let diff_minutes = (next_at - Utc::now()).num_minutes();

    assert!(
        diff_minutes > 30 && diff_minutes < 120,
        "next_consolidation_at should be ~1 hour from now; got {diff_minutes} minutes away"
    );

    // consolidation_count must start at 0.
    let count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    assert_eq!(count, 0, "consolidation_count should be 0 immediately after compile");

    fix.cleanup().await;
}

// ─── Test 2 ───────────────────────────────────────────────────────────────────

/// The `consolidate_article` handler must:
/// * increment `consolidation_count` from 0 → 1,
/// * update `next_consolidation_at` to ~18 hours from now,
/// * insert a pending `consolidate_article` task for pass 2 with an
///   `execute_after` approximately 18 hours in the future.
#[tokio::test]
#[serial]
async fn test_consolidation_pass1_increments_count_and_schedules_pass2() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    let article_id = fix
        .insert_article("Pass 1 Test Article", "Initial article content.")
        .await;

    // Mark the article as due for its first consolidation pass.
    sqlx::query(
        "UPDATE covalence.nodes \
         SET next_consolidation_at = now() - INTERVAL '1 minute', \
             consolidation_count   = 0 \
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("setup update should succeed");

    // Run pass 1.
    let task = TestFixture::make_task(
        "consolidate_article",
        None,
        json!({
            "article_id": article_id.to_string(),
            "pass": 1
        }),
    );

    let result = handle_consolidate_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_consolidate_article should succeed");

    assert!(
        result.get("skipped").is_none(),
        "handler should not skip a due article: {result}"
    );
    assert_eq!(result["pass"].as_i64().unwrap(), 1);

    // consolidation_count must now be 1.
    let count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    assert_eq!(count, 1, "consolidation_count should be 1 after pass 1");

    // next_consolidation_at must be approximately 18 hours from now.
    let next_at: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .expect("article row should exist");

    let next_at = next_at.expect("next_consolidation_at should be set for pass 2");
    let diff_hours = (next_at - Utc::now()).num_hours();

    assert!(
        diff_hours >= 16 && diff_hours <= 20,
        "next_consolidation_at should be ~18h from now; got {diff_hours}h away"
    );

    // A pending pass-2 task must exist in the queue.
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type                = 'consolidate_article' \
           AND  payload->>'article_id'   = $1 \
           AND  (payload->>'pass')::int  = 2 \
           AND  status                   = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(queued > 0, "a pending pass-2 task should be enqueued after pass 1");

    // The pass-2 task's execute_after must be in the future (~ 18 h).
    let exec_after: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT execute_after \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  (payload->>'pass')::int = 2 \
           AND  status                  = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .expect("pass-2 task should exist");

    let exec_after = exec_after.expect("execute_after should be set on the pass-2 task");
    assert!(
        exec_after > Utc::now(),
        "execute_after on the pass-2 task should be in the future"
    );

    fix.cleanup().await;
}

// ─── Test 3 ───────────────────────────────────────────────────────────────────

/// A task with `execute_after` in the future must NOT be claimed by the
/// worker's polling query (`WHERE execute_after IS NULL OR execute_after <= now()`).
///
/// This test directly verifies the SQL predicate rather than running the full
/// worker loop, keeping it deterministic and fast.
#[tokio::test]
#[serial]
async fn test_future_execute_after_task_not_claimable() {
    let mut fix = TestFixture::new().await;
    fix.track_task_type("consolidate_article");

    let article_id = fix
        .insert_article("Future Task Article", "article content")
        .await;

    // Insert a consolidate_article task scheduled 1 hour in the future.
    let future_task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
             (id, task_type, node_id, payload, status, priority, execute_after) \
         VALUES ($1, 'consolidate_article', NULL, $2, 'pending', 3, now() + INTERVAL '1 hour')",
    )
    .bind(future_task_id)
    .bind(json!({
        "article_id": article_id.to_string(),
        "pass": 2
    }))
    .execute(&fix.pool)
    .await
    .expect("task insert should succeed");

    // The worker's claim query excludes tasks where execute_after > now().
    // Count how many such tasks would be eligible for claiming right now.
    let claimable: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type  = 'consolidate_article' \
           AND  status     = 'pending' \
           AND  (execute_after IS NULL OR execute_after <= now())",
    )
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        claimable, 0,
        "a task with execute_after in the future should not be claimable by the worker \
         (got {claimable} claimable)"
    );

    // Confirm the task is still in pending state (not lost).
    let pending_status: String = sqlx::query_scalar(
        "SELECT status FROM covalence.slow_path_queue WHERE id = $1",
    )
    .bind(future_task_id)
    .fetch_one(&fix.pool)
    .await
    .expect("task should exist");

    assert_eq!(pending_status, "pending", "future task should remain in pending state");

    fix.cleanup().await;
}

// ─── Test 4 ───────────────────────────────────────────────────────────────────

/// After pass 3 completes, `next_consolidation_at` must be set to NULL and
/// `consolidation_count` must be 3.  No further tasks should be enqueued by
/// the handler.
#[tokio::test]
#[serial]
async fn test_pass3_clears_consolidation_schedule() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    let article_id = fix
        .insert_article("Pass 3 Test Article", "Article content for final pass.")
        .await;

    // Simulate having completed passes 1 and 2 already.
    sqlx::query(
        "UPDATE covalence.nodes \
         SET next_consolidation_at = now() - INTERVAL '1 minute', \
             consolidation_count   = 2 \
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("setup update should succeed");

    // Run pass 3.
    let task = TestFixture::make_task(
        "consolidate_article",
        None,
        json!({
            "article_id": article_id.to_string(),
            "pass": 3
        }),
    );

    let result = handle_consolidate_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_consolidate_article should succeed");

    assert!(
        result.get("skipped").is_none(),
        "handler should not skip a due article: {result}"
    );
    assert_eq!(result["pass"].as_i64().unwrap(), 3);

    // next_consolidation_at must be NULL (no further scheduled passes).
    let next_at: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .expect("article row should exist");

    assert!(
        next_at.is_none(),
        "next_consolidation_at should be NULL after pass 3 completes"
    );

    // consolidation_count must be 3.
    let count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    assert_eq!(count, 3, "consolidation_count should be 3 after all passes complete");

    // No further consolidate_article tasks should have been enqueued.
    let further_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  (payload->>'pass')::int > 3 \
           AND  status                  = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        further_tasks, 0,
        "no tasks beyond pass 3 should be enqueued by the handler"
    );

    fix.cleanup().await;
}
