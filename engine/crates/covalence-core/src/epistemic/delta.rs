//! Epistemic delta tracking.
//!
//! Measures how much a knowledge cluster has shifted due to new information.
//! Used to trigger batch re-compilation when delta exceeds threshold.

use std::collections::HashMap;
use uuid::Uuid;

/// Default threshold for epistemic delta significance.
pub const DEFAULT_DELTA_THRESHOLD: f64 = 0.10;

/// Tracks per-cluster epistemic changes.
///
/// The epistemic delta quantifies how much a knowledge cluster's confidence
/// has shifted, useful for alerting, prioritizing review, and triggering
/// consolidation.
#[derive(Debug, Clone)]
pub struct EpistemicDelta {
    /// Per-claim absolute confidence changes.
    pub changes: HashMap<Uuid, f64>,
    /// Total delta (sum of absolute changes).
    pub total_delta: f64,
    /// Threshold for considering the delta significant.
    pub threshold: f64,
}

impl EpistemicDelta {
    /// Create an empty delta tracker with the given threshold.
    pub fn new(threshold: f64) -> Self {
        Self {
            changes: HashMap::new(),
            total_delta: 0.0,
            threshold,
        }
    }

    /// Whether the total delta exceeds the significance threshold.
    pub fn is_significant(&self) -> bool {
        self.total_delta > self.threshold
    }

    /// Number of claims that changed.
    pub fn num_changed(&self) -> usize {
        self.changes.len()
    }
}

impl Default for EpistemicDelta {
    fn default() -> Self {
        Self::new(DEFAULT_DELTA_THRESHOLD)
    }
}

/// Compute the epistemic delta between two confidence snapshots.
///
/// Formula: `Σ |confidence_change(claim)| for all affected claims`
///
/// Claims present in only one snapshot are treated as having changed
/// from/to 0.0.
///
/// # Arguments
///
/// * `before` - Confidence snapshot before the update
/// * `after` - Confidence snapshot after the update
///
/// # Returns
///
/// The total delta as a non-negative float.
pub fn compute_delta(before: &HashMap<Uuid, f64>, after: &HashMap<Uuid, f64>) -> f64 {
    let mut total: f64 = 0.0;

    // Claims in `before` that changed or were removed
    for (id, &old_conf) in before {
        let new_conf = after.get(id).copied().unwrap_or(0.0);
        total += (old_conf - new_conf).abs();
    }

    // Claims only in `after` (new claims)
    for (id, &new_conf) in after {
        if !before.contains_key(id) {
            total += new_conf.abs();
        }
    }

    total
}

/// Compute a full [`EpistemicDelta`] with per-claim breakdown.
///
/// # Arguments
///
/// * `before` - Confidence snapshot before the update
/// * `after` - Confidence snapshot after the update
/// * `threshold` - Delta significance threshold
///
/// # Returns
///
/// An `EpistemicDelta` with per-claim changes and total delta.
pub fn compute_epistemic_delta(
    before: &HashMap<Uuid, f64>,
    after: &HashMap<Uuid, f64>,
    threshold: f64,
) -> EpistemicDelta {
    let mut delta = EpistemicDelta::new(threshold);

    for (id, &old_conf) in before {
        let new_conf = after.get(id).copied().unwrap_or(0.0);
        let change = (old_conf - new_conf).abs();
        if change > 1e-10 {
            delta.changes.insert(*id, change);
        }
    }

    for (id, &new_conf) in after {
        if !before.contains_key(id) && new_conf.abs() > 1e-10 {
            delta.changes.insert(*id, new_conf.abs());
        }
    }

    delta.total_delta = delta.changes.values().sum();
    delta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_id() -> Uuid {
        Uuid::new_v4()
    }

    #[test]
    fn test_delta_no_change() {
        let id1 = make_id();
        let id2 = make_id();
        let before: HashMap<Uuid, f64> = [(id1, 0.8), (id2, 0.6)].into();
        let after: HashMap<Uuid, f64> = [(id1, 0.8), (id2, 0.6)].into();
        let delta = compute_delta(&before, &after);
        assert!((delta - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_delta_single_change() {
        let id1 = make_id();
        let before: HashMap<Uuid, f64> = [(id1, 0.8)].into();
        let after: HashMap<Uuid, f64> = [(id1, 0.5)].into();
        let delta = compute_delta(&before, &after);
        assert!((delta - 0.3).abs() < 1e-10);
    }

    #[test]
    fn test_delta_new_claim() {
        let id1 = make_id();
        let id2 = make_id();
        let before: HashMap<Uuid, f64> = [(id1, 0.8)].into();
        let after: HashMap<Uuid, f64> = [(id1, 0.8), (id2, 0.5)].into();
        let delta = compute_delta(&before, &after);
        assert!((delta - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_delta_removed_claim() {
        let id1 = make_id();
        let id2 = make_id();
        let before: HashMap<Uuid, f64> = [(id1, 0.8), (id2, 0.6)].into();
        let after: HashMap<Uuid, f64> = [(id1, 0.8)].into();
        let delta = compute_delta(&before, &after);
        assert!((delta - 0.6).abs() < 1e-10);
    }

    #[test]
    fn test_delta_empty_snapshots() {
        let before: HashMap<Uuid, f64> = HashMap::new();
        let after: HashMap<Uuid, f64> = HashMap::new();
        let delta = compute_delta(&before, &after);
        assert!((delta - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_delta_before_empty() {
        let id1 = make_id();
        let before: HashMap<Uuid, f64> = HashMap::new();
        let after: HashMap<Uuid, f64> = [(id1, 0.7)].into();
        let delta = compute_delta(&before, &after);
        assert!((delta - 0.7).abs() < 1e-10);
    }

    #[test]
    fn test_epistemic_delta_struct() {
        let id1 = make_id();
        let id2 = make_id();
        let before: HashMap<Uuid, f64> = [(id1, 0.8), (id2, 0.6)].into();
        let after: HashMap<Uuid, f64> = [(id1, 0.5), (id2, 0.6)].into();
        let delta = compute_epistemic_delta(&before, &after, DEFAULT_DELTA_THRESHOLD);
        assert_eq!(delta.num_changed(), 1);
        assert!((delta.total_delta - 0.3).abs() < 1e-10);
        assert!(delta.is_significant()); // 0.3 > 0.10
    }

    #[test]
    fn test_epistemic_delta_not_significant() {
        let id1 = make_id();
        let before: HashMap<Uuid, f64> = [(id1, 0.80)].into();
        let after: HashMap<Uuid, f64> = [(id1, 0.81)].into();
        let delta = compute_epistemic_delta(&before, &after, DEFAULT_DELTA_THRESHOLD);
        assert!(!delta.is_significant()); // 0.01 < 0.10
    }

    #[test]
    fn test_epistemic_delta_default_threshold() {
        let delta = EpistemicDelta::default();
        assert!((delta.threshold - DEFAULT_DELTA_THRESHOLD).abs() < 1e-10);
        assert!(!delta.is_significant());
    }
}
