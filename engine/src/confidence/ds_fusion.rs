//! Dempster-Shafer multi-source fusion for confidence propagation (covalence#137).
//!
//! Implements the complement-product formula for combining independent
//! evidence sources into a single belief score.

/// Fuse a slice of effective per-source reliabilities into a single DS score.
///
/// Each element of `source_reliabilities` is an **already-computed**
/// effective reliability `r_i ∈ [0, 1]` for source *i*:
///
/// ```text
/// conf_ds = 1 − ∏(1 − rᵢ)
/// ```
///
/// Special cases:
/// * Empty slice → `0.0` (no evidence → no belief).
/// * Single element → that element's value.
/// * All elements are clamped to `[0.0, 1.0]` before use.
pub fn ds_fusion(source_reliabilities: &[f64]) -> f64 {
    if source_reliabilities.is_empty() {
        return 0.0;
    }

    let complement_product = source_reliabilities
        .iter()
        .fold(1.0_f64, |acc, &r| acc * (1.0 - r.clamp(0.0, 1.0)));

    1.0 - complement_product
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_zero() {
        assert_eq!(ds_fusion(&[]), 0.0);
    }

    #[test]
    fn single_source_identity() {
        assert!((ds_fusion(&[0.75]) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn three_sources_complement_product() {
        // 1 − (0.2 × 0.3 × 0.4) = 1 − 0.024 = 0.976
        let result = ds_fusion(&[0.8, 0.7, 0.6]);
        assert!((result - 0.976).abs() < 1e-9, "got {result}");
    }

    #[test]
    fn clamps_values_above_one() {
        // Should treat 1.5 as 1.0 → complement = 0 → result = 1.0
        assert_eq!(ds_fusion(&[1.5]), 1.0);
    }

    #[test]
    fn clamps_values_below_zero() {
        // Should treat -0.5 as 0.0 → complement = 1 → result = 0.0
        assert_eq!(ds_fusion(&[-0.5]), 0.0);
    }
}
