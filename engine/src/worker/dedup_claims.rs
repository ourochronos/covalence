//! Retroactive claim deduplication worker (covalence#208).
//!
//! Scans active `claim` nodes for near-duplicate pairs using the HNSW
//! pgvector index and either reports candidates (dry-run) or creates
//! `SAME_AS` edges between duplicates while canonicalising to the claim
//! with more `SUPPORTS_CLAIM` edges.
//!
//! ## Configuration
//!
//! The default cosine-distance threshold is controlled by the
//! `COVALENCE_CLAIM_DEDUP_THRESHOLD` environment variable (default `0.08`,
//! which corresponds to ≥ 0.92 cosine similarity).  Values are clamped to
//! `[0.0, 1.0]`.
//!
//! ## Queue task
//!
//! Task type: `"dedup_claims"`
//!
//! Payload fields (all optional):
//! | field       | type   | default                                    |
//! |-------------|--------|--------------------------------------------|
//! | `threshold` | f64    | `COVALENCE_CLAIM_DEDUP_THRESHOLD` or 0.08  |
//! | `dry_run`   | bool   | `true`                                     |
//! | `limit`     | i64    | 500                                        |

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use crate::worker::QueueTask;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Default cosine-distance threshold used when
/// `COVALENCE_CLAIM_DEDUP_THRESHOLD` is not set.
///
/// 0.08 distance ≈ 0.92 cosine similarity — tight enough to catch
/// near-verbatim duplicates without false positives.
pub const DEFAULT_DEDUP_THRESHOLD: f64 = 0.08;

/// Default HNSW candidate scan limit per seed claim.
const DEFAULT_LIMIT: i64 = 500;

/// Number of nearest neighbours fetched per claim from the HNSW index.
const HNSW_K: i64 = 10;

// ─── Public helpers ───────────────────────────────────────────────────────────

/// Read the effective dedup threshold from the environment, falling back to
/// [`DEFAULT_DEDUP_THRESHOLD`] if the variable is absent or invalid.
pub fn effective_threshold() -> f64 {
    std::env::var("COVALENCE_CLAIM_DEDUP_THRESHOLD")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(DEFAULT_DEDUP_THRESHOLD)
        .clamp(0.0, 1.0)
}

// ─── Result types ─────────────────────────────────────────────────────────────

/// A candidate duplicate pair surfaced by the scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupCandidate {
    /// The "seed" claim node id.
    pub claim_a_id: Uuid,
    /// The neighbouring claim node id.
    pub claim_b_id: Uuid,
    /// Cosine similarity (1 − distance).
    pub similarity: f64,
    /// Text content of `claim_a`.
    pub text_a: String,
    /// Text content of `claim_b`.
    pub text_b: String,
}

/// Summary returned after a dedup run.
#[derive(Debug, Serialize, Deserialize)]
pub struct DedupResult {
    /// Whether this was a dry run (no edges written).
    pub dry_run: bool,
    /// Distance threshold used.
    pub threshold: f64,
    /// Number of active claim nodes scanned.
    pub claims_scanned: usize,
    /// Candidate pairs found within the threshold.
    pub pairs_found: usize,
    /// `SAME_AS` edges created (0 in dry-run mode).
    pub edges_created: usize,
    /// Candidate pairs (populated in dry-run; empty in apply mode to keep payload small).
    pub candidates: Vec<DedupCandidate>,
}

// ─── Core implementation ──────────────────────────────────────────────────────

/// Run the claim dedup scan against `pool`.
///
/// * In **dry-run** mode the function returns all candidate pairs without
///   writing anything.
/// * In **apply** mode it creates `SAME_AS` edges for each pair and does not
///   populate `candidates` in the returned result.
pub async fn run_dedup_claims(
    pool: &PgPool,
    threshold: f64,
    dry_run: bool,
    limit: i64,
) -> anyhow::Result<DedupResult> {
    // ── 1. Fetch active claim nodes that have embeddings ─────────────────────
    let claim_rows = sqlx::query(
        r#"
        SELECT n.id, n.content
        FROM   covalence.nodes n
        JOIN   covalence.node_embeddings ne ON ne.node_id = n.id
        WHERE  n.node_type = 'claim'
          AND  n.status    = 'active'
        ORDER  BY n.created_at
        LIMIT  $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("dedup_claims: failed to fetch claim nodes")?;

    let claims_scanned = claim_rows.len();
    let mut seen_pairs: std::collections::HashSet<(Uuid, Uuid)> = std::collections::HashSet::new();
    let mut candidates: Vec<DedupCandidate> = Vec::new();
    let mut edges_created: usize = 0;

    // ── 2. For each claim, find near neighbours via HNSW index ───────────────
    for row in &claim_rows {
        let claim_id: Uuid = row.try_get("id")?;
        let claim_text: String = row.try_get("content").unwrap_or_default();

        // Find HNSW neighbours within threshold using pgvector cosine distance.
        // We join back to node_embeddings to get the seed embedding inline.
        let neighbours = sqlx::query(
            r#"
            SELECT
                nb.node_id          AS neighbour_id,
                n2.content          AS neighbour_text,
                (seed.embedding::vector <=> nb.embedding::vector)::float8 AS distance
            FROM   covalence.node_embeddings seed
            JOIN   covalence.node_embeddings nb
                   ON nb.node_id <> seed.node_id
            JOIN   covalence.nodes n2
                   ON n2.id = nb.node_id
                  AND n2.node_type = 'claim'
                  AND n2.status    = 'active'
            WHERE  seed.node_id = $1
              AND  (seed.embedding::vector <=> nb.embedding::vector)::float8 < $2
            ORDER  BY (seed.embedding::vector <=> nb.embedding::vector)
            LIMIT  $3
            "#,
        )
        .bind(claim_id)
        .bind(threshold)
        .bind(HNSW_K)
        .fetch_all(pool)
        .await
        .context("dedup_claims: HNSW neighbour query failed")?;

        for nb_row in neighbours {
            let nb_id: Uuid = nb_row.try_get("neighbour_id")?;
            let nb_text: String = nb_row.try_get("neighbour_text").unwrap_or_default();
            let distance: f64 = nb_row.try_get("distance")?;
            let similarity = 1.0 - distance;

            // Normalise pair key so (A,B) == (B,A).
            let pair = if claim_id < nb_id {
                (claim_id, nb_id)
            } else {
                (nb_id, claim_id)
            };
            if !seen_pairs.insert(pair) {
                continue; // already recorded
            }

            if dry_run {
                candidates.push(DedupCandidate {
                    claim_a_id: claim_id,
                    claim_b_id: nb_id,
                    similarity,
                    text_a: claim_text.clone(),
                    text_b: nb_text,
                });
            } else {
                // ── Apply mode ────────────────────────────────────────────────
                // Determine canonical claim: whichever has more SUPPORTS_CLAIM edges.
                let (canonical_id, duplicate_id) =
                    pick_canonical(pool, claim_id, nb_id).await?;

                // Create SAME_AS edge (idempotent — skip if already exists).
                let inserted = sqlx::query(
                    r#"
                    INSERT INTO covalence.edges
                        (id, source_node_id, target_node_id, edge_type, weight,
                         metadata, created_at)
                    VALUES
                        (gen_random_uuid(), $1, $2, 'SAME_AS', 0.95,
                         $3::jsonb, now())
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(canonical_id)
                .bind(duplicate_id)
                .bind(json!({
                    "dedup_similarity": similarity,
                    "source": "dedup_claims_worker"
                }))
                .execute(pool)
                .await
                .context("dedup_claims: failed to insert SAME_AS edge")?;

                if inserted.rows_affected() > 0 {
                    edges_created += 1;
                    tracing::info!(
                        canonical_id = %canonical_id,
                        duplicate_id = %duplicate_id,
                        similarity   = similarity,
                        "dedup_claims: SAME_AS edge created"
                    );
                }
            }
        }
    }

    Ok(DedupResult {
        dry_run,
        threshold,
        claims_scanned,
        pairs_found: if dry_run {
            candidates.len()
        } else {
            seen_pairs.len()
        },
        edges_created,
        candidates,
    })
}

/// Return `(canonical_id, duplicate_id)` where the canonical claim is the one
/// with more `SUPPORTS_CLAIM` in-edges.  Ties are broken by creation order
/// (older claim wins).
async fn pick_canonical(
    pool: &PgPool,
    id_a: Uuid,
    id_b: Uuid,
) -> anyhow::Result<(Uuid, Uuid)> {
    let count_a = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM covalence.edges
         WHERE target_node_id = $1 AND edge_type = 'SUPPORTS_CLAIM'",
    )
    .bind(id_a)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let count_b = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM covalence.edges
         WHERE target_node_id = $1 AND edge_type = 'SUPPORTS_CLAIM'",
    )
    .bind(id_b)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    if count_b > count_a {
        Ok((id_b, id_a))
    } else {
        Ok((id_a, id_b))
    }
}

// ─── Worker task handler ──────────────────────────────────────────────────────

/// Handler for the `dedup_claims` queue task type (covalence#208).
///
/// Payload fields (all optional):
/// * `threshold` — cosine distance cutoff (default: env `COVALENCE_CLAIM_DEDUP_THRESHOLD` or 0.08)
/// * `dry_run`   — when `true` (default) returns candidates without writing edges
/// * `limit`     — maximum number of claim nodes to scan (default 500)
pub async fn handle_dedup_claims(pool: &PgPool, task: &QueueTask) -> anyhow::Result<Value> {
    let threshold = task
        .payload
        .get("threshold")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(effective_threshold)
        .clamp(0.0, 1.0);

    let dry_run = task
        .payload
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let limit = task
        .payload
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(DEFAULT_LIMIT);

    let result = run_dedup_claims(pool, threshold, dry_run, limit).await?;

    Ok(json!({
        "dry_run":        result.dry_run,
        "threshold":      result.threshold,
        "claims_scanned": result.claims_scanned,
        "pairs_found":    result.pairs_found,
        "edges_created":  result.edges_created,
        "candidates":     result.candidates,
    }))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_threshold_default() {
        // Without the env var the default is returned.
        std::env::remove_var("COVALENCE_CLAIM_DEDUP_THRESHOLD");
        assert!((effective_threshold() - DEFAULT_DEDUP_THRESHOLD).abs() < f64::EPSILON);
    }

    #[test]
    fn effective_threshold_from_env() {
        std::env::set_var("COVALENCE_CLAIM_DEDUP_THRESHOLD", "0.05");
        assert!((effective_threshold() - 0.05).abs() < f64::EPSILON);
        std::env::remove_var("COVALENCE_CLAIM_DEDUP_THRESHOLD");
    }

    #[test]
    fn effective_threshold_clamps_high() {
        std::env::set_var("COVALENCE_CLAIM_DEDUP_THRESHOLD", "5.0");
        assert!((effective_threshold() - 1.0).abs() < f64::EPSILON);
        std::env::remove_var("COVALENCE_CLAIM_DEDUP_THRESHOLD");
    }

    #[test]
    fn effective_threshold_clamps_low() {
        std::env::set_var("COVALENCE_CLAIM_DEDUP_THRESHOLD", "-1.0");
        assert!((effective_threshold() - 0.0).abs() < f64::EPSILON);
        std::env::remove_var("COVALENCE_CLAIM_DEDUP_THRESHOLD");
    }

    #[test]
    fn effective_threshold_invalid_falls_back() {
        std::env::set_var("COVALENCE_CLAIM_DEDUP_THRESHOLD", "not_a_number");
        assert!((effective_threshold() - DEFAULT_DEDUP_THRESHOLD).abs() < f64::EPSILON);
        std::env::remove_var("COVALENCE_CLAIM_DEDUP_THRESHOLD");
    }
}
