//! Regression gating for evaluation metrics.
//!
//! Compares current metric values against a saved baseline and
//! returns pass/fail for each metric.

use serde::{Deserialize, Serialize};

/// A named metric with its current and baseline values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricComparison {
    /// Name of the metric.
    pub name: String,
    /// Current measured value.
    pub current: f64,
    /// Baseline value to compare against.
    pub baseline: f64,
    /// Whether this metric passed the regression gate.
    pub passed: bool,
}

/// Result of a regression gate check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionResult {
    /// Per-metric comparisons.
    pub comparisons: Vec<MetricComparison>,
    /// True if all metrics passed.
    pub all_passed: bool,
}

/// Regression gate that compares current metrics to a baseline.
///
/// A metric passes if `current >= baseline * (1.0 - tolerance)`.
/// The tolerance allows small regressions without failing the gate.
pub struct RegressionGate {
    /// Allowed fractional regression (e.g. 0.05 = 5% regression
    /// tolerance).
    tolerance: f64,
    /// Baseline metric values keyed by name.
    baselines: Vec<(String, f64)>,
}

impl RegressionGate {
    /// Create a new regression gate with the given tolerance.
    pub fn new(tolerance: f64) -> Self {
        Self {
            tolerance,
            baselines: Vec::new(),
        }
    }

    /// Add a baseline metric value.
    pub fn add_baseline(&mut self, name: impl Into<String>, value: f64) -> &mut Self {
        self.baselines.push((name.into(), value));
        self
    }

    /// Check current metrics against the baselines.
    ///
    /// Returns a `RegressionResult` with per-metric pass/fail and
    /// an overall gate result.
    pub fn check(&self, current_metrics: &[(String, f64)]) -> RegressionResult {
        let mut comparisons = Vec::new();

        for (name, baseline) in &self.baselines {
            let current = current_metrics
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| *v)
                .unwrap_or(0.0);

            let threshold = baseline * (1.0 - self.tolerance);
            let passed = current >= threshold;

            comparisons.push(MetricComparison {
                name: name.clone(),
                current,
                baseline: *baseline,
                passed,
            });
        }

        let all_passed = comparisons.iter().all(|c| c.passed);

        RegressionResult {
            comparisons,
            all_passed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pass_when_above_baseline() {
        let mut gate = RegressionGate::new(0.05);
        gate.add_baseline("precision", 0.8);
        gate.add_baseline("recall", 0.7);

        let current = vec![
            ("precision".to_string(), 0.85),
            ("recall".to_string(), 0.75),
        ];

        let result = gate.check(&current);
        assert!(result.all_passed);
        assert_eq!(result.comparisons.len(), 2);
        assert!(result.comparisons.iter().all(|c| c.passed));
    }

    #[test]
    fn fail_when_below_threshold() {
        let mut gate = RegressionGate::new(0.05);
        gate.add_baseline("precision", 0.8);

        // 0.7 < 0.8 * 0.95 = 0.76
        let current = vec![("precision".to_string(), 0.7)];

        let result = gate.check(&current);
        assert!(!result.all_passed);
        assert!(!result.comparisons[0].passed);
    }

    #[test]
    fn pass_within_tolerance() {
        let mut gate = RegressionGate::new(0.10);
        gate.add_baseline("f1", 0.8);

        // 0.75 >= 0.8 * 0.90 = 0.72
        let current = vec![("f1".to_string(), 0.75)];

        let result = gate.check(&current);
        assert!(result.all_passed);
    }

    #[test]
    fn missing_metric_uses_zero() {
        let mut gate = RegressionGate::new(0.0);
        gate.add_baseline("missing_metric", 0.5);

        let current: Vec<(String, f64)> = vec![];

        let result = gate.check(&current);
        assert!(!result.all_passed);
        assert!((result.comparisons[0].current - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_tolerance_exact_match_required() {
        let mut gate = RegressionGate::new(0.0);
        gate.add_baseline("exact", 0.5);

        let pass = vec![("exact".to_string(), 0.5)];
        assert!(gate.check(&pass).all_passed);

        let fail = vec![("exact".to_string(), 0.499)];
        assert!(!gate.check(&fail).all_passed);
    }

    #[test]
    fn empty_baselines_always_pass() {
        let gate = RegressionGate::new(0.05);
        let current = vec![("anything".to_string(), 0.0)];
        let result = gate.check(&current);
        assert!(result.all_passed);
        assert!(result.comparisons.is_empty());
    }
}
