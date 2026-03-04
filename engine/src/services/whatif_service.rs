//! Non-destructive source retraction preview (covalence#119).
//!
//! `POST /admin/whatif/retract` wraps all DB reads in a transaction that is
//! always rolled back — modelled on the HypoPG "EXPLAIN without ANALYZE"
//! pattern.  No rows are ever modified.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::AppError;

// ── Request / Response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WhatifRetractRequest {
    pub source_id: Uuid,
}

/// Per-article impact of retracting the requested source.
#[derive(Debug, Serialize)]
pub struct WhatifArticleImpact {
    pub id: Uuid,
    pub title: String,
    /// `"survives"` | `"orphaned"` | `"degraded"`
    ///
    /// Survivability rule (TMS/ATMS academic consensus):
    /// An article **survives** iff it has ≥1 remaining provenance link
    /// (`ORIGINATES` or `CONFIRMS`) whose source node has `reliability > 0.3`.
    /// *orphaned*  — zero remaining provenance links.
    /// *degraded*  — remaining links exist but none has `reliability > 0.3`.
    pub survivability: String,
    /// Expected change in confidence score (negative or zero).
    pub confidence_delta: f64,
    /// Number of provenance links that would remain after retraction.
    pub remaining_source_count: i64,
}

/// Aggregate impact report returned by `POST /admin/whatif/retract`.
#[derive(Debug, Serialize)]
pub struct WhatifRetractResponse {
    pub affected_articles: Vec<WhatifArticleImpact>,
    pub orphaned_count: usize,
    pub degraded_count: usize,
    /// `true` iff **no** article would be orphaned or degraded by this
    /// retraction (i.e. every affected article survives).
    pub safe_to_remove: bool,
}

// ── Service ───────────────────────────────────────────────────────────────────

pub struct WhatifService {
    pool: PgPool,
}

impl WhatifService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Compute the impact of retracting `source_id` without modifying any
    /// rows.  Uses a transaction that is always rolled back.
    pub async fn retract_preview(
        &self,
        req: WhatifRetractRequest,
    ) -> Result<WhatifRetractResponse, AppError> {
        // Begin a transaction that we will ALWAYS roll back — no mutations.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

        // ── 1. Verify source exists ───────────────────────────────────────────
        let source_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM covalence.nodes
                WHERE id = $1 AND node_type = 'source'
            )",
        )
        .bind(req.source_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

        if !source_exists {
            tx.rollback()
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
            return Err(AppError::NotFound(format!(
                "source {} not found",
                req.source_id
            )));
        }

        // ── 2. Find articles directly linked to this source ───────────────────
        // Provenance edges go:  source_node_id → target_node_id (article)
        // with edge_type IN ('ORIGINATES', 'CONFIRMS').
        let linked_rows = sqlx::query(
            "SELECT DISTINCT n.id, COALESCE(n.title, '') AS title, n.confidence
             FROM covalence.edges e
             JOIN covalence.nodes n ON n.id = e.target_node_id
             WHERE e.source_node_id = $1
               AND e.edge_type IN ('ORIGINATES', 'CONFIRMS')
               AND n.node_type = 'article'
               AND n.status = 'active'",
        )
        .bind(req.source_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.into()))?;

        let mut affected_articles: Vec<WhatifArticleImpact> = Vec::with_capacity(linked_rows.len());

        // ── 3. For each article compute survivability ─────────────────────────
        for row in &linked_rows {
            let article_id: Uuid = row
                .try_get("id")
                .map_err(|e| AppError::Internal(e.into()))?;
            let title: String = row
                .try_get("title")
                .map_err(|e| AppError::Internal(e.into()))?;
            let current_confidence: f64 = row
                .try_get("confidence")
                .map_err(|e| AppError::Internal(e.into()))?;

            // Count and inspect REMAINING provenance links (exclude source being retracted).
            let remaining_rows = sqlx::query(
                "SELECT COALESCE(n.reliability, 0.5) AS reliability
                 FROM covalence.edges e
                 JOIN covalence.nodes n ON n.id = e.source_node_id
                 WHERE e.target_node_id = $1
                   AND e.edge_type IN ('ORIGINATES', 'CONFIRMS')
                   AND e.source_node_id != $2
                   AND n.node_type = 'source'",
            )
            .bind(article_id)
            .bind(req.source_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

            let remaining_source_count = remaining_rows.len() as i64;

            // Survivability rule (TMS/ATMS):
            let survivability = if remaining_source_count == 0 {
                "orphaned"
            } else {
                let has_reliable = remaining_rows.iter().any(|r| {
                    let rel: f64 = r.try_get("reliability").unwrap_or(0.5);
                    rel > 0.3
                });
                if has_reliable { "survives" } else { "degraded" }
            };

            // Confidence delta — estimate the drop in article confidence.
            //
            // Model: article confidence ≈ average reliability of its provenance
            // sources.  Removing source X and averaging the remainder gives a
            // new estimate; delta = new_avg − current_confidence.
            //
            // Edge cases:
            //   • If no remaining sources → delta = −current_confidence (drops to 0).
            //   • Null reliability defaults to 0.5.
            let confidence_delta = if remaining_source_count == 0 {
                -current_confidence
            } else {
                let remaining_avg: f64 = remaining_rows
                    .iter()
                    .map(|r| r.try_get::<f64, _>("reliability").unwrap_or(0.5))
                    .sum::<f64>()
                    / remaining_source_count as f64;
                (remaining_avg - current_confidence).min(0.0)
            };

            affected_articles.push(WhatifArticleImpact {
                id: article_id,
                title,
                survivability: survivability.to_string(),
                confidence_delta,
                remaining_source_count,
            });
        }

        // ── 4. ROLLBACK — never mutate ────────────────────────────────────────
        tx.rollback()
            .await
            .map_err(|e| AppError::Internal(e.into()))?;

        // ── 5. Aggregate counts ───────────────────────────────────────────────
        let orphaned_count = affected_articles
            .iter()
            .filter(|a| a.survivability == "orphaned")
            .count();
        let degraded_count = affected_articles
            .iter()
            .filter(|a| a.survivability == "degraded")
            .count();
        let safe_to_remove = orphaned_count == 0 && degraded_count == 0;

        Ok(WhatifRetractResponse {
            affected_articles,
            orphaned_count,
            degraded_count,
            safe_to_remove,
        })
    }
}
