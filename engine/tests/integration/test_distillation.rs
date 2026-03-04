//! Integration tests for progressive distillation pipeline (covalence#104).
//!
//! Verifies that:
//! 1. Stage 1 (Rich, consolidation_count ≤ 2): behaviour is unchanged —
//!    no sources are marked `distilled_out_at`, the full source set is used.
//! 2. Stage 2 (Selective, count 3–4): when more than 10 sources are linked,
//!    the handler selects the top-10 by `trust_score × recency`, marks the
//!    remainder with `metadata.distilled_out_at`, and always recompiles even
//!    when no new sources exist.
//! 3. Stage 4 (Distilled, count 7+): when more than 4 sources are linked,
//!    only the top-4 are used; the dropped sources are marked non-destructively
//!    and their provenance edges are retained.

use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::consolidation::{DistillationStage, handle_consolidate_article};
use covalence_engine::worker::llm::LlmClient;

use super::helpers::{MockLlmClient, TestFixture};

// ─── helper: insert a source with explicit reliability ────────────────────────

/// Insert a source node with a specific `reliability` score and, optionally,
/// a fake `created_at` offset so tests can control the recency ranking.
async fn insert_source_with_reliability(
    fix: &mut TestFixture,
    title: &str,
    content: &str,
    reliability: f64,
    days_old: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, metadata, reliability, created_at, modified_at) \
         VALUES ($1, 'source', 'active', $2, $3, '{}'::jsonb, $4,
                 now() - ($5 * INTERVAL '1 day'),
                 now() - ($5 * INTERVAL '1 day'))",
    )
    .bind(id)
    .bind(title)
    .bind(content)
    .bind(reliability)
    .bind(days_old as f64)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_source_with_reliability({title}) failed: {e}"));
    fix.track(id)
}

// ─── Test 1: Stage 1 (Rich) — no change to current behaviour ─────────────────

/// With `consolidation_count` = 1 (stage 1 / Rich), the distillation pipeline
/// must not alter source selection beyond the legacy cap, and no sources must
/// be marked with `distilled_out_at`.
#[tokio::test]
#[serial]
async fn test_stage1_rich_no_distillation_markers() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    // Insert 4 sources with varying reliability.
    let mut src_ids = Vec::new();
    for i in 0..4 {
        let id = insert_source_with_reliability(
            &mut fix,
            &format!("Stage1 Source {i}"),
            &format!("Source {i} content for stage 1 test."),
            0.9 - (i as f64 * 0.1), // 0.9, 0.8, 0.7, 0.6
            i as i64,               // 0, 1, 2, 3 days old
        )
        .await;
        src_ids.push(id);
    }

    let article_id = fix
        .insert_article("Stage 1 Article", "Initial article content.")
        .await;

    // Link all sources to the article.
    for &src in &src_ids {
        fix.insert_originates_edge(src, article_id).await;
    }

    // Set count = 1 (stage 1, Rich) and mark as due.
    sqlx::query(
        "UPDATE covalence.nodes
         SET next_consolidation_at = now() - INTERVAL '1 minute',
             consolidation_count   = 1
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("setup update should succeed");

    // Verify stage derivation.
    assert_eq!(DistillationStage::from_count(1), DistillationStage::Rich);

    // Run pass 2 (count=1 → stage Rich).
    let task = TestFixture::make_task(
        "consolidate_article",
        None,
        json!({
            "article_id": article_id.to_string(),
            "pass": 2
        }),
    );

    let result = handle_consolidate_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_consolidate_article should succeed");

    assert!(
        result.get("skipped").is_none(),
        "handler should not skip a linked article: {result}"
    );
    assert_eq!(
        result["stage"].as_str(),
        Some("Rich"),
        "stage must be Rich for count=1"
    );

    // CRITICAL: no source must have distilled_out_at set.
    for &src in &src_ids {
        let meta: Value = sqlx::query_scalar("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(src)
            .fetch_one(&fix.pool)
            .await
            .expect("source should exist");

        assert!(
            meta.get("distilled_out_at").is_none(),
            "source {src} must NOT have distilled_out_at set in stage 1: {meta}"
        );
    }

    fix.cleanup().await;
}

// ─── Test 2: Stage 2 (Selective) — top-10 selection + distilled_out marking ──

/// With `consolidation_count` = 3 (stage 2 / Selective), when 14 sources are
/// linked only the top-10 by `trust_score × recency` should be retained; the
/// remaining 4 must receive `metadata.distilled_out_at`.  The handler must
/// recompile even though no new sources exist.
#[tokio::test]
#[serial]
async fn test_stage2_selective_marks_dropped_sources() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    // Insert 14 sources.  Give the first 10 high reliability + low age
    // so they rank at the top, and the last 4 low reliability + high age
    // so they are the ones distilled out.
    let total_sources = 14usize;
    let cap = 10usize; // stage 2 cap
    let expected_dropped = total_sources - cap;

    let mut high_rank_ids = Vec::new();
    let mut low_rank_ids = Vec::new();

    for i in 0..total_sources {
        let (reliability, days_old, group) = if i < cap {
            (0.95, i as i64, "high")
        } else {
            (0.10, 365 + i as i64, "low") // very old, very low reliability
        };
        let id = insert_source_with_reliability(
            &mut fix,
            &format!("Selective Source {} ({})", i, group),
            &format!("Content for source {i}."),
            reliability,
            days_old,
        )
        .await;
        if i < cap {
            high_rank_ids.push(id);
        } else {
            low_rank_ids.push(id);
        }
    }

    let article_id = fix
        .insert_article(
            "Stage 2 Article",
            "Initial article content for selective test.",
        )
        .await;

    // Link all 14 sources.
    for &src in high_rank_ids.iter().chain(low_rank_ids.iter()) {
        fix.insert_originates_edge(src, article_id).await;
    }

    // Set count = 3 (stage 2, Selective) and mark as due.
    sqlx::query(
        "UPDATE covalence.nodes
         SET next_consolidation_at = now() - INTERVAL '1 minute',
             consolidation_count   = 3
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("setup update should succeed");

    // Verify stage derivation.
    assert_eq!(
        DistillationStage::from_count(3),
        DistillationStage::Selective
    );
    assert_eq!(DistillationStage::Selective.source_cap(), Some(10));

    // Run pass 4 (count=3 → stage Selective).
    // No new sources → stage 1 would skip, but stage 2 always recompiles.
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
        "handler must not skip a stage-2 article even with no new sources: {result}"
    );
    assert_eq!(
        result["stage"].as_str(),
        Some("Selective"),
        "stage must be Selective for count=3"
    );

    // The handler must have recompiled (article content updated).
    // We verify by checking that consolidation_count was advanced to pass=4.
    let new_count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article should exist");
    assert_eq!(new_count, 4, "consolidation_count should be 4 after pass 4");

    // Low-rank sources (the last 4) must have distilled_out_at set.
    let mut marked_count = 0usize;
    for &src in &low_rank_ids {
        let meta: Value = sqlx::query_scalar("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(src)
            .fetch_one(&fix.pool)
            .await
            .expect("source should exist");

        if meta.get("distilled_out_at").is_some() {
            marked_count += 1;
        }
    }
    assert_eq!(
        marked_count, expected_dropped,
        "exactly {expected_dropped} low-rank sources must be marked distilled_out: \
         got {marked_count}"
    );

    // High-rank sources must NOT have distilled_out_at set.
    for &src in &high_rank_ids {
        let meta: Value = sqlx::query_scalar("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(src)
            .fetch_one(&fix.pool)
            .await
            .expect("source should exist");
        assert!(
            meta.get("distilled_out_at").is_none(),
            "high-rank source {src} must NOT be marked distilled_out: {meta}"
        );
    }

    // Dropped sources must still have their ORIGINATES edge (non-destructive).
    for &src in &low_rank_ids {
        let edge_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'ORIGINATES'",
        )
        .bind(src)
        .bind(article_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert_eq!(
            edge_count, 1,
            "dropped source {src} must retain its ORIGINATES provenance edge"
        );
    }

    fix.cleanup().await;
}

// ─── Test 3: Stage 4 (Distilled) — top-4 selection + backward compat ─────────

/// With `consolidation_count` = 7 (stage 4 / Distilled), when 8 sources are
/// linked only the top-4 by `trust_score × recency` are used; the other 4 are
/// marked `distilled_out_at`.  Backward compatibility is verified: the
/// consolidation_count still advances correctly and the next-pass task is
/// enqueued.
#[tokio::test]
#[serial]
async fn test_stage4_distilled_top4_and_backward_compat() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());
    fix.track_task_type("embed");
    fix.track_task_type("consolidate_article");

    let total_sources = 8usize;
    let cap = 4usize; // stage 4 cap
    let expected_dropped = total_sources - cap;

    let mut high_rank_ids = Vec::new();
    let mut low_rank_ids = Vec::new();

    for i in 0..total_sources {
        let (reliability, days_old, group) = if i < cap {
            (0.95, i as i64, "high") // recent + reliable → selected
        } else {
            (0.05, 500 + i as i64, "low") // very old + unreliable → dropped
        };
        let id = insert_source_with_reliability(
            &mut fix,
            &format!("Distilled Source {} ({})", i, group),
            &format!("Content for distilled source {i}."),
            reliability,
            days_old,
        )
        .await;
        if i < cap {
            high_rank_ids.push(id);
        } else {
            low_rank_ids.push(id);
        }
    }

    let article_id = fix
        .insert_article(
            "Stage 4 Article",
            "Initial article content for distillation test.",
        )
        .await;

    // Link all 8 sources.
    for &src in high_rank_ids.iter().chain(low_rank_ids.iter()) {
        fix.insert_originates_edge(src, article_id).await;
    }

    // Set count = 7 (stage 4, Distilled) and mark as due.
    sqlx::query(
        "UPDATE covalence.nodes
         SET next_consolidation_at = now() - INTERVAL '1 minute',
             consolidation_count   = 7
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .expect("setup update should succeed");

    // Verify stage derivation.
    assert_eq!(
        DistillationStage::from_count(7),
        DistillationStage::Distilled
    );
    assert_eq!(DistillationStage::Distilled.source_cap(), Some(4));
    assert!(DistillationStage::Distilled.always_recompile());

    // Run pass 8 (count=7 → stage Distilled).
    let task = TestFixture::make_task(
        "consolidate_article",
        None,
        json!({
            "article_id": article_id.to_string(),
            "pass": 8
        }),
    );

    let result = handle_consolidate_article(&fix.pool, &llm, &task)
        .await
        .expect("handle_consolidate_article should succeed");

    assert!(
        result.get("skipped").is_none(),
        "handler must not skip a stage-4 article: {result}"
    );
    assert_eq!(
        result["stage"].as_str(),
        Some("Distilled"),
        "stage must be Distilled for count=7"
    );

    // ── Backward compat: count advances and next-pass task is enqueued ───────
    let new_count: i32 =
        sqlx::query_scalar("SELECT consolidation_count FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article should exist");
    assert_eq!(
        new_count, 8,
        "consolidation_count should be 8 after pass 8 (backward compat)"
    );

    let next_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT next_consolidation_at FROM covalence.nodes WHERE id = $1")
            .bind(article_id)
            .fetch_one(&fix.pool)
            .await
            .expect("article should exist");
    assert!(
        next_at.is_some(),
        "next_consolidation_at must be set after a distilled pass"
    );
    assert!(
        next_at.unwrap() > Utc::now(),
        "next_consolidation_at must be in the future after a distilled pass"
    );

    let pass9_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.slow_path_queue \
         WHERE  task_type               = 'consolidate_article' \
           AND  payload->>'article_id'  = $1 \
           AND  (payload->>'pass')::int = 9 \
           AND  status                  = 'pending'",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        pass9_tasks > 0,
        "a pass-9 task must be enqueued after pass 8 (schedule continues — backward compat)"
    );

    // ── Distillation: low-rank sources marked, high-rank sources clean ───────
    let mut marked_count = 0usize;
    for &src in &low_rank_ids {
        let meta: Value = sqlx::query_scalar("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(src)
            .fetch_one(&fix.pool)
            .await
            .expect("source should exist");

        if meta.get("distilled_out_at").is_some() {
            marked_count += 1;
        }
    }
    assert_eq!(
        marked_count, expected_dropped,
        "exactly {expected_dropped} low-rank sources must be marked distilled_out; \
         got {marked_count}"
    );

    for &src in &high_rank_ids {
        let meta: Value = sqlx::query_scalar("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(src)
            .fetch_one(&fix.pool)
            .await
            .expect("source should exist");
        assert!(
            meta.get("distilled_out_at").is_none(),
            "high-rank source {src} must NOT be marked distilled_out in stage 4: {meta}"
        );
    }

    // Provenance edges must be retained for dropped sources (non-destructive).
    for &src in &low_rank_ids {
        let edge_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'ORIGINATES'",
        )
        .bind(src)
        .bind(article_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert_eq!(
            edge_count, 1,
            "dropped source {src} must retain its ORIGINATES provenance edge after distillation"
        );
    }

    fix.cleanup().await;
}
