//! Evidence fusion — Dempster-Shafer and Subjective Logic cumulative fusion.
//!
//! Stage 1: Dempster-Shafer for multi-source evidence combination.
//! Stage 2: Subjective Logic cumulative fusion for CONFIRMS edges.

use crate::types::opinion::Opinion;

/// A Dempster-Shafer mass function over a binary frame of discernment.
///
/// The frame has two hypotheses: the claim is true (belief) or false (disbelief),
/// plus the full frame (uncertainty). This is a simplified two-hypothesis DS
/// representation isomorphic to Subjective Logic opinions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MassFunction {
    /// Mass assigned to the claim being true.
    pub belief: f64,
    /// Mass assigned to the claim being false.
    pub disbelief: f64,
    /// Mass assigned to the full frame (ignorance).
    pub uncertainty: f64,
}

impl MassFunction {
    /// Create a new mass function. Returns `None` if masses do not sum to 1.
    pub fn new(belief: f64, disbelief: f64, uncertainty: f64) -> Option<Self> {
        let sum = belief + disbelief + uncertainty;
        if (sum - 1.0).abs() > 1e-6 {
            return None;
        }
        Some(Self {
            belief,
            disbelief,
            uncertainty,
        })
    }

    /// The belief interval: `[belief, belief + uncertainty]`.
    /// The plausibility is the upper bound.
    pub fn plausibility(&self) -> f64 {
        self.belief + self.uncertainty
    }

    /// Convert from an Opinion (dropping base rate).
    pub fn from_opinion(opinion: &Opinion) -> Self {
        Self {
            belief: opinion.belief,
            disbelief: opinion.disbelief,
            uncertainty: opinion.uncertainty,
        }
    }
}

/// Dempster-Shafer combination rule for two mass functions.
///
/// Combines two independent bodies of evidence using the standard DS rule:
///
/// ```text
/// m_combined(A) = (1 / (1 - K)) * Σ m1(B) * m2(C)  for B ∩ C = A
/// ```
///
/// where K is the conflict mass (sum of products where B ∩ C = ∅).
///
/// Returns `None` if K = 1 (total conflict — the sources are completely
/// contradictory and cannot be combined).
pub fn dempster_shafer_combine(m1: &MassFunction, m2: &MassFunction) -> Option<MassFunction> {
    // In a binary frame {T, F, Θ}:
    // T ∩ T = T, T ∩ F = ∅, T ∩ Θ = T
    // F ∩ F = F, F ∩ T = ∅, F ∩ Θ = F
    // Θ ∩ T = T, Θ ∩ F = F, Θ ∩ Θ = Θ

    // Conflict mass K: products where intersection is empty
    let k = m1.belief * m2.disbelief + m1.disbelief * m2.belief;

    if (k - 1.0).abs() < 1e-10 {
        return None; // Total conflict
    }

    let norm = 1.0 / (1.0 - k);

    // Belief mass: T ∩ T, T ∩ Θ, Θ ∩ T
    let belief =
        norm * (m1.belief * m2.belief + m1.belief * m2.uncertainty + m1.uncertainty * m2.belief);

    // Disbelief mass: F ∩ F, F ∩ Θ, Θ ∩ F
    let disbelief = norm
        * (m1.disbelief * m2.disbelief
            + m1.disbelief * m2.uncertainty
            + m1.uncertainty * m2.disbelief);

    // Uncertainty mass: Θ ∩ Θ
    let uncertainty = norm * (m1.uncertainty * m2.uncertainty);

    MassFunction::new(belief, disbelief, uncertainty)
}

/// Subjective Logic cumulative fusion of two opinions.
///
/// Independent confirmations reduce uncertainty while increasing belief.
/// The fusion operator is commutative and associative.
///
/// When both opinions are dogmatic (u=0), we fall back to averaging
/// (degenerate case since no uncertainty remains to redistribute).
///
/// # Arguments
///
/// * `w1` - First opinion
/// * `w2` - Second opinion
///
/// # Returns
///
/// The fused opinion, or `None` if the result would be invalid.
pub fn cumulative_fuse(w1: &Opinion, w2: &Opinion) -> Option<Opinion> {
    let u1 = w1.uncertainty;
    let u2 = w2.uncertainty;

    // Both dogmatic — average the belief masses
    if u1 < 1e-10 && u2 < 1e-10 {
        let belief = (w1.belief + w2.belief) / 2.0;
        let disbelief = (w1.disbelief + w2.disbelief) / 2.0;
        let base_rate = (w1.base_rate + w2.base_rate) / 2.0;
        return Opinion::new(belief, disbelief, 0.0, base_rate);
    }

    // Standard cumulative fusion formula from Jøsang
    let kappa = u1 + u2 - u1 * u2;

    let belief = (w1.belief * u2 + w2.belief * u1) / kappa;
    let disbelief = (w1.disbelief * u2 + w2.disbelief * u1) / kappa;
    let uncertainty = (u1 * u2) / kappa;

    // Base rate: weighted average by uncertainty
    let base_rate = if (u1 - 1.0).abs() < 1e-10 && (u2 - 1.0).abs() < 1e-10 {
        (w1.base_rate + w2.base_rate) / 2.0
    } else {
        // Use the complementary uncertainty as weight
        let weight_total = (1.0 - u1) + (1.0 - u2);
        if weight_total < 1e-10 {
            (w1.base_rate + w2.base_rate) / 2.0
        } else {
            (w1.base_rate * (1.0 - u1) + w2.base_rate * (1.0 - u2)) / weight_total
        }
    };

    Opinion::new(belief, disbelief, uncertainty, base_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- MassFunction tests ---

    #[test]
    fn test_mass_function_valid() {
        let mf = MassFunction::new(0.4, 0.3, 0.3);
        assert!(mf.is_some());
    }

    #[test]
    fn test_mass_function_invalid_sum() {
        let mf = MassFunction::new(0.5, 0.5, 0.5);
        assert!(mf.is_none());
    }

    #[test]
    fn test_plausibility() {
        let mf = MassFunction::new(0.4, 0.3, 0.3).unwrap();
        assert!((mf.plausibility() - 0.7).abs() < 1e-10);
    }

    #[test]
    fn test_from_opinion() {
        let op = Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap();
        let mf = MassFunction::from_opinion(&op);
        assert!((mf.belief - 0.6).abs() < 1e-10);
        assert!((mf.disbelief - 0.2).abs() < 1e-10);
        assert!((mf.uncertainty - 0.2).abs() < 1e-10);
    }

    // --- Dempster-Shafer tests ---

    #[test]
    fn test_ds_combine_agreeing_sources() {
        let m1 = MassFunction::new(0.6, 0.1, 0.3).unwrap();
        let m2 = MassFunction::new(0.5, 0.1, 0.4).unwrap();
        let combined = dempster_shafer_combine(&m1, &m2).unwrap();
        // Belief should increase, uncertainty should decrease
        assert!(combined.belief > m1.belief);
        assert!(combined.uncertainty < m1.uncertainty);
        // Sum should be 1
        let sum = combined.belief + combined.disbelief + combined.uncertainty;
        assert!((sum - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_ds_combine_one_vacuous() {
        // Combining with vacuous evidence should not change the other
        let m1 = MassFunction::new(0.6, 0.2, 0.2).unwrap();
        let vacuous = MassFunction::new(0.0, 0.0, 1.0).unwrap();
        let combined = dempster_shafer_combine(&m1, &vacuous).unwrap();
        assert!((combined.belief - m1.belief).abs() < 1e-10);
        assert!((combined.disbelief - m1.disbelief).abs() < 1e-10);
        assert!((combined.uncertainty - m1.uncertainty).abs() < 1e-10);
    }

    #[test]
    fn test_ds_combine_total_conflict_returns_none() {
        // One source says all true, the other says all false
        let m1 = MassFunction::new(1.0, 0.0, 0.0).unwrap();
        let m2 = MassFunction::new(0.0, 1.0, 0.0).unwrap();
        assert!(dempster_shafer_combine(&m1, &m2).is_none());
    }

    #[test]
    fn test_ds_combine_commutative() {
        let m1 = MassFunction::new(0.5, 0.2, 0.3).unwrap();
        let m2 = MassFunction::new(0.3, 0.3, 0.4).unwrap();
        let r1 = dempster_shafer_combine(&m1, &m2).unwrap();
        let r2 = dempster_shafer_combine(&m2, &m1).unwrap();
        assert!((r1.belief - r2.belief).abs() < 1e-10);
        assert!((r1.disbelief - r2.disbelief).abs() < 1e-10);
        assert!((r1.uncertainty - r2.uncertainty).abs() < 1e-10);
    }

    #[test]
    fn test_ds_combine_reduces_uncertainty() {
        let m1 = MassFunction::new(0.3, 0.1, 0.6).unwrap();
        let m2 = MassFunction::new(0.4, 0.1, 0.5).unwrap();
        let combined = dempster_shafer_combine(&m1, &m2).unwrap();
        assert!(combined.uncertainty < m1.uncertainty);
        assert!(combined.uncertainty < m2.uncertainty);
    }

    // --- Cumulative Fusion tests ---

    #[test]
    fn test_cumulative_fuse_reduces_uncertainty() {
        let w1 = Opinion::new(0.5, 0.1, 0.4, 0.5).unwrap();
        let w2 = Opinion::new(0.6, 0.1, 0.3, 0.5).unwrap();
        let fused = cumulative_fuse(&w1, &w2).unwrap();
        assert!(fused.uncertainty < w1.uncertainty);
        assert!(fused.uncertainty < w2.uncertainty);
    }

    #[test]
    fn test_cumulative_fuse_commutative() {
        let w1 = Opinion::new(0.5, 0.1, 0.4, 0.5).unwrap();
        let w2 = Opinion::new(0.3, 0.2, 0.5, 0.5).unwrap();
        let r1 = cumulative_fuse(&w1, &w2).unwrap();
        let r2 = cumulative_fuse(&w2, &w1).unwrap();
        assert!((r1.belief - r2.belief).abs() < 1e-10);
        assert!((r1.disbelief - r2.disbelief).abs() < 1e-10);
        assert!((r1.uncertainty - r2.uncertainty).abs() < 1e-10);
    }

    #[test]
    fn test_cumulative_fuse_with_vacuous() {
        let w1 = Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap();
        let vacuous = Opinion::vacuous(0.5);
        let fused = cumulative_fuse(&w1, &vacuous).unwrap();
        // Fusing with vacuous opinion should return something close to the original
        assert!((fused.belief - w1.belief).abs() < 1e-10);
        assert!((fused.disbelief - w1.disbelief).abs() < 1e-10);
        assert!((fused.uncertainty - w1.uncertainty).abs() < 1e-10);
    }

    #[test]
    fn test_cumulative_fuse_both_dogmatic() {
        let w1 = Opinion::certain(0.8);
        let w2 = Opinion::certain(0.6);
        let fused = cumulative_fuse(&w1, &w2).unwrap();
        assert!((fused.belief - 0.7).abs() < 1e-10);
        assert!((fused.uncertainty).abs() < 1e-10);
    }

    #[test]
    fn test_cumulative_fuse_preserves_opinion_constraint() {
        let w1 = Opinion::new(0.4, 0.2, 0.4, 0.5).unwrap();
        let w2 = Opinion::new(0.3, 0.3, 0.4, 0.6).unwrap();
        let fused = cumulative_fuse(&w1, &w2).unwrap();
        let sum = fused.belief + fused.disbelief + fused.uncertainty;
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cumulative_fuse_multiple_confirmations_converge() {
        // Multiple independent confirmations should drive belief up and uncertainty down
        let mut opinion = Opinion::new(0.3, 0.1, 0.6, 0.5).unwrap();
        let confirming = Opinion::new(0.5, 0.1, 0.4, 0.5).unwrap();
        for _ in 0..10 {
            opinion = cumulative_fuse(&opinion, &confirming).unwrap();
        }
        assert!(opinion.belief > 0.3);
        assert!(opinion.uncertainty < 0.1);
    }
}
