//! `decay_check` slow-path handler (Issue #21).
//!
//! Computes a decay score for a node based on elapsed time since last
//! modification, unresolved contentions, and absence of inferred edges.
//! The score is persisted in `metadata.decay_score` and — when it exceeds
//! [`RECOMPILE_THRESHOLD`] — a follow-up `compile` task is queued so the
//! worker can refresh the article's content from its source nodes.
//!
//! # Scoring formula
//!
//! | Component        | Weight | Description                                          |
//! |------------------|--------|------------------------------------------------------|
//! | Age              | 0.50   | `days_since_modified / MAX_AGE_DAYS`, clamped to 1   |
//! | Contention ratio | 0.30   | `open / (open + 1)` — penalises unresolved conflicts |
//! | Edge staleness   | 0.20   | 1 if node has no inferred outbound edges, else 0     |
//!
//! `decay_score = 0.5·age + 0.3·contention + 0.2·edge_staleness`

use anyhow::Context;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::QueueTask;

// ─── constants ────────────────────────────────────────────────────────────────

/// Articles this many days old without modification receive the maximum age score.
const MAX_AGE_DAYS: f64 = 365.0;

/// When `decay_score` meets or exceeds this value a `compile` task is queued
/// to refresh the article.
pub const RECOMPILE_THRESHOLD: f64 = 0.70;

// ─── handler ─────────────────────────────────────────────────────────────────

/// Handle a `decay_check` queue task.
///
/// # Behaviour
/// 1. Fetch node from `covalence.nodes`; skip if archived.
/// 2. Compute the three scoring components.
/// 3. Persist composite score to `metadata.decay_score`.
/// 4. If score ≥ [`RECOMPILE_THRESHOLD`], enqueue a `compile` task at
///    low priority (2) so the article can be refreshed asynchronously.
/// 5. Return a JSON result suitable for storage in `slow_path_queue.result`.
pub async fn handle_decay_check(pool: &PgPool, task: &QueueTask) -> anyhow::Result<Value> {
    use sqlx::Row as _;

    let node_id = task
        .node_id
        .context("decay_check task requires node_id")?;

    // ── 1. Fetch node ─────────────────────────────────────────────────────────
    let row = sqlx::query(
        "SELECT status \
         FROM   covalence.nodes \
         WHERE  id = $1",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?
    .with_context(|| format!("decay_check: node {node_id} not found"))?;

    let status: String = row.get::<Option<String>, _>("status").unwrap_or_default();

    // Archived nodes are not eligible for decay scoring.
    if status == "archived" {
        tracing::debug!(
            node_id = %node_id,
            "decay_check: node is archived — skipping"
        );
        return Ok(json!({
            "node_id":     node_id,
            "skipped":     true,
            "reason":      "archived",
            "decay_score": null,
        }));
    }

    // ── 2. Age score ──────────────────────────────────────────────────────────
    let age_days: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (now() - modified_at)) / 86400.0 \
         FROM   covalence.nodes \
         WHERE  id = $1",
    )
    .bind(node_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0.0_f64);

    let age_score: f64 = (age_days / MAX_AGE_DAYS).min(1.0);

    // ── 3. Contention score ───────────────────────────────────────────────────
    let open_contentions: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.contentions \
         WHERE  node_id = $1 \
           AND  status != 'resolved'",
    )
    .bind(node_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0_i64);

    // Monotonically-increasing ratio that saturates towards 1 but never reaches it.
    let contention_score: f64 =
        open_contentions as f64 / (open_contentions as f64 + 1.0_f64);

    // ── 4. Edge-staleness score ───────────────────────────────────────────────
    let inferred_edges: i64 = sqlx::query_scalar(
        "SELECT count(*) \
         FROM   covalence.edges \
         WHERE  source_node_id = $1 \
           AND  metadata->>'inferred' = 'true'",
    )
    .bind(node_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0_i64);

    // No inferred edges → worst staleness score.
    let edge_staleness_score: f64 = if inferred_edges == 0 { 1.0 } else { 0.0 };

    // ── 5. Composite decay score ──────────────────────────────────────────────
    let decay_score: f64 =
        0.5 * age_score + 0.3 * contention_score + 0.2 * edge_staleness_score;

    tracing::debug!(
        node_id             = %node_id,
        age_days,
        age_score,
        open_contentions,
        contention_score,
        inferred_edges,
        edge_staleness_score,
        decay_score,
        "decay_check: scores computed"
    );

    // ── 6. Persist score in metadata ──────────────────────────────────────────
    sqlx::query(
        r#"UPDATE covalence.nodes
              SET metadata    = jsonb_set(
                                    coalesce(metadata, '{}'::jsonb),
                                    '{decay_score}',
                                    $1::text::jsonb,
                                    true
                                ),
                  modified_at = now()
            WHERE id = $2"#,
    )
    .bind(decay_score.to_string())
    .bind(node_id)
    .execute(pool)
    .await
    .context("decay_check: failed to persist decay_score in metadata")?;

    // ── 7. Optionally queue a re-compile task ─────────────────────────────────
    let recompile_queued = if decay_score >= RECOMPILE_THRESHOLD {
        tracing::info!(
            node_id     = %node_id,
            decay_score,
            threshold   = RECOMPILE_THRESHOLD,
            "decay_check: score above recompile threshold — queuing compile task"
        );
        super::enqueue_task(pool, "compile", Some(node_id), json!({}), 2).await?;
        true
    } else {
        false
    };

    Ok(json!({
        "node_id":            node_id,
        "decay_score":        decay_score,
        "age_days":           age_days,
        "age_score":          age_score,
        "open_contentions":   open_contentions,
        "contention_score":   contention_score,
        "inferred_edges":     inferred_edges,
        "edge_staleness":     edge_staleness_score,
        "recompile_queued":   recompile_queued,
        "skipped":            false,
    }))
}
