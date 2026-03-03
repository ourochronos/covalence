//! Cross-Dimensional Divergence Detector (covalence#58).
//!
//! Periodic scan that computes cross-dimensional divergence for every active
//! node that holds both a content embedding (`node_embeddings`) and a graph
//! embedding (`graph_embeddings`).
//!
//! ## Algorithm
//!
//! For each eligible node N:
//! 1. Collect its immediate graph neighbors (up to [`NEIGHBOR_LIMIT`]).
//! 2. Compute **vector_score**: average cosine similarity between N's content
//!    embedding and each neighbor's content embedding.
//! 3. Compute **structural_score**: average cosine similarity between N's graph
//!    embedding and each neighbor's graph embedding (prefers `node2vec`,
//!    falls back to `spectral`).
//! 4. `divergence_score = |vector_score − structural_score|`
//! 5. If `divergence_score >= threshold`, classify the anomaly:
//!    * `vector_score > structural_score` → **fragmented** (semantically similar
//!      but structurally disconnected)
//!    * `structural_score >= vector_score` → **structural_twin** (same graph
//!      position, very different content)
//!
//! Results are written to each node's `metadata.divergence_flags` JSONB field
//! and returned from the HTTP endpoints.

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::worker::QueueTask;

// ─── Constants ─────────────────────────────────────────────────────────────────

/// Default divergence threshold; nodes above this are flagged.
pub const DEFAULT_DIVERGENCE_THRESHOLD: f64 = 0.5;

/// Maximum neighbors to sample per node.
const NEIGHBOR_LIMIT: i64 = 10;

/// Maximum nodes to evaluate per scan run (prevents runaway on large graphs).
const SCAN_LIMIT: i64 = 500;

// ─── Result types ──────────────────────────────────────────────────────────────

/// A single flagged anomaly produced by the divergence scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DivergenceAnomaly {
    /// UUID of the divergent node.
    pub node_id: Uuid,
    /// Human-readable title (may be `None` for untitled nodes).
    pub title: Option<String>,
    /// Classification: `"fragmented"` or `"structural_twin"`.
    #[serde(rename = "type")]
    pub anomaly_type: String,
    /// Absolute difference between vector and structural scores.
    pub divergence_score: f64,
    /// Raw per-dimension scores that led to the classification.
    pub details: DivergenceDetails,
}

/// Per-dimension similarity averages for a flagged node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DivergenceDetails {
    /// Average cosine similarity to graph neighbors via content embeddings.
    pub vector_score: f64,
    /// Average cosine similarity to graph neighbors via graph embeddings.
    pub structural_score: f64,
}

/// Summary returned by [`run_divergence_scan`] and [`read_divergence_report`].
#[derive(Debug, Serialize)]
pub struct DivergenceScanResult {
    /// Total nodes evaluated during this scan.
    pub scanned: usize,
    /// Number of nodes that exceeded the divergence threshold.
    pub flagged: usize,
    /// Detailed anomaly records, sorted by `divergence_score` descending.
    pub anomalies: Vec<DivergenceAnomaly>,
}

// ─── Core classification logic ────────────────────────────────────────────────

/// Classify a node given its average per-dimension similarity scores.
///
/// Returns `None` when `divergence_score < threshold` (node is healthy).
/// Returns the anomaly type string otherwise.
///
/// Classification rules:
/// * `vector_score > structural_score` → `"fragmented"` — content similar to
///   neighbors but structurally distant (semantically coupled, graphically isolated).
/// * `structural_score >= vector_score` → `"structural_twin"` — graph-position
///   identical to neighbors but content diverges (positional twins, content strangers).
pub fn classify_anomaly(
    vector_score: f64,
    structural_score: f64,
    threshold: f64,
) -> Option<String> {
    let divergence = (vector_score - structural_score).abs();
    if divergence < threshold {
        return None;
    }
    if vector_score > structural_score {
        Some("fragmented".to_string())
    } else {
        Some("structural_twin".to_string())
    }
}

// ─── Scan ─────────────────────────────────────────────────────────────────────

/// Run a full divergence scan and persist flags to node metadata.
///
/// `threshold` — divergence score required to flag a node (default [`DEFAULT_DIVERGENCE_THRESHOLD`]).
///
/// Nodes without neighbors are counted in `scanned` but never flagged because
/// divergence is undefined for isolated nodes.
pub async fn run_divergence_scan(
    pool: &PgPool,
    threshold: f64,
) -> anyhow::Result<DivergenceScanResult> {
    use sqlx::Row as _;

    // ── Step 1: candidates ─────────────────────────────────────────────────────
    // Only nodes with BOTH a content embedding AND a graph embedding are eligible.
    let candidates = sqlx::query(
        "SELECT n.id, n.title
         FROM   covalence.nodes n
         WHERE  n.status = 'active'
           AND  EXISTS (SELECT 1 FROM covalence.node_embeddings  ne WHERE ne.node_id = n.id)
           AND  EXISTS (SELECT 1 FROM covalence.graph_embeddings ge WHERE ge.node_id = n.id)
         ORDER  BY n.created_at DESC
         LIMIT  $1",
    )
    .bind(SCAN_LIMIT)
    .fetch_all(pool)
    .await
    .context("divergence_scan: failed to fetch candidate nodes")?;

    let mut scanned = 0usize;
    let mut anomalies: Vec<DivergenceAnomaly> = Vec::new();

    for row in &candidates {
        let node_id: Uuid = row.get("id");
        let title: Option<String> = row.get("title");

        // ── Step 2: collect neighbors ──────────────────────────────────────────
        let neighbors: Vec<Uuid> = sqlx::query_scalar::<_, Uuid>(
            "SELECT target_node_id AS neighbor
             FROM   covalence.edges
             WHERE  source_node_id = $1
             UNION
             SELECT source_node_id
             FROM   covalence.edges
             WHERE  target_node_id = $1
             LIMIT  $2",
        )
        .bind(node_id)
        .bind(NEIGHBOR_LIMIT)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        scanned += 1;

        if neighbors.is_empty() {
            // Isolated node — write a neutral flag and continue.
            let flag = json!({
                "divergence_score": 0.0,
                "vector_score":     0.0,
                "structural_score": 0.0,
                "scanned_at":       chrono::Utc::now().to_rfc3339(),
                "flagged":          false,
                "isolated":         true,
            });
            let _ = write_divergence_flag(pool, node_id, &flag).await;
            continue;
        }

        // ── Step 3: vector similarity (content embeddings) ────────────────────
        // Average cosine similarity to all neighbors that have node_embeddings.
        // 1.0 - cosine_distance converts distance → similarity.
        let vector_sim: f64 = sqlx::query_scalar::<_, f64>(
            "SELECT COALESCE(
                AVG((1.0 - (ne_n.embedding::vector <=> ne_self.embedding::vector))::float8),
                0.0
             )
             FROM   covalence.node_embeddings ne_self
             JOIN   covalence.node_embeddings ne_n ON ne_n.node_id = ANY($2)
             WHERE  ne_self.node_id = $1",
        )
        .bind(node_id)
        .bind(&neighbors)
        .fetch_one(pool)
        .await
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);

        // ── Step 4: structural similarity (graph embeddings) ──────────────────
        // Prefer node2vec method; fall back to spectral if unavailable.
        let structural_sim: f64 = sqlx::query_scalar::<_, f64>(
            "SELECT COALESCE(
                AVG((1.0 - (ge_n.embedding <=> ge_self.embedding))::float8),
                0.0
             )
             FROM   covalence.graph_embeddings ge_self
             JOIN   covalence.graph_embeddings ge_n
                ON  ge_n.node_id = ANY($2)
               AND  ge_n.method  = ge_self.method
             WHERE  ge_self.node_id = $1
               AND  ge_self.method  = (
                     SELECT method
                     FROM   covalence.graph_embeddings
                     WHERE  node_id = $1
                     ORDER  BY CASE method WHEN 'node2vec' THEN 0 ELSE 1 END
                     LIMIT  1
                   )",
        )
        .bind(node_id)
        .bind(&neighbors)
        .fetch_one(pool)
        .await
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);

        // ── Step 5: classify ──────────────────────────────────────────────────
        let divergence_score = (vector_sim - structural_sim).abs();
        let anomaly_type_opt = classify_anomaly(vector_sim, structural_sim, threshold);
        let is_flagged = anomaly_type_opt.is_some();

        if let Some(ref anomaly_type) = anomaly_type_opt {
            anomalies.push(DivergenceAnomaly {
                node_id,
                title: title.clone(),
                anomaly_type: anomaly_type.clone(),
                divergence_score,
                details: DivergenceDetails {
                    vector_score: vector_sim,
                    structural_score: structural_sim,
                },
            });
        }

        // ── Step 6: persist flag to node metadata ─────────────────────────────
        let flag = json!({
            "divergence_score": divergence_score,
            "vector_score":     vector_sim,
            "structural_score": structural_sim,
            "anomaly_type":     anomaly_type_opt,
            "scanned_at":       chrono::Utc::now().to_rfc3339(),
            "flagged":          is_flagged,
        });
        let _ = write_divergence_flag(pool, node_id, &flag).await;
    }

    // Sort anomalies descending by divergence_score for easy triage.
    anomalies.sort_by(|a, b| {
        b.divergence_score
            .partial_cmp(&a.divergence_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    tracing::info!(
        scanned,
        flagged = anomalies.len(),
        threshold,
        "divergence_scan: complete"
    );

    Ok(DivergenceScanResult {
        scanned,
        flagged: anomalies.len(),
        anomalies,
    })
}

/// Persist `divergence_flags` to a node's metadata JSONB column.
async fn write_divergence_flag(pool: &PgPool, node_id: Uuid, flag: &Value) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE covalence.nodes
           SET metadata = jsonb_set(
                              coalesce(metadata, '{}'::jsonb),
                              '{divergence_flags}',
                              $1,
                              true
                          ),
               modified_at = now()
           WHERE id = $2"#,
    )
    .bind(flag)
    .bind(node_id)
    .execute(pool)
    .await
    .with_context(|| format!("write_divergence_flag: failed for node {node_id}"))?;
    Ok(())
}

// ─── Report (read from metadata, no re-scan) ──────────────────────────────────

/// Return the most recent divergence scan results stored in node metadata,
/// without triggering a new scan.
///
/// Only returns nodes whose metadata records `flagged = true`.
pub async fn read_divergence_report(pool: &PgPool) -> anyhow::Result<DivergenceScanResult> {
    use sqlx::Row as _;

    // Flagged anomalies sorted by divergence_score descending.
    let rows = sqlx::query(
        "SELECT id,
                title,
                metadata->'divergence_flags' AS flags
         FROM   covalence.nodes
         WHERE  status = 'active'
           AND  metadata->'divergence_flags' IS NOT NULL
           AND  (metadata->'divergence_flags'->>'flagged')::boolean = true
         ORDER  BY (metadata->'divergence_flags'->>'divergence_score')::float8 DESC
         LIMIT  200",
    )
    .fetch_all(pool)
    .await
    .context("read_divergence_report: failed to query flagged nodes")?;

    let mut anomalies: Vec<DivergenceAnomaly> = Vec::new();

    for row in &rows {
        let node_id: Uuid = row.get("id");
        let title: Option<String> = row.get("title");
        let flags: Option<Value> = row.get("flags");

        if let Some(flags) = flags {
            let divergence_score = flags
                .get("divergence_score")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let vector_score = flags
                .get("vector_score")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let structural_score = flags
                .get("structural_score")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            // Re-classify from stored scores (or use the stored anomaly_type).
            let anomaly_type = flags
                .get("anomaly_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    classify_anomaly(vector_score, structural_score, DEFAULT_DIVERGENCE_THRESHOLD)
                })
                .unwrap_or_else(|| "unknown".to_string());

            anomalies.push(DivergenceAnomaly {
                node_id,
                title,
                anomaly_type,
                divergence_score,
                details: DivergenceDetails {
                    vector_score,
                    structural_score,
                },
            });
        }
    }

    // Total nodes that were scanned (have any divergence_flags entry).
    let scanned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM   covalence.nodes
         WHERE  status = 'active'
           AND  metadata->'divergence_flags' IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(DivergenceScanResult {
        scanned: scanned as usize,
        flagged: anomalies.len(),
        anomalies,
    })
}

// ─── Worker task handler ──────────────────────────────────────────────────────

/// Handler for the `divergence_scan` queue task type.
///
/// Payload: `{ "threshold": 0.5 }` (all fields optional).
pub async fn handle_divergence_scan(pool: &PgPool, task: &QueueTask) -> anyhow::Result<Value> {
    let threshold = task
        .payload
        .get("threshold")
        .and_then(|v| v.as_f64())
        .unwrap_or(DEFAULT_DIVERGENCE_THRESHOLD)
        .clamp(0.0, 1.0);

    let result = run_divergence_scan(pool, threshold).await?;

    Ok(json!({
        "scanned":   result.scanned,
        "flagged":   result.flagged,
        "anomalies": result.anomalies,
        "threshold": threshold,
    }))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_anomaly ──────────────────────────────────────────────────────

    #[test]
    fn test_no_divergence_equal_scores() {
        // Both dimensions identical → divergence = 0 → no flag.
        assert!(classify_anomaly(0.6, 0.6, 0.5).is_none());
    }

    #[test]
    fn test_no_divergence_below_threshold() {
        // |0.65 − 0.35| = 0.30 < 0.50 → no flag.
        assert!(classify_anomaly(0.65, 0.35, 0.5).is_none());
    }

    #[test]
    fn test_classify_fragmented() {
        // High vector, low structural → "fragmented".
        // |0.8 − 0.1| = 0.7 ≥ 0.5 and vector > structural.
        let result = classify_anomaly(0.8, 0.1, 0.5);
        assert_eq!(result, Some("fragmented".to_string()));
    }

    #[test]
    fn test_classify_structural_twin() {
        // High structural, low vector → "structural_twin".
        // |0.1 − 0.8| = 0.7 ≥ 0.5 and structural > vector.
        let result = classify_anomaly(0.1, 0.8, 0.5);
        assert_eq!(result, Some("structural_twin".to_string()));
    }

    #[test]
    fn test_threshold_exactly_met_is_flagged() {
        // |0.75 − 0.25| = 0.50 == threshold → flagged (divergence >= threshold).
        let result = classify_anomaly(0.75, 0.25, 0.5);
        assert!(result.is_some(), "exact threshold hit should be flagged");
    }

    #[test]
    fn test_threshold_just_below_not_flagged() {
        // |0.74 − 0.25| = 0.49 < 0.50 → not flagged.
        assert!(classify_anomaly(0.74, 0.25, 0.5).is_none());
    }

    #[test]
    fn test_custom_threshold_tight() {
        // With a tight threshold of 0.1, even small divergence flags the node.
        let result = classify_anomaly(0.6, 0.4, 0.1);
        assert!(result.is_some());
    }

    #[test]
    fn test_custom_threshold_relaxed() {
        // With a very relaxed threshold of 0.9, a divergence of 0.7 is fine.
        assert!(classify_anomaly(0.8, 0.1, 0.9).is_none());
    }

    // ── divergence_score arithmetic ───────────────────────────────────────────

    #[test]
    fn test_divergence_score_symmetric() {
        // |a − b| == |b − a| — order doesn't change magnitude.
        let a = 0.8_f64;
        let b = 0.2_f64;
        let d1 = (a - b).abs();
        let d2 = (b - a).abs();
        assert!((d1 - d2).abs() < 1e-10);
    }

    #[test]
    fn test_high_divergence_score_exceeds_threshold() {
        let vector_score = 0.85_f64;
        let structural_score = 0.10_f64;
        let divergence = (vector_score - structural_score).abs();
        assert!(
            divergence >= DEFAULT_DIVERGENCE_THRESHOLD,
            "divergence {divergence} should exceed {DEFAULT_DIVERGENCE_THRESHOLD}"
        );
    }

    #[test]
    fn test_low_divergence_score_below_threshold() {
        let vector_score = 0.55_f64;
        let structural_score = 0.50_f64;
        let divergence = (vector_score - structural_score).abs();
        assert!(
            divergence < DEFAULT_DIVERGENCE_THRESHOLD,
            "divergence {divergence} should be below {DEFAULT_DIVERGENCE_THRESHOLD}"
        );
    }
}
