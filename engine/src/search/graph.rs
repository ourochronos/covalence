//! Graph dimension adaptor — edge traversal from candidate anchors (SPEC §7.1, §7.2).
//!
//! Runs AFTER lexical+vector (cascade step 2). Traverses typed edges from
//! candidate anchor nodes to discover structurally related nodes.
//!
//! ## Multi-hop BFS (tracking#92 Phase B item #5)
//!
//! Delegates to the `covalence.graph_traverse` stored procedure (migration 035,
//! covalence#143).  The procedure executes a recursive CTE BFS from all anchor
//! nodes up to `max_hops` depth.  Each additional hop applies a score-decay
//! factor of 0.7 in Rust after the proc returns, so hop-2 results carry 70% of
//! the edge score and hop-3 results carry 49%.  When a node is reachable via
//! multiple paths, the highest decayed score is kept.
//!
//! ## Phase 7 — Intent-aware filtering (covalence#54)
//!
//! Intent is forwarded to `graph_traverse` as a text parameter.  The stored
//! procedure maps intent names ('factual' | 'temporal' | 'causal' | 'entity')
//! to their respective edge-type sets and filters traversal accordingly.
//! When intent is `None`, all edge types are traversed.
//!
//! ## SQL injection fix (covalence#144)
//!
//! All previous `format!()` string-interpolation was removed.  The proc call
//! uses fully typed `$1..$4` bind parameters — no dynamic SQL construction.
//!
//! ## COALESCE NULL fix (covalence#147)
//!
//! The stored procedure returns `COALESCE(e.causal_weight, 0.0)` so NULL edge
//! weights no longer silently produce wrong scores.

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};
use crate::models::SearchIntent;

/// Per-hop score-decay base.
const HOP_DECAY: f64 = 0.7;

/// Hard upper bound on `max_hops` accepted from callers.
const MAX_HOPS_LIMIT: u32 = 3;

// =============================================================================
// GraphHopRow — return type of covalence.graph_traverse(...)
// =============================================================================

/// A single row returned by `covalence.graph_traverse`.
///
/// Matches the `RETURNS TABLE` definition in migration 035:
/// `(node_id UUID, edge_id UUID, hop_depth INT, causal_weight FLOAT8, edge_type TEXT)`
#[derive(Debug, FromRow)]
pub struct GraphHopRow {
    /// Neighbor node reached at this hop.
    pub node_id: Uuid,
    /// Edge that was traversed to reach `node_id`.
    pub edge_id: Option<Uuid>,
    /// 1-based hop depth from any start node.
    pub hop_depth: i32,
    /// `COALESCE(e.causal_weight, 0.0)` — never NULL from the proc.
    pub causal_weight: Option<f64>,
    /// Edge type label (e.g. `"CONFIRMS"`, `"CAUSES"`).
    pub edge_type: Option<String>,
}

// =============================================================================
// GraphAdaptor
// =============================================================================

pub struct GraphAdaptor;

impl Default for GraphAdaptor {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphAdaptor {
    pub fn new() -> Self {
        Self
    }

    /// Map a [`SearchIntent`] to the intent-filter string accepted by the
    /// `covalence.graph_traverse` stored procedure.
    ///
    /// The mapping is intentionally kept in sync with `graph::memory::intent_edge_types`.
    fn intent_filter(intent: Option<&SearchIntent>) -> Option<&'static str> {
        match intent {
            Some(SearchIntent::Factual) => Some("factual"),
            Some(SearchIntent::Temporal) => Some("temporal"),
            Some(SearchIntent::Causal) => Some("causal"),
            Some(SearchIntent::Entity) => Some("entity"),
            None => None,
        }
    }
}

#[async_trait]
impl DimensionAdaptor for GraphAdaptor {
    fn name(&self) -> &'static str {
        "graph"
    }

    async fn check_availability(&self, pool: &PgPool) -> bool {
        // Check that the edges table exists
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables \
             WHERE table_schema = 'covalence' AND table_name = 'edges')",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(false)
    }

    /// Multi-hop BFS from anchor nodes via `covalence.graph_traverse`.
    ///
    /// The stored procedure handles cycle detection, namespace scoping, edge
    /// filtering, and COALESCE of NULL causal weights.  Rust post-processes the
    /// returned rows to apply per-hop score decay and deduplicate by node_id.
    ///
    /// * `max_hops` comes from `query.max_hops` (default 1, capped at 3).
    /// * Score: `decayed = causal_weight × 0.7^(hop_depth − 1)`.
    /// * If a node is reachable via multiple paths, the highest decayed score wins.
    /// * The `limit` applies to the total across all hops.
    async fn search(
        &self,
        pool: &PgPool,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> anyhow::Result<Vec<DimensionResult>> {
        let anchors = match candidates {
            Some(c) if !c.is_empty() => c,
            _ => return Ok(vec![]), // Graph requires anchor nodes from prior dimensions
        };

        let max_hops = query.max_hops.unwrap_or(1).clamp(1, MAX_HOPS_LIMIT) as i32;
        let min_weight = query.min_causal_weight.map(|w| w as f64).unwrap_or(0.0);
        let intent_filter = Self::intent_filter(query.intent.as_ref());

        // ── Single stored-proc call replaces the per-hop format!() BFS loop ──
        // Fixes covalence#144 (SQL injection) and covalence#147 (NULL weight).
        let rows = sqlx::query_as::<_, GraphHopRow>(
            "SELECT * FROM covalence.graph_traverse($1, $2, $3, $4)",
        )
        .bind(anchors)
        .bind(max_hops)
        .bind(min_weight)
        .bind(intent_filter)
        .fetch_all(pool)
        .await?;

        // ── Post-process: apply hop decay, deduplicate by node_id ─────────────
        // best[node_id] = (highest_decayed_score, hop_at_which_it_was_found)
        let mut best: HashMap<Uuid, (f64, u32)> = HashMap::new();

        for row in rows {
            let hop = row.hop_depth as u32;
            // Decay factor: 1.0 for hop 1, 0.7 for hop 2, 0.49 for hop 3.
            let decay = HOP_DECAY.powi(row.hop_depth - 1);
            let raw_score = row.causal_weight.unwrap_or(0.0);
            let decayed = raw_score * decay;

            let entry = best.entry(row.node_id).or_insert((decayed, hop));
            if decayed > entry.0 {
                *entry = (decayed, hop);
            }
        }

        // ── Assemble results ──────────────────────────────────────────────────
        let mut results: Vec<DimensionResult> = best
            .into_iter()
            .map(|(node_id, (score, hop))| DimensionResult {
                node_id,
                raw_score: score,
                normalized_score: 0.0,
                hop: Some(hop),
            })
            .collect();

        // Sort by raw score descending before truncating so the limit retains
        // the highest-scoring nodes across all hops.
        results.sort_by(|a, b| {
            b.raw_score
                .partial_cmp(&a.raw_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    fn normalize_scores(&self, results: &mut [DimensionResult]) {
        // Simple ratio normalization: normalized = raw_score / max_raw_score
        if results.is_empty() {
            return;
        }
        let max = results
            .iter()
            .map(|r| r.raw_score)
            .fold(f64::NEG_INFINITY, f64::max);
        if max > 0.0 {
            for r in results.iter_mut() {
                r.normalized_score = r.raw_score / max;
            }
        }
    }

    fn estimate_selectivity(&self, _query: &DimensionQuery) -> f64 {
        0.5 // depends on graph density
    }

    fn parallelizable(&self) -> bool {
        false // must run after lexical+vector (cascade step 2)
    }
}
