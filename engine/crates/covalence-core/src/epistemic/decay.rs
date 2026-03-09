//! Temporal decay — SUPERSEDES and CORRECTS edge handling.
//!
//! Stage 4: SUPERSEDES applies proportional confidence reduction.
//! CORRECTS immediately zeros confidence.

/// Apply SUPERSEDES decay to the old claim's confidence.
///
/// Formula: `old_conf * (1 - new_conf * weight)`
///
/// Full supersession (weight=1, new_conf=1) drives the old confidence
/// toward zero. Partial supersession with lower weight or lower confidence
/// in the new claim preserves some of the old confidence.
///
/// # Arguments
///
/// * `old_conf` - Confidence of the superseded claim
/// * `new_conf` - Confidence of the superseding claim
/// * `weight` - Edge weight controlling supersession strength, in `[0, 1]`
///
/// # Returns
///
/// Updated confidence for the old claim, clamped to `[0, 1]`.
pub fn apply_supersedes(old_conf: f64, new_conf: f64, weight: f64) -> f64 {
    let result = old_conf * (1.0 - new_conf * weight);
    result.clamp(0.0, 1.0)
}

/// Apply CORRECTS — explicit retraction.
///
/// Always returns 0.0, regardless of input. An explicit correction is the
/// strongest epistemic signal and immediately zeros the corrected claim.
///
/// # Returns
///
/// Always 0.0.
pub fn apply_corrects() -> f64 {
    0.0
}

/// Apply APPENDED_AFTER — additive temporal sequencing.
///
/// Returns the input confidence unchanged. Append-only edges do not modify
/// existing claims; they merely establish temporal ordering.
///
/// # Arguments
///
/// * `conf` - Current confidence of the existing claim
///
/// # Returns
///
/// The same confidence value, unmodified.
pub fn apply_append(conf: f64) -> f64 {
    conf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supersedes_full_weight_full_confidence() {
        // Full supersession: old should approach zero
        let result = apply_supersedes(0.8, 1.0, 1.0);
        assert!((result - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_supersedes_partial_weight() {
        // weight=0.5, new_conf=1.0: old * (1 - 1.0 * 0.5) = old * 0.5
        let result = apply_supersedes(0.8, 1.0, 0.5);
        assert!((result - 0.4).abs() < 1e-10);
    }

    #[test]
    fn test_supersedes_low_new_confidence() {
        // new_conf=0.3, weight=1.0: old * (1 - 0.3) = old * 0.7
        let result = apply_supersedes(0.8, 0.3, 1.0);
        assert!((result - 0.56).abs() < 1e-10);
    }

    #[test]
    fn test_supersedes_zero_weight_no_change() {
        let result = apply_supersedes(0.8, 1.0, 0.0);
        assert!((result - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_supersedes_zero_new_confidence() {
        let result = apply_supersedes(0.8, 0.0, 1.0);
        assert!((result - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_supersedes_clamped_non_negative() {
        // Extreme values should never produce negative results
        let result = apply_supersedes(0.5, 1.5, 1.0);
        assert!(result >= 0.0);
    }

    #[test]
    fn test_corrects_always_zero() {
        assert!((apply_corrects() - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_append_no_change() {
        assert!((apply_append(0.75) - 0.75).abs() < 1e-10);
        assert!((apply_append(0.0) - 0.0).abs() < 1e-10);
        assert!((apply_append(1.0) - 1.0).abs() < 1e-10);
    }
}
