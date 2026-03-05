//! DF-QuAD contradiction penalty for confidence propagation (covalence#137).
//!
//! Models how attacking arguments (CONTRADICTS / CONTENDS edges) reduce
//! the confidence accumulated from the DS-fusion step.

/// Apply DF-QuAD attack penalty to a base confidence.
///
/// `attackers` is a slice of `(trust_score, edge_weight)` pairs where each
/// entry represents one attacking node B and its edge weight `w(B→A)`.
///
/// ```text
/// attack_contribution(Bⱼ→A) = trust_score(Bⱼ) × w(Bⱼ→A)   [clamped to [0,1]]
/// total_attack = 1 − ∏(1 − attack_contribution(Bⱼ→A))
/// conf_after_penalty = base_conf × (1 − total_attack)
/// ```
///
/// Returns `base_conf` unchanged when the attacker list is empty.
pub fn dfquad_penalty(base_conf: f64, attackers: &[(f64, f64)]) -> f64 {
    if attackers.is_empty() {
        return base_conf;
    }

    let complement_product = attackers.iter().fold(1.0_f64, |acc, &(trust, weight)| {
        acc * (1.0 - (trust * weight).clamp(0.0, 1.0))
    });

    let total_attack = 1.0 - complement_product;
    base_conf * (1.0 - total_attack)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_attackers_identity() {
        assert!((dfquad_penalty(0.9, &[]) - 0.9).abs() < 1e-12);
    }

    #[test]
    fn single_contradicts_attacker() {
        // base=0.9, trust=0.8, w=1.0 → total_attack=0.8 → conf=0.9×0.2=0.18
        let result = dfquad_penalty(0.9, &[(0.8, 1.0)]);
        assert!((result - 0.18).abs() < 1e-9, "got {result}");
    }

    #[test]
    fn contends_less_damaging_than_contradicts() {
        let base = 0.9;
        let trust = 0.8;
        let contradicts = dfquad_penalty(base, &[(trust, 1.0)]);
        let contends = dfquad_penalty(base, &[(trust, 0.3)]);
        assert!(
            contends > contradicts,
            "contends ({contends}) should be less damaging than contradicts ({contradicts})"
        );
    }

    #[test]
    fn zero_attack_leaves_base_unchanged() {
        // trust=0.0 → contribution=0 → total_attack=0 → unchanged
        let result = dfquad_penalty(0.7, &[(0.0, 1.0)]);
        assert!((result - 0.7).abs() < 1e-12);
    }
}
