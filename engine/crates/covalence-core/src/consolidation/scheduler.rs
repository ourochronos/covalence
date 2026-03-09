//! Consolidation scheduler.
//!
//! Manages timing for batch and deep consolidation runs.
//! Batch: triggered by timer or epistemic delta threshold.
//! Deep: scheduled daily with on-demand override.
//!
//! The scheduler accumulates epistemic deltas reported by the
//! ingestion pipeline. When the accumulated delta exceeds the
//! configured threshold, [`should_run_batch`](ConsolidationScheduler::should_run_batch)
//! returns `true` even if the time-based interval has not elapsed.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::epistemic::delta::EpistemicDelta;

/// Scheduler that determines when batch and deep consolidation should run.
pub struct ConsolidationScheduler {
    /// Minimum interval between batch runs.
    pub batch_interval: Duration,
    /// Minimum interval between deep runs.
    pub deep_interval: Duration,
    /// Epistemic delta threshold that triggers an early batch run.
    pub delta_threshold: f64,
    /// Most recently recorded epistemic delta value.
    latest_delta: f64,
    /// Accumulated epistemic delta since last batch run.
    accumulated_delta: f64,
    /// Number of delta reports accumulated since last reset.
    delta_report_count: u64,
}

impl ConsolidationScheduler {
    /// Create a new scheduler with explicit intervals and threshold.
    pub fn new(batch_interval: Duration, deep_interval: Duration, delta_threshold: f64) -> Self {
        Self {
            batch_interval,
            deep_interval,
            delta_threshold,
            latest_delta: 0.0,
            accumulated_delta: 0.0,
            delta_report_count: 0,
        }
    }

    /// Record a raw epistemic delta value for use in scheduling
    /// decisions.
    ///
    /// This stores the latest delta so that external callers can
    /// query whether a batch run should be triggered without passing
    /// the delta explicitly.
    pub fn record_delta(&mut self, delta: f64) {
        self.latest_delta = delta;
    }

    /// Get the most recently recorded delta value.
    pub fn latest_delta(&self) -> f64 {
        self.latest_delta
    }

    /// Report an [`EpistemicDelta`] from the ingestion pipeline.
    ///
    /// The scheduler accumulates the total delta from each report.
    /// When the accumulated delta exceeds `delta_threshold`,
    /// [`should_run_batch`](Self::should_run_batch) will return `true`
    /// regardless of timing.
    ///
    /// Call [`reset_accumulated_delta`](Self::reset_accumulated_delta)
    /// after a batch run completes to clear the accumulator.
    pub fn report_delta(&mut self, delta: &EpistemicDelta) {
        self.accumulated_delta += delta.total_delta;
        self.latest_delta = delta.total_delta;
        self.delta_report_count += 1;
    }

    /// Get the accumulated epistemic delta since last reset.
    pub fn accumulated_delta(&self) -> f64 {
        self.accumulated_delta
    }

    /// Number of delta reports received since last reset.
    pub fn delta_report_count(&self) -> u64 {
        self.delta_report_count
    }

    /// Whether the accumulated delta has crossed the significance
    /// threshold, indicating a batch run should be triggered.
    pub fn accumulated_delta_is_significant(&self) -> bool {
        self.accumulated_delta > self.delta_threshold
    }

    /// Reset the accumulated delta after a batch run completes.
    pub fn reset_accumulated_delta(&mut self) {
        self.accumulated_delta = 0.0;
        self.delta_report_count = 0;
    }

    /// Check whether a batch consolidation should run.
    ///
    /// Returns true if any of the following conditions are met:
    /// - The elapsed time since `last_run` exceeds `batch_interval`
    /// - `current_delta` exceeds the threshold
    /// - The accumulated delta (from [`report_delta`](Self::report_delta)
    ///   calls) exceeds the threshold
    pub fn should_run_batch(
        &self,
        last_run: DateTime<Utc>,
        now: DateTime<Utc>,
        current_delta: f64,
    ) -> bool {
        let elapsed = now.signed_duration_since(last_run);
        let interval =
            chrono::Duration::from_std(self.batch_interval).unwrap_or(chrono::Duration::MAX);
        elapsed >= interval
            || current_delta > self.delta_threshold
            || self.accumulated_delta > self.delta_threshold
    }

    /// Check whether a deep consolidation should run.
    ///
    /// Returns true if the elapsed time since `last_run` exceeds
    /// `deep_interval`.
    pub fn should_run_deep(&self, last_run: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        let elapsed = now.signed_duration_since(last_run);
        let interval =
            chrono::Duration::from_std(self.deep_interval).unwrap_or(chrono::Duration::MAX);
        elapsed >= interval
    }
}

impl Default for ConsolidationScheduler {
    fn default() -> Self {
        Self {
            batch_interval: Duration::from_secs(3600), // 1 hour
            deep_interval: Duration::from_secs(86400), // 24 hours
            delta_threshold: 0.1,
            latest_delta: 0.0,
            accumulated_delta: 0.0,
            delta_report_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_triggers_on_time() {
        let scheduler = ConsolidationScheduler::default();
        let last = Utc::now() - chrono::Duration::hours(2);
        assert!(scheduler.should_run_batch(last, Utc::now(), 0.0));
    }

    #[test]
    fn batch_does_not_trigger_early() {
        let scheduler = ConsolidationScheduler::default();
        let last = Utc::now() - chrono::Duration::minutes(30);
        assert!(!scheduler.should_run_batch(last, Utc::now(), 0.0));
    }

    #[test]
    fn batch_triggers_on_delta() {
        let scheduler = ConsolidationScheduler::default();
        let last = Utc::now() - chrono::Duration::minutes(5);
        assert!(scheduler.should_run_batch(last, Utc::now(), 0.5));
    }

    #[test]
    fn batch_does_not_trigger_below_delta() {
        let scheduler = ConsolidationScheduler::default();
        let last = Utc::now() - chrono::Duration::minutes(5);
        assert!(!scheduler.should_run_batch(last, Utc::now(), 0.05));
    }

    #[test]
    fn deep_triggers_on_time() {
        let scheduler = ConsolidationScheduler::default();
        let last = Utc::now() - chrono::Duration::hours(25);
        assert!(scheduler.should_run_deep(last, Utc::now()));
    }

    #[test]
    fn deep_does_not_trigger_early() {
        let scheduler = ConsolidationScheduler::default();
        let last = Utc::now() - chrono::Duration::hours(12);
        assert!(!scheduler.should_run_deep(last, Utc::now()));
    }

    #[test]
    fn default_values() {
        let scheduler = ConsolidationScheduler::default();
        assert_eq!(scheduler.batch_interval, Duration::from_secs(3600));
        assert_eq!(scheduler.deep_interval, Duration::from_secs(86400));
        assert!((scheduler.delta_threshold - 0.1).abs() < f64::EPSILON);
        assert!((scheduler.latest_delta - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn record_delta_stores_value() {
        let mut scheduler = ConsolidationScheduler::default();
        assert!((scheduler.latest_delta() - 0.0).abs() < f64::EPSILON);

        scheduler.record_delta(0.25);
        assert!((scheduler.latest_delta() - 0.25).abs() < f64::EPSILON);

        scheduler.record_delta(0.05);
        assert!((scheduler.latest_delta() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn report_delta_accumulates() {
        use crate::epistemic::delta::EpistemicDelta;

        let mut scheduler = ConsolidationScheduler::default();
        assert!((scheduler.accumulated_delta() - 0.0).abs() < f64::EPSILON);
        assert_eq!(scheduler.delta_report_count(), 0);

        // First report: 0.03 total delta (below threshold)
        let mut d1 = EpistemicDelta::new(0.1);
        d1.total_delta = 0.03;
        scheduler.report_delta(&d1);

        assert!((scheduler.accumulated_delta() - 0.03).abs() < 1e-10);
        assert!((scheduler.latest_delta() - 0.03).abs() < 1e-10);
        assert_eq!(scheduler.delta_report_count(), 1);
        assert!(!scheduler.accumulated_delta_is_significant());

        // Second report: 0.04 (accumulated = 0.07, still below)
        let mut d2 = EpistemicDelta::new(0.1);
        d2.total_delta = 0.04;
        scheduler.report_delta(&d2);

        assert!((scheduler.accumulated_delta() - 0.07).abs() < 1e-10);
        assert_eq!(scheduler.delta_report_count(), 2);
        assert!(!scheduler.accumulated_delta_is_significant());

        // Third report: 0.05 (accumulated = 0.12, above 0.1 threshold)
        let mut d3 = EpistemicDelta::new(0.1);
        d3.total_delta = 0.05;
        scheduler.report_delta(&d3);

        assert!((scheduler.accumulated_delta() - 0.12).abs() < 1e-10);
        assert_eq!(scheduler.delta_report_count(), 3);
        assert!(scheduler.accumulated_delta_is_significant());
    }

    #[test]
    fn report_delta_triggers_batch() {
        use crate::epistemic::delta::EpistemicDelta;

        let mut scheduler = ConsolidationScheduler::default();
        let recent = Utc::now() - chrono::Duration::minutes(5);

        // No accumulated delta: should not trigger
        assert!(!scheduler.should_run_batch(recent, Utc::now(), 0.0));

        // Accumulate enough delta to cross threshold
        let mut delta = EpistemicDelta::new(0.1);
        delta.total_delta = 0.15;
        scheduler.report_delta(&delta);

        // Now should trigger even though time hasn't elapsed
        assert!(scheduler.should_run_batch(recent, Utc::now(), 0.0));
    }

    #[test]
    fn reset_accumulated_delta_clears_state() {
        use crate::epistemic::delta::EpistemicDelta;

        let mut scheduler = ConsolidationScheduler::default();

        let mut delta = EpistemicDelta::new(0.1);
        delta.total_delta = 0.15;
        scheduler.report_delta(&delta);

        assert!(scheduler.accumulated_delta_is_significant());
        assert_eq!(scheduler.delta_report_count(), 1);

        scheduler.reset_accumulated_delta();

        assert!((scheduler.accumulated_delta() - 0.0).abs() < f64::EPSILON);
        assert_eq!(scheduler.delta_report_count(), 0);
        assert!(!scheduler.accumulated_delta_is_significant());
    }

    #[test]
    fn default_accumulated_values() {
        let scheduler = ConsolidationScheduler::default();
        assert!((scheduler.accumulated_delta() - 0.0).abs() < f64::EPSILON);
        assert_eq!(scheduler.delta_report_count(), 0);
        assert!(!scheduler.accumulated_delta_is_significant());
    }
}
