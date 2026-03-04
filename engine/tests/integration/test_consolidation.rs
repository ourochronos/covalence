//! Integration tests for expanding-interval article recompilation schedule
//! (covalence#67).
//!
//! Verifies that:
//! 1. A compiled article gets `next_consolidation_at` set ~1h from now.
//! 2. Pass 1 → `consolidation_count` = 1, `next_consolidation_at` ≈ +12 h.
//! 3. Tasks with `execute_after` in the future are not claimed by the worker.
//! 4. Pass 3 → count = 3, `next_consolidation_at` ≈ +1 week (schedule continues).
//! 5. Pass 4+ → count = 4, `next_consolidation_at` ≈ +30 days (monthly cadence).
//! 6. Orphan articles (no linked sources) are skipped and not advanced.
//! 7. Maintenance scan enqueues consolidation tasks for due articles.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::consolidation::{
    SCHEDULE_PASS_2_HOURS, SCHEDULE_PASS_4_WEEKS, SCHEDULE_PASS_N_DAYS, handle_consolidate_article,
};
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
    let next_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1")
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

    assert_eq!(
        count, 0,
        "consolidation_count should be 0 immediately after compile"
    );

    fix.cleanup().await;
}

// ─── Test 2 ───────────────────────────────────────────────────────────────────

/// The `consolidate_article` handler must:
/// * increment `consolidation_count` from 0 → 1,
/// * update `next_consolidation_at` to ~SCHEDULE_PASS_2_HOURS from now,
/// * insert a pending `consolidate_article` task for pass 2 with an
///   `execute_after` approximately SCHEDULE_PASS_2_HOURS in the future.
#[tokio::test]
#[serial]
async fn test_consolidation_pass1_increments_count_and_schedules_pass2() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    let src = fix
        .insert_source("Pass 1 Source", "Source content for pass 1 test.")
        .await;
    let article_id = fix
        .insert_article("Pass 1 Test Article", "Initial article content.")
        .await;

    // Link the source so the orphan guard passes.
    fix.insert_originates_edge(src, article_id).await;

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
        "handler should not skip a linked article: {result}"
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

    // next_consolidation_at must be approximately SCHEDULE_PASS_2_HOURS from now.
    let next_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    let next_at = next_at.expect("next_consolidation_at should be set for pass 2");
    let diff_hours = (next_at - Utc::now()).num_hours();
    let expected_hours = SCHEDULE_PASS_2_HOURS;

    assert!(
        diff_hours >= expected_hours - 2 && diff_hours <= expected_hours + 2,
        "next_consolidation_at should be ~{expected_hours}h from now; got {diff_hours}h away"
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

    assert!(
        queued > 0,
        "a pending pass-2 task should be enqueued after pass 1"
    );

    // The pass-2 task's execute_after must be in the future (~12 h).
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
    let pending_status: String =
        sqlx::query_scalar("SELECT status FROM covalence.slow_path_queue WHERE id = $1")
            .bind(future_task_id)
            .fetch_one(&fix.pool)
            .await
            .expect("task should exist");

    assert_eq!(
        pending_status, "pending",
        "future task should remain in pending state"
    );

    fix.cleanup().await;
}

// ─── Test 4 ───────────────────────────────────────────────────────────────────

/// After pass 3 completes, `next_consolidation_at` must be set to ~1 week from
/// now (SCHEDULE_PASS_4_WEEKS) and `consolidation_count` must be 3.
/// The schedule continues — no terminal NULL.
#[tokio::test]
#[serial]
async fn test_pass3_schedules_weekly_followup() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    let src = fix
        .insert_source("Pass 3 Source", "Source content for the third pass test.")
        .await;
    let article_id = fix
        .insert_article("Pass 3 Test Article", "Article content for third pass.")
        .await;

    // Link source so orphan guard passes.
    fix.insert_originates_edge(src, article_id).await;

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
        "handler should not skip a linked article: {result}"
    );
    assert_eq!(result["pass"].as_i64().unwrap(), 3);

    // next_consolidation_at must be approximately SCHEDULE_PASS_4_WEEKS from now.
    let next_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    let next_at = next_at.expect("next_consolidation_at should NOT be NULL after pass 3");
    let diff_days = (next_at - Utc::now()).num_days();
    let expected_days = SCHEDULE_PASS_4_WEEKS * 7;

    assert!(
        diff_days >= expected_days - 1 && diff_days <= expected_days + 1,
        "next_consolidation_at should be ~{expected_days} days from now after pass 3; \
         got {diff_days} days"
    );

    // consolidation_count must be 3.
    let count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    assert_eq!(count, 3, "consolidation_count should be 3 after pass 3");

    // A pass-4 task must have been scheduled.
    let pass4_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  (payload->>'pass')::int = 4 \
           AND  status                  = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        pass4_tasks > 0,
        "a pass-4 task should be enqueued after pass 3 completes (schedule continues)"
    );

    fix.cleanup().await;
}

// ─── Test 5 ───────────────────────────────────────────────────────────────────

/// After pass 4 (and any subsequent pass), `next_consolidation_at` should be
/// set to approximately SCHEDULE_PASS_N_DAYS (≈ 30 days) from now.
#[tokio::test]
#[serial]
async fn test_pass4_and_beyond_uses_monthly_interval() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    let src = fix
        .insert_source("Monthly Source", "Source for monthly interval test.")
        .await;
    let article_id = fix
        .insert_article(
            "Monthly Interval Article",
            "Article for monthly cadence test.",
        )
        .await;

    fix.insert_originates_edge(src, article_id).await;

    // Simulate having completed passes 1–3 already (count = 3).
    sqlx::query(
        "UPDATE covalence.nodes \
         SET next_consolidation_at = now() - INTERVAL '1 minute', \
             consolidation_count   = 3 \
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("setup update should succeed");

    // Run pass 4.
    let task = TestFixture::make_task(
        "consolidate_article",
        None,
        json!({
            "article_id": article_id.to_string(),
            "pass": 4
        }),
    );

    let result = handle_consolidate_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_consolidate_article should succeed");

    assert!(
        result.get("skipped").is_none(),
        "handler should not skip a linked article: {result}"
    );
    assert_eq!(result["pass"].as_i64().unwrap(), 4);

    // consolidation_count must be 4.
    let count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    assert_eq!(count, 4, "consolidation_count should be 4 after pass 4");

    // next_consolidation_at must be ~SCHEDULE_PASS_N_DAYS from now.
    let next_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    let next_at = next_at.expect("next_consolidation_at should be set after pass 4");
    let diff_days = (next_at - Utc::now()).num_days();
    let expected_days = SCHEDULE_PASS_N_DAYS;

    assert!(
        diff_days >= expected_days - 2 && diff_days <= expected_days + 2,
        "next_consolidation_at should be ~{expected_days} days from now after pass 4; \
         got {diff_days} days"
    );

    // A pass-5 task must be scheduled.
    let pass5_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  (payload->>'pass')::int = 5 \
           AND  status                  = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        pass5_tasks > 0,
        "a pass-5 task should be enqueued after pass 4"
    );

    fix.cleanup().await;
}

// ─── Test 6 ───────────────────────────────────────────────────────────────────

/// An orphan article (one with zero linked sources) must be **skipped** by the
/// handler.  The `consolidation_count` and `next_consolidation_at` must remain
/// unchanged so the heartbeat can retry once sources are linked.
#[tokio::test]
#[serial]
async fn test_orphan_article_is_skipped() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("consolidate_article");

    // Insert an article with NO linked sources — this is the orphan case.
    let article_id = fix
        .insert_article("Orphan Article", "Article with no linked sources.")
        .await;

    // Record the initial consolidation state.
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
        .expect("handle_consolidate_article should not error on orphan");

    assert_eq!(
        result["skipped"].as_bool(),
        Some(true),
        "orphan article must return skipped=true: {result}"
    );
    assert_eq!(
        result["reason"].as_str(),
        Some("orphan_article"),
        "skip reason must be 'orphan_article': {result}"
    );

    // consolidation_count must NOT have been advanced.
    let count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article row should exist");

    assert_eq!(
        count, 0,
        "consolidation_count must remain 0 for a skipped orphan article"
    );

    // No follow-up pass task must have been enqueued.
    let follow_up: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        follow_up, 0,
        "no follow-up tasks should be enqueued for a skipped orphan article"
    );

    fix.cleanup().await;
}

// ─── Test 7 ───────────────────────────────────────────────────────────────────

/// The admin maintenance endpoint's `scan_due_consolidations` flag must query
/// for articles where `next_consolidation_at <= now()` and enqueue
/// `consolidate_article` tasks for them.
#[tokio::test]
#[serial]
async fn test_maintenance_scan_queues_due_articles() {
    let mut fix = TestFixture::new().await;
    fix.track_task_type("consolidate_article");

    // Insert two articles: one overdue, one not yet due.
    let overdue_id = fix
        .insert_article(
            "Overdue Article",
            "This article is past its consolidation time.",
        )
        .await;
    let not_due_id = fix
        .insert_article("Not Due Article", "This article is not yet due.")
        .await;

    sqlx::query(
        "UPDATE covalence.nodes \
         SET next_consolidation_at = now() - INTERVAL '5 minutes', \
             consolidation_count   = 0 \
         WHERE id = $1",
    )
    .bind(overdue_id)
    .execute(&fix.pool)
    .await
    .expect("setup overdue update should succeed");

    sqlx::query(
        "UPDATE covalence.nodes \
         SET next_consolidation_at = now() + INTERVAL '6 hours', \
             consolidation_count   = 0 \
         WHERE id = $1",
    )
    .bind(not_due_id)
    .execute(&fix.pool)
    .await
    .expect("setup not-due update should succeed");

    // Call the maintenance service directly.
    use covalence_engine::services::admin_service::{AdminService, MaintenanceRequest};
    let svc = AdminService::new(fix.pool.clone());
    let resp = svc
        .maintenance(MaintenanceRequest {
            scan_due_consolidations: Some(true),
            ..Default::default()
        })
        .await
        .expect("maintenance should succeed");

    // At least one action must mention the scan.
    assert!(
        resp.actions_taken
            .iter()
            .any(|a| a.contains("scan_due_consolidations")),
        "actions_taken must include scan_due_consolidations: {:?}",
        resp.actions_taken
    );

    // A consolidate_article task must exist for the overdue article.
    let overdue_queued: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  status                  = 'pending'",
    )
    .bind(overdue_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        overdue_queued > 0,
        "a consolidate_article task should be queued for the overdue article"
    );

    // No task must exist for the not-yet-due article.
    let not_due_queued: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  status                  = 'pending'",
    )
    .bind(not_due_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        not_due_queued, 0,
        "no consolidate_article task should be queued for the not-yet-due article"
    );

    fix.cleanup().await;
}
