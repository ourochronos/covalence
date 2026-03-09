//! Contradiction handling via DF-QuAD framework.
//!
//! Stage 3: CONTRADICTS and CONTENDS edges use gradual degradation.
//! Fixed-point iteration resolves circular attacks.

/// Weight multiplier for CONTENDS edges (partial disagreement).
pub const CONTENDS_WEIGHT_FACTOR: f64 = 0.3;

/// Default convergence epsilon for circular attack resolution.
pub const DEFAULT_ATTACK_EPSILON: f64 = 1e-6;

/// Maximum iterations for circular attack resolution.
pub const MAX_ATTACK_ITERATIONS: usize = 1000;

/// Apply a DF-QuAD attack to a target's confidence.
///
/// Formula: `target_conf * (1 - attacker_conf * edge_weight)`
///
/// This models gradual degradation — a highly confident attacker with a
/// strong edge weight can significantly reduce the target's confidence,
/// but never fully zero it in a single step.
///
/// # Arguments
///
/// * `target_conf` - Current confidence of the attacked claim
/// * `attacker_conf` - Confidence of the attacking claim
/// * `edge_weight` - Attack edge weight. For `CONTRADICTS` use full weight,
///   for `CONTENDS` use `weight * 0.3`
///
/// # Returns
///
/// Updated target confidence, clamped to `[0, 1]`.
pub fn dfquad_attack(target_conf: f64, attacker_conf: f64, edge_weight: f64) -> f64 {
    let result = target_conf * (1.0 - attacker_conf * edge_weight);
    result.clamp(0.0, 1.0)
}

/// Resolve circular attacks via fixed-point iteration.
///
/// When claims mutually attack each other (A contradicts B, B contradicts A),
/// we iterate the attack computations until confidences converge to a stable
/// fixed point.
///
/// # Arguments
///
/// * `claims` - Mutable slice of (id, confidence) pairs. Confidences are updated in place.
/// * `attacks` - Slice of (attacker_index, target_index, effective_weight) triples,
///   where indices refer to positions in `claims`.
/// * `epsilon` - Convergence threshold — stop when no confidence changes by more than epsilon.
///
/// # Returns
///
/// Number of iterations performed. Returns 0 if there are no attacks.
pub fn resolve_circular_attacks(
    claims: &mut [(uuid::Uuid, f64)],
    attacks: &[(usize, usize, f64)],
    epsilon: f64,
) -> usize {
    if attacks.is_empty() || claims.is_empty() {
        return 0;
    }

    let mut iterations = 0;
    let mut base_confidences: Vec<f64> = claims.iter().map(|(_, c)| *c).collect();

    loop {
        iterations += 1;
        let mut max_delta: f64 = 0.0;

        // Start from base confidences each iteration
        let mut new_confidences = base_confidences.clone();

        // Apply all attacks to compute new confidences
        for &(attacker_idx, target_idx, weight) in attacks {
            if attacker_idx >= claims.len() || target_idx >= claims.len() {
                continue;
            }
            let attacker_conf = claims[attacker_idx].1;
            new_confidences[target_idx] =
                dfquad_attack(new_confidences[target_idx], attacker_conf, weight);
        }

        // Check convergence
        for (i, claim) in claims.iter_mut().enumerate() {
            let delta = (claim.1 - new_confidences[i]).abs();
            if delta > max_delta {
                max_delta = delta;
            }
            claim.1 = new_confidences[i];
        }

        // Update base confidences for next iteration
        base_confidences = claims.iter().map(|(_, c)| *c).collect();

        if max_delta < epsilon || iterations >= MAX_ATTACK_ITERATIONS {
            break;
        }
    }

    iterations
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_dfquad_basic_attack() {
        // target=0.8, attacker=0.6, weight=1.0
        // result = 0.8 * (1 - 0.6 * 1.0) = 0.8 * 0.4 = 0.32
        let result = dfquad_attack(0.8, 0.6, 1.0);
        assert!((result - 0.32).abs() < 1e-10);
    }

    #[test]
    fn test_dfquad_contends_weight() {
        // CONTENDS uses 0.3x weight
        let contradicts = dfquad_attack(0.8, 0.6, 1.0);
        let contends = dfquad_attack(0.8, 0.6, CONTENDS_WEIGHT_FACTOR);
        // CONTENDS should be less damaging
        assert!(contends > contradicts);
    }

    #[test]
    fn test_dfquad_zero_attacker_no_effect() {
        let result = dfquad_attack(0.8, 0.0, 1.0);
        assert!((result - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_dfquad_zero_weight_no_effect() {
        let result = dfquad_attack(0.8, 0.9, 0.0);
        assert!((result - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_dfquad_full_attack() {
        // weight=1, attacker=1: target * (1 - 1) = 0
        let result = dfquad_attack(0.8, 1.0, 1.0);
        assert!((result - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_dfquad_clamp_non_negative() {
        // Extreme values should not produce negative results
        let result = dfquad_attack(0.5, 1.5, 1.0);
        assert!(result >= 0.0);
    }

    #[test]
    fn test_resolve_no_attacks() {
        let mut claims = vec![(Uuid::new_v4(), 0.8), (Uuid::new_v4(), 0.6)];
        let iterations = resolve_circular_attacks(&mut claims, &[], DEFAULT_ATTACK_EPSILON);
        assert_eq!(iterations, 0);
    }

    #[test]
    fn test_resolve_empty_claims() {
        let mut claims: Vec<(Uuid, f64)> = vec![];
        let iterations =
            resolve_circular_attacks(&mut claims, &[(0, 1, 0.5)], DEFAULT_ATTACK_EPSILON);
        assert_eq!(iterations, 0);
    }

    #[test]
    fn test_resolve_one_way_attack() {
        let mut claims = vec![(Uuid::new_v4(), 0.8), (Uuid::new_v4(), 0.9)];
        // Claim 0 attacks claim 1 with weight 0.5
        let attacks = vec![(0, 1, 0.5)];
        let iterations = resolve_circular_attacks(&mut claims, &attacks, DEFAULT_ATTACK_EPSILON);
        assert!(iterations > 0);
        // Claim 0 should be unchanged (not attacked)
        assert!((claims[0].1 - 0.8).abs() < 0.01);
        // Claim 1 should be reduced
        assert!(claims[1].1 < 0.9);
    }

    #[test]
    fn test_resolve_mutual_attack_converges() {
        let mut claims = vec![(Uuid::new_v4(), 0.8), (Uuid::new_v4(), 0.7)];
        // Mutual contradiction
        let attacks = vec![(0, 1, 1.0), (1, 0, 1.0)];
        let iterations = resolve_circular_attacks(&mut claims, &attacks, DEFAULT_ATTACK_EPSILON);
        assert!(iterations > 1);
        // Both should be reduced
        assert!(claims[0].1 < 0.8);
        assert!(claims[1].1 < 0.7);
    }

    #[test]
    fn test_resolve_respects_max_iterations() {
        let mut claims = vec![(Uuid::new_v4(), 0.99), (Uuid::new_v4(), 0.99)];
        // Very tight mutual attack — may take many iterations
        let attacks = vec![(0, 1, 0.01), (1, 0, 0.01)];
        let iterations = resolve_circular_attacks(&mut claims, &attacks, 1e-15);
        assert!(iterations <= MAX_ATTACK_ITERATIONS);
    }

    #[test]
    fn test_resolve_out_of_bounds_indices_ignored() {
        let mut claims = vec![(Uuid::new_v4(), 0.8)];
        let attacks = vec![(0, 5, 1.0)]; // target index out of bounds
        let iterations = resolve_circular_attacks(&mut claims, &attacks, DEFAULT_ATTACK_EPSILON);
        assert!(iterations > 0);
        // Claim should be unchanged since the attack target is out of bounds
        assert!((claims[0].1 - 0.8).abs() < 1e-10);
    }
}
