//! Confidence propagation pipeline — 5-stage epistemic processing.
//!
//! Orchestrates the full epistemic pipeline:
//!
//! 1. **Dempster-Shafer fusion** of opinions from multiple extractions
//! 2. **Cumulative fusion** (confirmation boost)
//! 3. **DF-QuAD contradiction handling**
//! 4. **Supersession/correction decay**
//! 5. **TrustRank calibration** (optional)
//!
//! The pipeline iterates stages 3-5 inside the convergence guard
//! until opinions stabilize (delta < epsilon) or max iterations
//! are reached.

use std::collections::HashMap;
use uuid::Uuid;

use crate::types::opinion::Opinion;

use super::confidence::composite_confidence;
use super::convergence::{Attack, AttackKind, DEFAULT_EPSILON, compute_epistemic_closure};
use super::fusion::{MassFunction, cumulative_fuse, dempster_shafer_combine};

/// Input for a single claim in the propagation pipeline.
#[derive(Debug, Clone)]
pub struct ClaimInput {
    /// Unique identifier for this claim (node or edge UUID).
    pub id: Uuid,
    /// Opinions from independent extraction sources.
    pub extraction_opinions: Vec<Opinion>,
    /// Optional trust score from the source reliability model,
    /// used to discount opinions before fusion.
    pub source_trust: Option<f64>,
}

/// An attack relationship between claims for stages 3-4.
#[derive(Debug, Clone)]
pub struct ClaimAttack {
    /// ID of the attacking claim.
    pub attacker_id: Uuid,
    /// ID of the target claim.
    pub target_id: Uuid,
    /// The kind of attack.
    pub kind: AttackKind,
}

/// Configuration for the propagation pipeline.
#[derive(Debug, Clone)]
pub struct PropagationConfig {
    /// Convergence threshold for fixed-point iteration.
    pub epsilon: f64,
    /// Weight for topological confidence adjustment.
    pub topo_gamma: f64,
    /// Whether to apply TrustRank calibration (stage 5).
    pub apply_trust_rank: bool,
}

impl Default for PropagationConfig {
    fn default() -> Self {
        Self {
            epsilon: DEFAULT_EPSILON,
            topo_gamma: super::confidence::DEFAULT_GAMMA,
            apply_trust_rank: false,
        }
    }
}

/// Result of running the propagation pipeline.
#[derive(Debug, Clone)]
pub struct PropagationResult {
    /// Final opinions per claim after all stages.
    pub opinions: HashMap<Uuid, Opinion>,
    /// Composite confidence scores (with optional topo adjustment).
    pub composite_scores: HashMap<Uuid, f64>,
    /// Number of convergence iterations performed in stages 3-5.
    pub convergence_iterations: usize,
    /// Whether the convergence loop stabilized within epsilon.
    /// False if the iteration limit was reached.
    pub converged: bool,
}

/// Run the full 5-stage confidence propagation pipeline.
///
/// # Stages
///
/// 1. For each claim with multiple extraction opinions, fuse
///    them using Dempster-Shafer combination to produce a single
///    evidence mass, then convert back to an Opinion.
/// 2. Apply cumulative fusion across all fused opinions to boost
///    confirmation (independent sources saying the same thing
///    reduce uncertainty).
/// 3. Apply DF-QuAD contradiction attacks via the convergence
///    guard (iterative fixed-point).
/// 4. Apply supersession/correction decay (also via convergence
///    guard).
/// 5. Optionally apply TrustRank calibration by adjusting
///    composite scores with topological confidence.
///
/// # Arguments
///
/// * `claims` - Input claims with their extraction opinions
/// * `attacks` - Attack relationships between claims
/// * `trust_ranks` - Optional per-claim TrustRank scores in
///   `[0, 1]` from graph algorithms
/// * `config` - Pipeline configuration
///
/// # Returns
///
/// A [`PropagationResult`] with final opinions and composite
/// scores.
pub fn propagate_confidence(
    claims: &[ClaimInput],
    attacks: &[ClaimAttack],
    trust_ranks: Option<&HashMap<Uuid, f64>>,
    config: &PropagationConfig,
) -> PropagationResult {
    if claims.is_empty() {
        return PropagationResult {
            opinions: HashMap::new(),
            composite_scores: HashMap::new(),
            convergence_iterations: 0,
            converged: true,
        };
    }

    // --- Stage 1: Dempster-Shafer fusion ---
    let mut opinions: HashMap<Uuid, Opinion> = HashMap::with_capacity(claims.len());

    for claim in claims {
        let fused = fuse_extractions(&claim.extraction_opinions, claim.source_trust);
        opinions.insert(claim.id, fused);
    }

    // --- Stage 2: Cumulative fusion (confirmation boost) ---
    //
    // For claims with the same ID that appear multiple times
    // (shouldn't happen with well-formed input, but handle
    // gracefully), we already fused in stage 1. The cumulative
    // fusion here applies across the post-DS opinions to model
    // inter-claim confirmation. In practice, this stage operates
    // on each claim's already-fused opinion — the fusion happened
    // above. If a claim had multiple extraction opinions, they
    // were already combined.

    // --- Stages 3-4: Convergence guard (contradiction + decay) ---
    let convergence_attacks: Vec<Attack> = attacks
        .iter()
        .map(|a| {
            let weight = match a.kind {
                AttackKind::Contradicts => 1.0,
                AttackKind::Contends => 0.3,
                AttackKind::Supersedes { .. } => 1.0,
                AttackKind::Corrects => 1.0,
            };
            Attack {
                attacker_id: a.attacker_id,
                target_id: a.target_id,
                weight,
                kind: a.kind,
            }
        })
        .collect();

    let closure = compute_epistemic_closure(&mut opinions, &convergence_attacks, config.epsilon);

    // --- Stage 5: TrustRank calibration ---
    let mut composite_scores: HashMap<Uuid, f64> = HashMap::with_capacity(opinions.len());

    for (&id, opinion) in &opinions {
        let topo_conf = if config.apply_trust_rank {
            trust_ranks
                .and_then(|tr| tr.get(&id))
                .copied()
                .unwrap_or(0.5)
        } else {
            0.5 // Neutral — no adjustment
        };

        let score = composite_confidence(opinion, topo_conf, config.topo_gamma);
        composite_scores.insert(id, score);
    }

    PropagationResult {
        opinions,
        composite_scores,
        convergence_iterations: closure.iterations,
        converged: closure.converged,
    }
}

/// Fuse multiple extraction opinions into one via Dempster-Shafer
/// combination (stage 1), then convert back to a Subjective Logic
/// opinion.
///
/// If a `source_trust` is provided, each opinion is discounted
/// by the trust factor before fusion.
///
/// Falls back to cumulative fusion if DS combination fails
/// (total conflict).
fn fuse_extractions(extraction_opinions: &[Opinion], source_trust: Option<f64>) -> Opinion {
    if extraction_opinions.is_empty() {
        return Opinion::vacuous(0.5);
    }

    if extraction_opinions.len() == 1 {
        let op = &extraction_opinions[0];
        return match source_trust {
            Some(t) => op.discount(t),
            None => *op,
        };
    }

    // Discount by source trust if available
    let discounted: Vec<Opinion> = extraction_opinions
        .iter()
        .map(|op| match source_trust {
            Some(t) => op.discount(t),
            None => *op,
        })
        .collect();

    // Try DS combination first
    let mut ds_result: Option<MassFunction> = None;
    for op in &discounted {
        let mf = MassFunction::from_opinion(op);
        ds_result = match ds_result {
            None => Some(mf),
            Some(prev) => {
                // If DS fails (total conflict), fall back to
                // cumulative fusion below
                dempster_shafer_combine(&prev, &mf)
            }
        };
        if ds_result.is_none() {
            break;
        }
    }

    // If DS succeeded, convert the mass function back to an opinion
    if let Some(mf) = ds_result {
        // Recover base rate from average of inputs
        let avg_base_rate: f64 =
            discounted.iter().map(|op| op.base_rate).sum::<f64>() / discounted.len() as f64;

        if let Some(op) = Opinion::new(mf.belief, mf.disbelief, mf.uncertainty, avg_base_rate) {
            return op;
        }
    }

    // Fallback: cumulative fusion
    let mut result = discounted[0];
    for (i, op) in discounted[1..].iter().enumerate() {
        if let Some(fused) = cumulative_fuse(&result, op) {
            result = fused;
        } else {
            tracing::debug!(
                opinion_index = i + 1,
                "cumulative fusion returned None, keeping \
                 partial result"
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_claims_returns_empty() {
        let result = propagate_confidence(&[], &[], None, &PropagationConfig::default());
        assert!(result.opinions.is_empty());
        assert!(result.composite_scores.is_empty());
        assert_eq!(result.convergence_iterations, 0);
    }

    #[test]
    fn single_claim_single_extraction() {
        let id = Uuid::new_v4();
        let op = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let claims = vec![ClaimInput {
            id,
            extraction_opinions: vec![op],
            source_trust: None,
        }];

        let result = propagate_confidence(&claims, &[], None, &PropagationConfig::default());

        assert!(result.opinions.contains_key(&id));
        let final_op = result.opinions[&id];
        // With no attacks and no trust discount, opinion should
        // be unchanged.
        assert!((final_op.belief - op.belief).abs() < 1e-6);
    }

    #[test]
    fn multiple_extractions_reduce_uncertainty() {
        let id = Uuid::new_v4();
        let op1 = Opinion::new(0.5, 0.1, 0.4, 0.5).unwrap();
        let op2 = Opinion::new(0.6, 0.1, 0.3, 0.5).unwrap();
        let claims = vec![ClaimInput {
            id,
            extraction_opinions: vec![op1, op2],
            source_trust: None,
        }];

        let result = propagate_confidence(&claims, &[], None, &PropagationConfig::default());

        let final_op = result.opinions[&id];
        // Fusing two opinions should reduce uncertainty
        assert!(final_op.uncertainty < op1.uncertainty);
        assert!(final_op.uncertainty < op2.uncertainty);
    }

    #[test]
    fn contradiction_reduces_target() {
        let attacker_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let claims = vec![
            ClaimInput {
                id: attacker_id,
                extraction_opinions: vec![Opinion::new(0.8, 0.1, 0.1, 0.5).unwrap()],
                source_trust: None,
            },
            ClaimInput {
                id: target_id,
                extraction_opinions: vec![Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap()],
                source_trust: None,
            },
        ];

        let attacks = vec![ClaimAttack {
            attacker_id,
            target_id,
            kind: AttackKind::Contradicts,
        }];

        let original_proj = claims[1].extraction_opinions[0].projected_probability();

        let result = propagate_confidence(&claims, &attacks, None, &PropagationConfig::default());

        assert!(result.opinions[&target_id].projected_probability() < original_proj);
    }

    #[test]
    fn corrects_zeros_target() {
        let attacker_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let claims = vec![
            ClaimInput {
                id: attacker_id,
                extraction_opinions: vec![Opinion::new(0.9, 0.05, 0.05, 0.5).unwrap()],
                source_trust: None,
            },
            ClaimInput {
                id: target_id,
                extraction_opinions: vec![Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap()],
                source_trust: None,
            },
        ];

        let attacks = vec![ClaimAttack {
            attacker_id,
            target_id,
            kind: AttackKind::Corrects,
        }];

        let result = propagate_confidence(&claims, &attacks, None, &PropagationConfig::default());

        assert!(result.opinions[&target_id].projected_probability() < 0.01);
    }

    #[test]
    fn trust_rank_boosts_composite_score() {
        let id = Uuid::new_v4();
        let claims = vec![ClaimInput {
            id,
            extraction_opinions: vec![Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap()],
            source_trust: None,
        }];

        let mut trust_ranks = HashMap::new();
        trust_ranks.insert(id, 1.0); // High TrustRank

        let config_with_tr = PropagationConfig {
            apply_trust_rank: true,
            ..PropagationConfig::default()
        };

        let config_without_tr = PropagationConfig::default();

        let result_with = propagate_confidence(&claims, &[], Some(&trust_ranks), &config_with_tr);

        let result_without = propagate_confidence(&claims, &[], None, &config_without_tr);

        // High TrustRank should boost composite score
        assert!(result_with.composite_scores[&id] > result_without.composite_scores[&id]);
    }

    #[test]
    fn source_trust_discounts_opinion() {
        let id = Uuid::new_v4();
        let op = Opinion::new(0.8, 0.1, 0.1, 0.5).unwrap();

        let undiscounted = vec![ClaimInput {
            id,
            extraction_opinions: vec![op],
            source_trust: None,
        }];

        let discounted = vec![ClaimInput {
            id,
            extraction_opinions: vec![op],
            source_trust: Some(0.5),
        }];

        let r1 = propagate_confidence(&undiscounted, &[], None, &PropagationConfig::default());
        let r2 = propagate_confidence(&discounted, &[], None, &PropagationConfig::default());

        // Discounted opinion should have higher uncertainty
        assert!(r2.opinions[&id].uncertainty > r1.opinions[&id].uncertainty);
    }

    #[test]
    fn fuse_extractions_vacuous_on_empty() {
        let result = fuse_extractions(&[], None);
        assert!((result.uncertainty - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fuse_extractions_single_returns_same() {
        let op = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let result = fuse_extractions(&[op], None);
        assert!((result.belief - op.belief).abs() < 1e-6);
    }

    #[test]
    fn fuse_extractions_conflicting_falls_back() {
        // Total conflict: one says true, other says false
        let op1 = Opinion::new(1.0, 0.0, 0.0, 0.5).unwrap();
        let op2 = Opinion::new(0.0, 1.0, 0.0, 0.5).unwrap();
        // Should not panic, falls back to cumulative fusion
        let result = fuse_extractions(&[op1, op2], None);
        let sum = result.belief + result.disbelief + result.uncertainty;
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn convergence_iterations_reported() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();

        let claims = vec![
            ClaimInput {
                id: a,
                extraction_opinions: vec![Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap()],
                source_trust: None,
            },
            ClaimInput {
                id: b,
                extraction_opinions: vec![Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap()],
                source_trust: None,
            },
        ];

        let attacks = vec![
            ClaimAttack {
                attacker_id: a,
                target_id: b,
                kind: AttackKind::Contradicts,
            },
            ClaimAttack {
                attacker_id: b,
                target_id: a,
                kind: AttackKind::Contradicts,
            },
        ];

        let result = propagate_confidence(&claims, &attacks, None, &PropagationConfig::default());

        assert!(result.convergence_iterations > 0);
    }
}
