//! Source-ranking helpers shared across worker task handlers.
//!
//! These pure-DB query functions rank candidate source nodes by different
//! criteria and are shared by `compile`, `consolidate_article`, and (in future)
//! claims compilation.  Extracting them here prevents a third inline copy when
//! the claims worker needs the same logic (covalence#173 wave 3).

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

/// Select up to `cap` source IDs from `candidate_ids`, ranked by
/// `trust_score × exp(-0.1 × days_old)` (covalence#104 recency formula).
///
/// `reliability` is used as the trust_score proxy.  Days are computed from
/// `created_at`.  Returns an empty [`Vec`] immediately when `candidate_ids` is
/// empty (no DB round-trip).
pub(crate) async fn select_by_trust_recency(
    pool: &PgPool,
    candidate_ids: &[Uuid],
    cap: usize,
) -> anyhow::Result<Vec<Uuid>> {
    if candidate_ids.is_empty() {
        return Ok(vec![]);
    }
    sqlx::query_scalar(
        "SELECT id
         FROM   covalence.nodes
         WHERE  id = ANY($1)
         ORDER BY
             COALESCE(reliability, 0.5)
             * EXP(-0.1 * EXTRACT(EPOCH FROM (now() - COALESCE(created_at, now()))) / 86400.0)
             DESC
         LIMIT $2",
    )
    .bind(candidate_ids)
    .bind(cap as i64)
    .fetch_all(pool)
    .await
    .context("select_by_trust_recency: failed to rank sources")
}

/// Select up to `cap` source IDs from `candidate_ids`, ranked by `reliability`
/// only (descending).
///
/// Used for the Stage 1 "lost-in-the-middle" source cap (covalence#85).
/// Returns an empty [`Vec`] immediately when `candidate_ids` is empty.
pub(crate) async fn select_by_reliability(
    pool: &PgPool,
    candidate_ids: &[Uuid],
    cap: usize,
) -> anyhow::Result<Vec<Uuid>> {
    if candidate_ids.is_empty() {
        return Ok(vec![]);
    }
    sqlx::query_scalar(
        "SELECT id FROM covalence.nodes \
         WHERE  id = ANY($1) \
         ORDER BY COALESCE(reliability, 0.5) DESC \
         LIMIT  $2",
    )
    .bind(candidate_ids)
    .bind(cap as i64)
    .fetch_all(pool)
    .await
    .context("select_by_reliability: failed to rank sources")
}
