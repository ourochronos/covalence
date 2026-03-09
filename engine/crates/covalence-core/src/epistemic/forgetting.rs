//! Bayesian Model Reduction — principled forgetting.
//!
//! Forgetting is a mathematically principled optimization that increases model
//! evidence by reducing complexity (Friston & Zeidman 2018).
//!
//! An artifact can be safely forgotten when its `keep_score < threshold`,
//! meaning the posterior belief is indistinguishable from its prior — no
//! learning occurred.
//!
//! Three-tier eviction priority:
//! 1. Prune first — artifacts where posterior ≈ prior (no information gained)
//! 2. Prune second — low structural importance × low trust × low recency
//! 3. Archive, do not prune — high structural importance OR high corroboration

use std::collections::HashMap;

use uuid::Uuid;

/// Decision from the BMR analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvictionDecision {
    /// Keep the artifact — it has sufficient value.
    Retain,
    /// Archive the artifact — it has structural importance but low activity.
    Archive,
    /// Prune the artifact — it provides negligible model evidence.
    Prune,
}

/// Weights for the BMR keep score computation.
///
/// `keep_score = w1*structural_importance + w2*actr_base_level
///             + w3*accommodation_count - w4*contradiction_age`
#[derive(Debug, Clone)]
pub struct BmrWeights {
    /// Weight for structural importance (betweenness centrality).
    pub structural: f64,
    /// Weight for ACT-R base level activation.
    pub actr: f64,
    /// Weight for accommodation count (how many sources support this).
    pub accommodation: f64,
    /// Penalty weight for contradiction age.
    pub contradiction_penalty: f64,
}

impl Default for BmrWeights {
    fn default() -> Self {
        Self {
            structural: 0.3,
            actr: 0.3,
            accommodation: 0.25,
            contradiction_penalty: 0.15,
        }
    }
}

/// Input signals for computing a node's keep score.
#[derive(Debug, Clone)]
pub struct NodeSignals {
    /// Structural importance (betweenness centrality), normalized to [0, 1].
    pub structural_importance: f64,
    /// ACT-R base level activation: `ln(sum(t_k^{-0.5}))`.
    pub actr_base_level: f64,
    /// Number of sources that corroborate this node.
    pub accommodation_count: u32,
    /// Age of the oldest unresolved contradiction (in days). 0 if none.
    pub contradiction_age_days: f64,
    /// Projected probability from the node's opinion tuple.
    pub confidence: f64,
}

/// Compute the ACT-R base level activation.
///
/// `B_i = ln(sum(t_k^{-0.5}))` where `t_k` is the time since the k-th
/// access in days. Combines access count, recency, and spacing.
pub fn actr_base_level(access_times_days_ago: &[f64]) -> f64 {
    if access_times_days_ago.is_empty() {
        return -10.0; // Very low activation for never-accessed items
    }

    let sum: f64 = access_times_days_ago
        .iter()
        .map(|&t| {
            let t = t.max(0.001); // Avoid division by zero
            t.powf(-0.5)
        })
        .sum();

    if sum > 0.0 { sum.ln() } else { -10.0 }
}

/// Compute the BMR keep score for a node.
///
/// Higher scores mean the node should be retained. Scores below `threshold`
/// indicate the node is a candidate for pruning or archival.
pub fn keep_score(signals: &NodeSignals, weights: &BmrWeights) -> f64 {
    let raw = weights.structural * signals.structural_importance
        + weights.actr * signals.actr_base_level.clamp(0.0, 5.0) / 5.0 // normalize ACT-R to [0,1]
        + weights.accommodation * (signals.accommodation_count as f64).min(10.0) / 10.0 // normalize
        - weights.contradiction_penalty * (signals.contradiction_age_days / 365.0).min(1.0);

    raw.clamp(0.0, 1.0)
}

/// Decide whether to retain, archive, or prune a node.
pub fn eviction_decision(
    signals: &NodeSignals,
    weights: &BmrWeights,
    prune_threshold: f64,
    archive_threshold: f64,
) -> EvictionDecision {
    let score = keep_score(signals, weights);

    // High structural importance always archives (never prunes)
    if signals.structural_importance > 0.5 {
        if score < archive_threshold {
            return EvictionDecision::Archive;
        }
        return EvictionDecision::Retain;
    }

    // High corroboration also protects from pruning
    if signals.accommodation_count >= 3 {
        if score < archive_threshold {
            return EvictionDecision::Archive;
        }
        return EvictionDecision::Retain;
    }

    if score < prune_threshold {
        EvictionDecision::Prune
    } else if score < archive_threshold {
        EvictionDecision::Archive
    } else {
        EvictionDecision::Retain
    }
}

/// Run BMR analysis across all nodes, using precomputed structural importance.
///
/// Returns a map of node UUID to eviction decision.
pub fn bmr_analysis(
    node_signals: &HashMap<Uuid, NodeSignals>,
    weights: &BmrWeights,
    prune_threshold: f64,
    archive_threshold: f64,
) -> HashMap<Uuid, EvictionDecision> {
    node_signals
        .iter()
        .map(|(id, signals)| {
            (
                *id,
                eviction_decision(signals, weights, prune_threshold, archive_threshold),
            )
        })
        .collect()
}

/// Summary of a BMR forgetting pass.
#[derive(Debug, Clone)]
pub struct BmrReport {
    /// Total nodes analyzed.
    pub total_analyzed: usize,
    /// Nodes marked for pruning.
    pub prune_count: usize,
    /// Nodes marked for archival.
    pub archive_count: usize,
    /// Nodes retained.
    pub retain_count: usize,
}

/// Produce a summary report from BMR decisions.
pub fn bmr_report(decisions: &HashMap<Uuid, EvictionDecision>) -> BmrReport {
    let mut prune = 0;
    let mut archive = 0;
    let mut retain = 0;

    for decision in decisions.values() {
        match decision {
            EvictionDecision::Prune => prune += 1,
            EvictionDecision::Archive => archive += 1,
            EvictionDecision::Retain => retain += 1,
        }
    }

    BmrReport {
        total_analyzed: decisions.len(),
        prune_count: prune,
        archive_count: archive,
        retain_count: retain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actr_recent_access_high_activation() {
        // Accessed 0.1 days ago — very recent
        let level = actr_base_level(&[0.1]);
        assert!(level > 0.0, "Recent access should have positive activation");
    }

    #[test]
    fn actr_old_access_low_activation() {
        // Accessed 365 days ago — old
        let level = actr_base_level(&[365.0]);
        assert!(
            level < actr_base_level(&[1.0]),
            "Old access should have lower activation"
        );
    }

    #[test]
    fn actr_many_accesses_higher() {
        let single = actr_base_level(&[1.0]);
        let many = actr_base_level(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(many > single, "More accesses should increase base level");
    }

    #[test]
    fn actr_no_accesses() {
        let level = actr_base_level(&[]);
        assert!(level < 0.0, "No accesses should give very low activation");
    }

    #[test]
    fn keep_score_high_importance_high_score() {
        let signals = NodeSignals {
            structural_importance: 0.9,
            actr_base_level: 2.0,
            accommodation_count: 5,
            contradiction_age_days: 0.0,
            confidence: 0.8,
        };
        let score = keep_score(&signals, &BmrWeights::default());
        assert!(score > 0.5, "High importance should yield high keep score");
    }

    #[test]
    fn keep_score_low_everything_low_score() {
        let signals = NodeSignals {
            structural_importance: 0.0,
            actr_base_level: -5.0,
            accommodation_count: 0,
            contradiction_age_days: 300.0,
            confidence: 0.1,
        };
        let score = keep_score(&signals, &BmrWeights::default());
        assert!(
            score < 0.1,
            "Low signals should yield low keep score: {score}"
        );
    }

    #[test]
    fn eviction_high_importance_never_prunes() {
        let signals = NodeSignals {
            structural_importance: 0.8,
            actr_base_level: -5.0,
            accommodation_count: 0,
            contradiction_age_days: 300.0,
            confidence: 0.1,
        };
        let decision = eviction_decision(&signals, &BmrWeights::default(), 0.1, 0.3);
        assert_ne!(
            decision,
            EvictionDecision::Prune,
            "High structural importance should never prune"
        );
    }

    #[test]
    fn eviction_high_corroboration_never_prunes() {
        let signals = NodeSignals {
            structural_importance: 0.0,
            actr_base_level: -5.0,
            accommodation_count: 5,
            contradiction_age_days: 300.0,
            confidence: 0.1,
        };
        let decision = eviction_decision(&signals, &BmrWeights::default(), 0.1, 0.3);
        assert_ne!(
            decision,
            EvictionDecision::Prune,
            "High corroboration should never prune"
        );
    }

    #[test]
    fn eviction_low_everything_prunes() {
        let signals = NodeSignals {
            structural_importance: 0.0,
            actr_base_level: -5.0,
            accommodation_count: 0,
            contradiction_age_days: 200.0,
            confidence: 0.1,
        };
        let decision = eviction_decision(&signals, &BmrWeights::default(), 0.1, 0.3);
        assert_eq!(decision, EvictionDecision::Prune);
    }

    #[test]
    fn bmr_analysis_mixed() {
        let mut signals = HashMap::new();
        let id_keep = Uuid::new_v4();
        let id_prune = Uuid::new_v4();

        signals.insert(
            id_keep,
            NodeSignals {
                structural_importance: 0.9,
                actr_base_level: 3.0,
                accommodation_count: 5,
                contradiction_age_days: 0.0,
                confidence: 0.9,
            },
        );
        signals.insert(
            id_prune,
            NodeSignals {
                structural_importance: 0.0,
                actr_base_level: -5.0,
                accommodation_count: 0,
                contradiction_age_days: 200.0,
                confidence: 0.1,
            },
        );

        let decisions = bmr_analysis(&signals, &BmrWeights::default(), 0.1, 0.3);
        assert_eq!(decisions[&id_keep], EvictionDecision::Retain);
        assert_eq!(decisions[&id_prune], EvictionDecision::Prune);

        let report = bmr_report(&decisions);
        assert_eq!(report.total_analyzed, 2);
        assert_eq!(report.prune_count, 1);
        assert_eq!(report.retain_count, 1);
    }
}
