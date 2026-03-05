//! SUPERSEDES temporal decay for confidence propagation (covalence#137).
//!
//! When a source explicitly supersedes an article it reduces the article's
//! confidence using the same complement-product formula as DF-QuAD.

/// Apply SUPERSEDES decay to a base confidence.
///
/// `supersedes` is a slice of `(trust_score, causal_weight)` pairs where each
/// entry represents one superseding node B and its edge causal weight.
///
/// ```text
/// supersede_factor(Bₖ→A) = trust_score(Bₖ) × causal_weight(edge)  [clamped to [0,1]]
/// total_supersede = 1 − ∏(1 − supersede_factor(Bₖ→A))
/// conf_after_decay = base_conf × (1 − total_supersede)
/// ```
///
/// Returns `base_conf` unchanged when the superseder list is empty.
pub fn supersedes_decay(base_conf: f64, supersedes: &[(f64, f64)]) -> f64 {
    if supersedes.is_empty() {
        return base_conf;
    }

    let complement_product = supersedes.iter().fold(1.0_f64, |acc, &(trust, weight)| {
        acc * (1.0 - (trust * weight).clamp(0.0, 1.0))
    });

    let total_supersede = 1.0 - complement_product;
    base_conf * (1.0 - total_supersede)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_supersedes_identity() {
        assert!((supersedes_decay(0.8, &[]) - 0.8).abs() < 1e-12);
    }

    #[test]
    fn full_supersession_drives_to_zero() {
        // trust=1.0, weight=1.0 → total=1.0 → result=0.0
        let result = supersedes_decay(0.8, &[(1.0, 1.0)]);
        assert!((result - 0.0).abs() < 1e-12, "got {result}");
    }

    #[test]
    fn partial_supersession() {
        // trust=0.6, weight=0.8 → factor=0.48 → total=0.48 → decay=0.485×0.52=0.2522
        let base = 0.485;
        let result = supersedes_decay(base, &[(0.6, 0.8)]);
        let expected = base * (1.0 - 0.48);
        assert!(
            (result - expected).abs() < 1e-9,
            "got {result}, expected {expected}"
        );
    }
}
