//! Convergence guard for epistemic propagation.
//!
//! Prevents oscillation between frameworks by using two-phase propagation
//! with damping and fixed-point iteration.

use std::collections::HashMap;
use uuid::Uuid;

use crate::types::opinion::Opinion;

use super::contradiction::dfquad_attack;
use super::decay;

/// Default convergence epsilon.
pub const DEFAULT_EPSILON: f64 = 1e-6;

/// Default damping factor to prevent oscillation.
pub const DEFAULT_DAMPING: f64 = 0.8;

/// Maximum iterations for epistemic closure.
pub const MAX_CLOSURE_ITERATIONS: usize = 500;

/// An attack relationship between two claims.
#[derive(Debug, Clone)]
pub struct Attack {
    /// ID of the attacking claim.
    pub attacker_id: Uuid,
    /// ID of the target claim.
    pub target_id: Uuid,
    /// Effective edge weight (1.0 for CONTRADICTS, 0.3 for CONTENDS).
    pub weight: f64,
    /// Attack kind for applying the correct decay/attack formula.
    pub kind: AttackKind,
}

/// The kind of epistemic challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackKind {
    /// Full contradiction — DF-QuAD at full weight.
    Contradicts,
    /// Partial disagreement — DF-QuAD at 0.3x weight.
    Contends,
    /// Temporal supersession — proportional decay.
    Supersedes {
        /// Edge weight controlling supersession strength.
        edge_weight_raw: u64,
    },
    /// Explicit retraction — immediate zeroing.
    Corrects,
}

impl AttackKind {
    /// Helper to create Supersedes from an f64, storing as fixed-point.
    pub fn supersedes(weight: f64) -> Self {
        // Store as millionths for lossless round-trip
        Self::Supersedes {
            edge_weight_raw: (weight * 1_000_000.0) as u64,
        }
    }

    /// Recover the f64 weight for Supersedes.
    fn supersedes_weight(&self) -> f64 {
        match self {
            Self::Supersedes { edge_weight_raw } => *edge_weight_raw as f64 / 1_000_000.0,
            _ => 0.0,
        }
    }
}

/// Compute epistemic closure over a set of opinions and attacks.
///
/// This implements the two-phase convergence guard from the spec:
/// 1. Local aggregation (stages 1-2) is assumed already complete.
/// 2. Structural revision (stages 3-5) is iterated here with damping.
///
/// The function modifies opinions in-place by applying contradiction attacks
/// (stage 3) and temporal decay (stage 4) repeatedly until the maximum
/// per-claim projected probability change is below epsilon.
///
/// # Arguments
///
/// * `opinions` - Mutable map of claim ID to opinion. Modified in place.
/// * `attacks` - Slice of attack relationships to apply.
/// * `epsilon` - Convergence threshold.
///
/// # Returns
///
/// Number of iterations performed.
pub fn compute_epistemic_closure(
    opinions: &mut HashMap<Uuid, Opinion>,
    attacks: &[Attack],
    epsilon: f64,
) -> usize {
    if attacks.is_empty() || opinions.is_empty() {
        return 0;
    }

    let mut iterations = 0;

    loop {
        iterations += 1;
        let mut max_delta: f64 = 0.0;

        // Snapshot projected probabilities before this round
        let old_projections: HashMap<Uuid, f64> = opinions
            .iter()
            .map(|(id, op)| (*id, op.projected_probability()))
            .collect();

        // Apply all attacks to compute new projected probabilities
        let mut new_projections: HashMap<Uuid, f64> = old_projections.clone();

        for attack in attacks {
            let attacker_proj = match old_projections.get(&attack.attacker_id) {
                Some(&p) => p,
                None => continue,
            };

            let target_proj = match new_projections.get(&attack.target_id) {
                Some(&p) => p,
                None => continue,
            };

            let updated = match attack.kind {
                AttackKind::Contradicts => dfquad_attack(target_proj, attacker_proj, attack.weight),
                AttackKind::Contends => dfquad_attack(target_proj, attacker_proj, attack.weight),
                AttackKind::Supersedes { .. } => decay::apply_supersedes(
                    target_proj,
                    attacker_proj,
                    attack.kind.supersedes_weight(),
                ),
                AttackKind::Corrects => decay::apply_corrects(),
            };

            new_projections.insert(attack.target_id, updated);
        }

        // Apply damped updates to opinions
        for (id, opinion) in opinions.iter_mut() {
            let old_proj = old_projections.get(id).copied().unwrap_or(0.0);
            let new_proj = new_projections.get(id).copied().unwrap_or(old_proj);

            // Damped update: blend old and new
            let damped_proj = DEFAULT_DAMPING * new_proj + (1.0 - DEFAULT_DAMPING) * old_proj;

            let delta = (old_proj - damped_proj).abs();
            if delta > max_delta {
                max_delta = delta;
            }

            // Update the opinion's belief to reflect the new projected probability
            // while preserving base_rate and adjusting uncertainty proportionally
            update_opinion_from_projection(opinion, damped_proj);
        }

        if max_delta < epsilon || iterations >= MAX_CLOSURE_ITERATIONS {
            break;
        }
    }

    iterations
}

/// Update an opinion's belief/disbelief/uncertainty to match a target
/// projected probability while preserving internal consistency.
///
/// We solve: `belief + base_rate * uncertainty = target_proj` and
/// `belief + disbelief + uncertainty = 1`. Uncertainty is preserved
/// when possible; when the target is unreachable at current uncertainty,
/// uncertainty is reduced to the minimum needed.
fn update_opinion_from_projection(opinion: &mut Opinion, target_proj: f64) {
    let target_proj = target_proj.clamp(0.0, 1.0);
    let a = opinion.base_rate;
    let u = opinion.uncertainty;

    // Try keeping uncertainty the same: b = target - a*u
    let b = target_proj - a * u;
    if b >= 0.0 && b <= 1.0 - u {
        opinion.belief = b;
        opinion.disbelief = 1.0 - b - u;
        return;
    }

    // Target too low for current uncertainty (b would be negative).
    // Reduce uncertainty: with b=0, u = target/a (or 0 if a≈0).
    if b < 0.0 {
        let new_u = if a > 1e-10 {
            (target_proj / a).min(1.0)
        } else {
            0.0
        };
        opinion.belief = 0.0;
        opinion.uncertainty = new_u;
        opinion.disbelief = (1.0 - new_u).max(0.0);
        return;
    }

    // Target too high for current uncertainty (b > 1-u).
    // Reduce uncertainty: b = 1-u, so target = 1-u+a*u = 1-u(1-a),
    // u = (1-target)/(1-a).
    let new_u = if (1.0 - a) > 1e-10 {
        ((1.0 - target_proj) / (1.0 - a)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    opinion.uncertainty = new_u;
    opinion.belief = (target_proj - a * new_u).clamp(0.0, 1.0);
    opinion.disbelief = (1.0 - opinion.belief - new_u).max(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closure_empty_attacks() {
        let mut opinions = HashMap::new();
        opinions.insert(Uuid::new_v4(), Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());
        let iterations = compute_epistemic_closure(&mut opinions, &[], DEFAULT_EPSILON);
        assert_eq!(iterations, 0);
    }

    #[test]
    fn test_closure_empty_opinions() {
        let mut opinions = HashMap::new();
        let attacks = vec![Attack {
            attacker_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            weight: 1.0,
            kind: AttackKind::Contradicts,
        }];
        let iterations = compute_epistemic_closure(&mut opinions, &attacks, DEFAULT_EPSILON);
        assert_eq!(iterations, 0);
    }

    #[test]
    fn test_closure_single_contradiction() {
        let attacker_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let mut opinions = HashMap::new();
        opinions.insert(attacker_id, Opinion::new(0.8, 0.1, 0.1, 0.5).unwrap());
        opinions.insert(target_id, Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());

        let attacks = vec![Attack {
            attacker_id,
            target_id,
            weight: 1.0,
            kind: AttackKind::Contradicts,
        }];

        let original_target_proj = opinions[&target_id].projected_probability();
        let iterations = compute_epistemic_closure(&mut opinions, &attacks, DEFAULT_EPSILON);

        assert!(iterations > 0);
        // Target's projected probability should decrease
        assert!(opinions[&target_id].projected_probability() < original_target_proj);
        // Attacker should be unchanged (no attacks targeting it)
    }

    #[test]
    fn test_closure_mutual_contradiction_converges() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let mut opinions = HashMap::new();
        opinions.insert(id_a, Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());
        opinions.insert(id_b, Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap());

        let attacks = vec![
            Attack {
                attacker_id: id_a,
                target_id: id_b,
                weight: 1.0,
                kind: AttackKind::Contradicts,
            },
            Attack {
                attacker_id: id_b,
                target_id: id_a,
                weight: 1.0,
                kind: AttackKind::Contradicts,
            },
        ];

        let iterations = compute_epistemic_closure(&mut opinions, &attacks, DEFAULT_EPSILON);

        assert!(iterations > 1);
        assert!(iterations < MAX_CLOSURE_ITERATIONS);
        // Both should be reduced
        assert!(opinions[&id_a].projected_probability() < 0.8);
        assert!(opinions[&id_b].projected_probability() < 0.7);
    }

    #[test]
    fn test_closure_contends_less_impact() {
        let attacker_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        // Run with CONTRADICTS
        let mut opinions_contradicts = HashMap::new();
        opinions_contradicts.insert(attacker_id, Opinion::new(0.8, 0.1, 0.1, 0.5).unwrap());
        opinions_contradicts.insert(target_id, Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());
        let attacks_contradicts = vec![Attack {
            attacker_id,
            target_id,
            weight: 1.0,
            kind: AttackKind::Contradicts,
        }];
        compute_epistemic_closure(
            &mut opinions_contradicts,
            &attacks_contradicts,
            DEFAULT_EPSILON,
        );

        // Run with CONTENDS
        let mut opinions_contends = HashMap::new();
        opinions_contends.insert(attacker_id, Opinion::new(0.8, 0.1, 0.1, 0.5).unwrap());
        opinions_contends.insert(target_id, Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());
        let attacks_contends = vec![Attack {
            attacker_id,
            target_id,
            weight: 0.3,
            kind: AttackKind::Contends,
        }];
        compute_epistemic_closure(&mut opinions_contends, &attacks_contends, DEFAULT_EPSILON);

        // CONTENDS should have less impact
        assert!(
            opinions_contends[&target_id].projected_probability()
                > opinions_contradicts[&target_id].projected_probability()
        );
    }

    #[test]
    fn test_closure_corrects_zeros() {
        let attacker_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let mut opinions = HashMap::new();
        opinions.insert(attacker_id, Opinion::new(0.9, 0.05, 0.05, 0.5).unwrap());
        opinions.insert(target_id, Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());

        let attacks = vec![Attack {
            attacker_id,
            target_id,
            weight: 1.0,
            kind: AttackKind::Corrects,
        }];

        compute_epistemic_closure(&mut opinions, &attacks, DEFAULT_EPSILON);

        // Target should be at or very near zero
        assert!(opinions[&target_id].projected_probability() < 0.01);
    }

    #[test]
    fn test_closure_supersedes() {
        let new_id = Uuid::new_v4();
        let old_id = Uuid::new_v4();

        let mut opinions = HashMap::new();
        opinions.insert(new_id, Opinion::new(0.9, 0.05, 0.05, 0.5).unwrap());
        opinions.insert(old_id, Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap());

        let attacks = vec![Attack {
            attacker_id: new_id,
            target_id: old_id,
            weight: 1.0,
            kind: AttackKind::supersedes(0.8),
        }];

        let original_old_proj = opinions[&old_id].projected_probability();
        compute_epistemic_closure(&mut opinions, &attacks, DEFAULT_EPSILON);

        // Old claim's confidence should decrease
        assert!(opinions[&old_id].projected_probability() < original_old_proj);
    }

    #[test]
    fn test_update_opinion_preserves_constraint() {
        let mut opinion = Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap();
        update_opinion_from_projection(&mut opinion, 0.5);
        let sum = opinion.belief + opinion.disbelief + opinion.uncertainty;
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_attack_kind_supersedes_roundtrip() {
        let kind = AttackKind::supersedes(0.75);
        assert!((kind.supersedes_weight() - 0.75).abs() < 1e-4);
    }
}
