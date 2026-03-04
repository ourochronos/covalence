//! Graph dimension adaptor — edge traversal from candidate anchors (SPEC §7.1, §7.2).
//!
//! Runs AFTER lexical+vector (cascade step 2). Traverses typed edges from
//! candidate anchor nodes to discover structurally related nodes.
//!
//! ## Multi-hop BFS (tracking#92 Phase B item #5)
//!
//! Iterative breadth-first search from the anchor set up to `max_hops` (1–3).
//! Each additional hop applies a score-decay factor of 0.7, so hop-2 results
//! carry 70% of the edge score and hop-3 results carry 49%.  Cycle detection
//! via a visited `HashSet` prevents revisiting the same node across hops.
//! When a node is reachable via multiple paths, the highest score is kept.
//!
//! ## Phase 7 — Intent-aware filtering (covalence#54)
//!
//! `priority_edges()` now delegates to the shared `intent_edge_types()` mapping
//! in `graph::memory`, eliminating duplication between the DB adaptor and the
//! in-memory graph layer.
//!
//! ## Causal metadata filters (covalence#116)
//!
//! When `min_causal_strength`, `causal_level`, or `evidence_types` are set on
//! the query, the graph traversal LEFT-JOINs `edge_causal_metadata` and applies
//! the filters.  Edges whose enrichment row is absent (NULL from the LEFT JOIN)
//! are excluded when any of these filters is active.
//!
//! For `intent = causal`, the edge score is replaced with the Pearl composite:
//! ```text
//! causal_score = COALESCE(ecm.causal_strength, e.causal_weight)
//!              × COALESCE(ecm.direction_conf, 0.5)
//!              × (1.0 − COALESCE(ecm.hidden_conf_risk, 0.5))
//! ```

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::dimension::{DimensionAdaptor, DimensionQuery, DimensionResult};
use crate::graph::intent_edge_types;
use crate::models::{CausalEvidenceType, CausalLevel, SearchIntent};

/// Per-hop score-decay base.
const HOP_DECAY: f64 = 0.7;

/// Hard upper bound on `max_hops` accepted from callers.
const MAX_HOPS_LIMIT: u32 = 3;

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

    /// Intent → priority edge types (SPEC §7.1 table).
    ///
    /// Delegates to the shared [`intent_edge_types`] mapping in
    /// `graph::memory` so the DB adaptor and in-memory graph layer stay
    /// in sync (Phase 7, covalence#54).
    ///
    /// Returns an empty `Vec` when `intent` is `None`, which causes
    /// [`fetch_frontier_neighbors`] to traverse all edge types.
    fn priority_edges(intent: Option<&SearchIntent>) -> Vec<String> {
        match intent {
            Some(i) => intent_edge_types(i),
            None => vec![], // all edges, no filter
        }
    }

    /// Build optional SQL clauses for causal-metadata filters (covalence#116).
    ///
    /// Returns `(join_clause, where_clauses)`:
    /// * `join_clause` — a `LEFT JOIN` that should follow the `FROM covalence.edges e` clause.
    /// * `where_clauses` — zero or more `AND …` predicates.
    ///
    /// When any filter is active the join is included; when no filters are set
    /// both strings are empty (backward-compatible fast path).
    fn causal_meta_clauses(
        min_causal_strength: Option<f64>,
        causal_level: Option<CausalLevel>,
        evidence_types: Option<&Vec<CausalEvidenceType>>,
        is_causal_intent: bool,
    ) -> (String, String) {
        let any_filter = min_causal_strength.is_some()
            || causal_level.is_some()
            || evidence_types.map(|v| !v.is_empty()).unwrap_or(false);

        // Always join when causal intent so we can compute the composite score
        // even without explicit filters.
        let need_join = any_filter || is_causal_intent;

        if !need_join {
            return (String::new(), String::new());
        }

        let join = "LEFT JOIN covalence.edge_causal_metadata ecm ON e.id = ecm.edge_id".to_string();

        let mut where_parts: Vec<String> = Vec::new();

        if let Some(min_cs) = min_causal_strength {
            // NULL ecm row → no metadata → exclude when filter is active.
            where_parts.push(format!(
                "AND ecm.causal_strength IS NOT NULL AND ecm.causal_strength >= {min_cs}"
            ));
        }

        if let Some(ref level) = causal_level {
            let level_str = match level {
                CausalLevel::Association => "association",
                CausalLevel::Intervention => "intervention",
                CausalLevel::Counterfactual => "counterfactual",
            };
            where_parts.push(format!(
                "AND ecm.causal_level IS NOT NULL AND ecm.causal_level = '{level_str}'"
            ));
        }

        if let Some(types) = evidence_types {
            if !types.is_empty() {
                let list = types
                    .iter()
                    .map(|t| {
                        let s = match t {
                            CausalEvidenceType::StructuralPrior => "structural_prior",
                            CausalEvidenceType::ExpertAssertion => "expert_assertion",
                            CausalEvidenceType::Statistical => "statistical",
                            CausalEvidenceType::Experimental => "experimental",
                            CausalEvidenceType::GrangerTemporal => "granger_temporal",
                            CausalEvidenceType::LlmExtracted => "llm_extracted",
                            CausalEvidenceType::DomainRule => "domain_rule",
                        };
                        format!("'{s}'")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                where_parts.push(format!(
                    "AND ecm.evidence_type IS NOT NULL AND ecm.evidence_type IN ({list})"
                ));
            }
        }

        (join, where_parts.join("\n                          "))
    }

    /// Compute the score expression for a given intent.
    ///
    /// For `causal` intent: Pearl composite score using `edge_causal_metadata`
    /// columns.  Falls back gracefully to the edge's own `causal_weight` when
    /// the enrichment row is absent.
    ///
    /// For all other intents: the standard `weight * confidence` product.
    fn score_expr(is_causal_intent: bool, has_priority: bool) -> String {
        let base = if is_causal_intent {
            // Pearl composite: causal_strength × direction_conf × (1 − hidden_conf_risk)
            "(COALESCE(ecm.causal_strength, e.causal_weight) \
               * COALESCE(ecm.direction_conf, 0.5) \
               * (1.0 - COALESCE(ecm.hidden_conf_risk, 0.5)))::float8"
                .to_string()
        } else {
            "(e.weight * e.confidence)::float8".to_string()
        };

        if has_priority {
            format!("({base} * CASE WHEN e.edge_type = ANY($3) THEN 2.0 ELSE 1.0 END)::float8")
        } else {
            base
        }
    }

    /// Fetch all active neighbors of `frontier` nodes with their raw edge scores.
    ///
    /// Returns `(neighbor_id, raw_score)` pairs (before hop-decay is applied).
    /// Priority edge types are boosted 2× when `priority` is non-empty.
    /// The caller is responsible for filtering already-visited nodes.
    async fn fetch_frontier_neighbors(
        pool: &PgPool,
        frontier: &[Uuid],
        priority: &[String],
        namespace: &str,
        min_causal_weight: Option<f32>,
        min_causal_strength: Option<f64>,
        causal_level: Option<CausalLevel>,
        evidence_types: Option<&Vec<CausalEvidenceType>>,
        is_causal_intent: bool,
    ) -> anyhow::Result<Vec<(Uuid, f64)>> {
        if frontier.is_empty() {
            return Ok(vec![]);
        }

        // Build the causal_weight filter clause. When min_causal_weight is set
        // we add a literal comparison; otherwise the clause is empty.
        //
        // We use inline SQL rather than a bound parameter for this optional
        // clause because sqlx does not support conditional bind counts cleanly.
        let causal_filter = match min_causal_weight {
            Some(w) => format!("AND e.causal_weight >= {w}"),
            None => String::new(),
        };

        // Build causal metadata JOIN and WHERE clauses (covalence#116).
        let (ecm_join, ecm_where) = Self::causal_meta_clauses(
            min_causal_strength,
            causal_level,
            evidence_types,
            is_causal_intent,
        );

        let has_priority = !priority.is_empty();
        let score_expr = Self::score_expr(is_causal_intent, has_priority);

        let rows = if !has_priority {
            // No intent filter — all edge types.
            sqlx::query_as::<_, (Uuid, f64)>(&format!(
                "SELECT DISTINCT ON (neighbor_id) neighbor_id, score FROM (
                        SELECT
                            CASE WHEN e.source_node_id = ANY($1) THEN e.target_node_id
                                 ELSE e.source_node_id END                           AS neighbor_id,
                            {score_expr}                                              AS score
                        FROM covalence.edges e
                        {ecm_join}
                        WHERE (e.source_node_id = ANY($1) OR e.target_node_id = ANY($1))
                          AND e.namespace = $2
                          AND e.valid_to IS NULL
                          {causal_filter}
                          {ecm_where}
                    ) sub
                    JOIN covalence.nodes n ON n.id = sub.neighbor_id
                    WHERE n.status = 'active' AND n.namespace = $2
                    ORDER BY neighbor_id, score DESC"
            ))
            .bind(frontier)
            .bind(namespace)
            .fetch_all(pool)
            .await?
        } else {
            // Intent-filtered: boost priority edge types by 2×.
            sqlx::query_as::<_, (Uuid, f64)>(&format!(
                "SELECT DISTINCT ON (neighbor_id) neighbor_id, score FROM (
                        SELECT
                            CASE WHEN e.source_node_id = ANY($1) THEN e.target_node_id
                                 ELSE e.source_node_id END                           AS neighbor_id,
                            {score_expr}                                              AS score
                        FROM covalence.edges e
                        {ecm_join}
                        WHERE (e.source_node_id = ANY($1) OR e.target_node_id = ANY($1))
                          AND e.namespace = $2
                          AND e.valid_to IS NULL
                          {causal_filter}
                          {ecm_where}
                    ) sub
                    JOIN covalence.nodes n ON n.id = sub.neighbor_id
                    WHERE n.status = 'active' AND n.namespace = $2
                    ORDER BY neighbor_id, score DESC"
            ))
            .bind(frontier)
            .bind(namespace)
            .bind(priority)
            .fetch_all(pool)
            .await?
        };

        Ok(rows)
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

    /// Multi-hop BFS from anchor nodes.
    ///
    /// * `max_hops` comes from `query.max_hops` (default 1, capped at 3).
    /// * Score decay: `hop_score = edge_score × 0.7^(hop - 1)`.
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

        let priority = Self::priority_edges(query.intent.as_ref());
        let max_hops = query.max_hops.unwrap_or(1).clamp(1, MAX_HOPS_LIMIT);
        let min_causal_weight = query.min_causal_weight;
        let is_causal_intent = matches!(query.intent, Some(SearchIntent::Causal));

        // ── BFS state ──────────────────────────────────────────────────────────
        // `visited` starts with all anchor nodes so the BFS never returns them.
        let mut visited: HashSet<Uuid> = anchors.iter().cloned().collect();
        // best[node] = (highest_decayed_score, hop_at_which_it_was_found)
        let mut best: HashMap<Uuid, (f64, u32)> = HashMap::new();
        let mut frontier: Vec<Uuid> = anchors.to_vec();

        for hop in 1..=max_hops {
            if frontier.is_empty() {
                break;
            }

            // Decay factor for this hop: 1.0 for hop 1, 0.7 for hop 2, 0.49 for hop 3.
            let decay = HOP_DECAY.powi((hop as i32) - 1);

            let rows = Self::fetch_frontier_neighbors(
                pool,
                &frontier,
                &priority,
                &query.namespace,
                min_causal_weight,
                query.min_causal_strength,
                query.causal_level,
                query.evidence_types.as_ref(),
                is_causal_intent,
            )
            .await?;

            let mut next_frontier: Vec<Uuid> = Vec::new();

            for (neighbor_id, raw_score) in rows {
                let decayed = raw_score * decay;

                // Deduplication: always keep the highest score seen so far.
                let entry = best.entry(neighbor_id).or_insert((decayed, hop));
                if decayed > entry.0 {
                    *entry = (decayed, hop);
                }

                // Cycle detection: only add to next frontier if not yet visited.
                if !visited.contains(&neighbor_id) {
                    visited.insert(neighbor_id);
                    next_frontier.push(neighbor_id);
                }
            }

            frontier = next_frontier;
        }

        // ── Assemble results ───────────────────────────────────────────────────
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
        // path_cost normalization: score = 1.0 / (1.0 + path_cost)
        // Since raw_score = weight * confidence (higher = better), invert:
        // normalized = raw_score / max_raw_score (simple ratio normalization)
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
