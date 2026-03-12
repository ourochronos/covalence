//! Deep consolidation service — wires graph algorithms to the DeepConsolidator trait.
//!
//! Performs: TrustRank recalibration, community detection, BMR forgetting,
//! cross-domain bridge discovery.

use crate::consolidation::deep::{DeepConfig, DeepConsolidator, DeepReport};
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::algorithms::{structural_importance, trust_rank};
use crate::graph::bridges::discover_bridges;
use crate::graph::community::detect_communities;

/// Deep consolidator that runs graph algorithms on the sidecar.
pub struct GraphDeepConsolidator {
    graph: SharedGraph,
}

impl GraphDeepConsolidator {
    /// Create a new deep consolidator.
    pub fn new(graph: SharedGraph) -> Self {
        Self { graph }
    }
}

#[async_trait::async_trait]
impl DeepConsolidator for GraphDeepConsolidator {
    async fn run_deep(&self, config: &DeepConfig) -> Result<DeepReport> {
        let g = self.graph.read().await;

        // Step 1: Identify seed nodes for TrustRank (nodes with clearance >= 1)
        let seeds: Vec<(uuid::Uuid, f64)> = g
            .graph()
            .node_indices()
            .filter_map(|idx| {
                let meta = &g.graph()[idx];
                if meta.clearance_level >= 1 {
                    Some((meta.id, 1.0))
                } else {
                    None
                }
            })
            .collect();

        // Step 2: TrustRank computation
        let trust_scores = if seeds.is_empty() {
            // No verified seeds — use all nodes with equal weight
            let all: Vec<_> = g
                .graph()
                .node_indices()
                .map(|idx| (g.graph()[idx].id, 1.0))
                .collect();
            trust_rank(g.graph(), &all, config.trust_rank_damping, 50)
        } else {
            trust_rank(g.graph(), &seeds, config.trust_rank_damping, 50)
        };

        // Step 3: Community detection
        let communities = detect_communities(g.graph());

        // Step 4: Structural importance (betweenness centrality)
        let importance = structural_importance(g.graph());

        // Step 5: BMR forgetting analysis
        use crate::epistemic::forgetting::{BmrWeights, NodeSignals, bmr_analysis, bmr_report};

        let mut node_signals = std::collections::HashMap::new();
        for idx in g.graph().node_indices() {
            let meta = &g.graph()[idx];
            let struct_imp = importance.get(&meta.id).copied().unwrap_or(0.0);
            let trust = trust_scores.get(&meta.id).copied().unwrap_or(0.0);

            node_signals.insert(
                meta.id,
                NodeSignals {
                    structural_importance: struct_imp,
                    // Without access logs, use trust as proxy for ACT-R base level
                    actr_base_level: trust * 5.0,
                    // Without provenance query, use clearance as proxy
                    accommodation_count: if meta.clearance_level > 0 { 3 } else { 1 },
                    contradiction_age_days: 0.0,
                    confidence: trust,
                },
            );
        }

        let decisions = bmr_analysis(
            &node_signals,
            &BmrWeights::default(),
            config.bmr_threshold,
            config.bmr_threshold * 3.0,
        );
        let report = bmr_report(&decisions);

        // Step 6: Cross-domain bridge discovery
        let bridges = discover_bridges(g.graph(), &communities);

        Ok(DeepReport {
            communities_found: communities.len(),
            nodes_forgotten: report.prune_count,
            trust_scores_updated: trust_scores.len(),
            bridges_found: bridges.len(),
        })
    }
}
