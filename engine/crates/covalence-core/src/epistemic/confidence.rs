//! Composite confidence computation.
//!
//! Composes source confidence, extraction confidence, and topological
//! confidence into a single composite score for search ranking.
//!
//! Also provides Bayesian confidence aggregation via Beta distribution
//! conjugate updating for combining multiple independent confidence
//! estimates about the same entity.

use crate::error::{Error, Result};
use crate::types::opinion::Opinion;

/// Default weight for topological influence on composite confidence.
pub const DEFAULT_GAMMA: f64 = 0.4;

/// Compute composite confidence from an opinion and topological confidence.
///
/// Formula: `projected_probability(opinion) * (1 + gamma * (topo_confidence - 0.5))`
///
/// The topological confidence adjusts the projected probability up or down
/// based on graph structure — nodes mentioned by many sources via diverse
/// paths receive a boost, while isolated nodes are slightly penalized.
///
/// # Arguments
///
/// * `opinion` - The Subjective Logic opinion for the claim
/// * `topo_confidence` - Topological confidence in `[0, 1]` from graph structure
/// * `gamma` - Weight controlling topological influence (default 0.4)
///
/// # Returns
///
/// Composite confidence clamped to `[0, 1]`.
pub fn composite_confidence(opinion: &Opinion, topo_confidence: f64, gamma: f64) -> f64 {
    let projected = opinion.projected_probability();
    let raw = projected * (1.0 + gamma * (topo_confidence - 0.5));
    raw.clamp(0.0, 1.0)
}

/// Default prior alpha for Bayesian aggregation (uniform prior).
pub const DEFAULT_PRIOR_ALPHA: f64 = 1.0;

/// Default prior beta for Bayesian aggregation (uniform prior).
pub const DEFAULT_PRIOR_BETA: f64 = 1.0;

/// Result of Bayesian confidence aggregation.
///
/// Wraps the posterior Beta distribution parameters alongside the
/// derived posterior mean, which serves as the aggregated confidence.
#[derive(Debug, Clone, Copy)]
pub struct BayesianAggregation {
    /// Posterior alpha parameter.
    pub alpha: f64,
    /// Posterior beta parameter.
    pub beta: f64,
    /// Posterior mean: `alpha / (alpha + beta)`.
    pub mean: f64,
    /// Number of observations incorporated.
    pub observation_count: usize,
}

/// Aggregate multiple independent confidence estimates using Bayesian
/// Beta-distribution conjugate updating.
///
/// Each observation is a `(confidence, weight)` pair where:
/// - `confidence` is in `[0, 1]`, representing the source's estimate
/// - `weight` is a positive value representing source reliability
///   (higher weight = more influential)
///
/// The weight scales the pseudo-observation count: an observation
/// with weight `w` and confidence `c` contributes `w * c` to alpha
/// and `w * (1 - c)` to beta, analogous to observing `w` Bernoulli
/// trials with success rate `c`.
///
/// Starts from a uniform Beta(1, 1) prior by default. Use
/// [`bayesian_aggregate_with_prior`] for a custom prior.
///
/// # Arguments
///
/// * `observations` - Slice of `(confidence, weight)` pairs
///
/// # Errors
///
/// Returns [`Error::InvalidInput`] if any confidence is outside
/// `[0, 1]` or any weight is non-positive.
///
/// # Returns
///
/// A [`BayesianAggregation`] containing the posterior parameters and
/// the posterior mean.
pub fn bayesian_aggregate(observations: &[(f64, f64)]) -> Result<BayesianAggregation> {
    bayesian_aggregate_with_prior(observations, DEFAULT_PRIOR_ALPHA, DEFAULT_PRIOR_BETA)
}

/// Aggregate multiple independent confidence estimates with a custom
/// Beta prior.
///
/// See [`bayesian_aggregate`] for details on observation semantics.
///
/// # Arguments
///
/// * `observations` - Slice of `(confidence, weight)` pairs
/// * `prior_alpha` - Prior alpha parameter (must be positive)
/// * `prior_beta` - Prior beta parameter (must be positive)
///
/// # Errors
///
/// Returns [`Error::InvalidInput`] if:
/// - Any confidence is outside `[0, 1]`
/// - Any weight is non-positive
/// - Prior parameters are non-positive
pub fn bayesian_aggregate_with_prior(
    observations: &[(f64, f64)],
    prior_alpha: f64,
    prior_beta: f64,
) -> Result<BayesianAggregation> {
    if prior_alpha <= 0.0 || prior_beta <= 0.0 {
        return Err(Error::InvalidInput(
            "prior alpha and beta must be positive".to_string(),
        ));
    }

    let mut alpha = prior_alpha;
    let mut beta = prior_beta;

    for (i, &(confidence, weight)) in observations.iter().enumerate() {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(Error::InvalidInput(format!(
                "observation {i}: confidence {confidence} outside [0, 1]"
            )));
        }
        if weight <= 0.0 {
            return Err(Error::InvalidInput(format!(
                "observation {i}: weight {weight} must be positive"
            )));
        }
        alpha += weight * confidence;
        beta += weight * (1.0 - confidence);
    }

    let mean = alpha / (alpha + beta);

    Ok(BayesianAggregation {
        alpha,
        beta,
        mean,
        observation_count: observations.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_neutral_topology_no_adjustment() {
        // topo_confidence = 0.5 means no adjustment
        let opinion = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let projected = opinion.projected_probability(); // 0.7 + 0.5*0.2 = 0.8
        let result = composite_confidence(&opinion, 0.5, DEFAULT_GAMMA);
        assert!((result - projected).abs() < 1e-10);
    }

    #[test]
    fn test_high_topology_boosts() {
        let opinion = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let baseline = composite_confidence(&opinion, 0.5, DEFAULT_GAMMA);
        let boosted = composite_confidence(&opinion, 1.0, DEFAULT_GAMMA);
        assert!(boosted > baseline);
    }

    #[test]
    fn test_low_topology_penalizes() {
        let opinion = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let baseline = composite_confidence(&opinion, 0.5, DEFAULT_GAMMA);
        let penalized = composite_confidence(&opinion, 0.0, DEFAULT_GAMMA);
        assert!(penalized < baseline);
    }

    #[test]
    fn test_zero_gamma_ignores_topology() {
        let opinion = Opinion::new(0.7, 0.1, 0.2, 0.5).unwrap();
        let projected = opinion.projected_probability();
        let result = composite_confidence(&opinion, 1.0, 0.0);
        assert!((result - projected).abs() < 1e-10);
    }

    #[test]
    fn test_vacuous_opinion() {
        let opinion = Opinion::vacuous(0.5);
        // projected = 0 + 0.5 * 1.0 = 0.5
        let result = composite_confidence(&opinion, 0.5, DEFAULT_GAMMA);
        assert!((result - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_certain_opinion_full_topology() {
        let opinion = Opinion::certain(1.0);
        // projected = 1.0, topo = 1.0
        // raw = 1.0 * (1 + 0.4 * 0.5) = 1.2 -> clamped to 1.0
        let result = composite_confidence(&opinion, 1.0, DEFAULT_GAMMA);
        assert!((result - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_result_clamped_to_zero() {
        // Edge case: very low projected probability with heavy penalty
        let opinion = Opinion::new(0.0, 0.9, 0.1, 0.01).unwrap();
        // projected = 0.0 + 0.01 * 0.1 = 0.001
        // With large negative gamma and low topo, could theoretically go negative
        let result = composite_confidence(&opinion, 0.0, 10.0);
        assert!(result >= 0.0);
    }

    #[test]
    fn test_specific_values() {
        let opinion = Opinion::new(0.6, 0.2, 0.2, 0.5).unwrap();
        // projected = 0.6 + 0.5 * 0.2 = 0.7
        // composite = 0.7 * (1 + 0.4 * (0.8 - 0.5)) = 0.7 * 1.12 = 0.784
        let result = composite_confidence(&opinion, 0.8, DEFAULT_GAMMA);
        assert!((result - 0.784).abs() < 1e-10);
    }

    // --- Bayesian aggregation tests ---

    #[test]
    fn test_bayesian_no_observations() {
        // With no observations, posterior = prior Beta(1,1) -> mean 0.5
        let agg = bayesian_aggregate(&[]).unwrap();
        assert!((agg.mean - 0.5).abs() < 1e-10);
        assert!((agg.alpha - 1.0).abs() < 1e-10);
        assert!((agg.beta - 1.0).abs() < 1e-10);
        assert_eq!(agg.observation_count, 0);
    }

    #[test]
    fn test_bayesian_single_observation() {
        // One observation (0.8, weight=1.0)
        // alpha = 1 + 1*0.8 = 1.8
        // beta  = 1 + 1*0.2 = 1.2
        // mean  = 1.8 / 3.0 = 0.6
        let agg = bayesian_aggregate(&[(0.8, 1.0)]).unwrap();
        assert!((agg.alpha - 1.8).abs() < 1e-10);
        assert!((agg.beta - 1.2).abs() < 1e-10);
        assert!((agg.mean - 0.6).abs() < 1e-10);
        assert_eq!(agg.observation_count, 1);
    }

    #[test]
    fn test_bayesian_multiple_observations() {
        // Two observations: (0.9, 1.0) and (0.7, 1.0)
        // alpha = 1 + 0.9 + 0.7 = 2.6
        // beta  = 1 + 0.1 + 0.3 = 1.4
        // mean  = 2.6 / 4.0 = 0.65
        let agg = bayesian_aggregate(&[(0.9, 1.0), (0.7, 1.0)]).unwrap();
        assert!((agg.alpha - 2.6).abs() < 1e-10);
        assert!((agg.beta - 1.4).abs() < 1e-10);
        assert!((agg.mean - 0.65).abs() < 1e-10);
        assert_eq!(agg.observation_count, 2);
    }

    #[test]
    fn test_bayesian_weighted_observations() {
        // High-weight observation dominates
        // (0.9, 10.0) and (0.1, 1.0)
        // alpha = 1 + 10*0.9 + 1*0.1 = 10.1
        // beta  = 1 + 10*0.1 + 1*0.9 = 2.9
        // mean  = 10.1 / 13.0
        let agg = bayesian_aggregate(&[(0.9, 10.0), (0.1, 1.0)]).unwrap();
        assert!((agg.alpha - 10.1).abs() < 1e-10);
        assert!((agg.beta - 2.9).abs() < 1e-10);
        let expected_mean = 10.1 / 13.0;
        assert!((agg.mean - expected_mean).abs() < 1e-10);
    }

    #[test]
    fn test_bayesian_high_weight_pulls_mean() {
        // A reliable source (high weight) with confidence 0.9 should
        // pull the mean closer to 0.9 than a low-weight source at 0.1
        let agg_high = bayesian_aggregate(&[(0.9, 10.0), (0.1, 1.0)]).unwrap();
        let agg_low = bayesian_aggregate(&[(0.9, 1.0), (0.1, 10.0)]).unwrap();
        assert!(agg_high.mean > agg_low.mean);
    }

    #[test]
    fn test_bayesian_all_ones() {
        // All confidence = 1.0
        // alpha = 1 + 3*1.0 = 4.0
        // beta  = 1 + 3*0.0 = 1.0
        // mean  = 4/5 = 0.8
        let agg = bayesian_aggregate(&[(1.0, 1.0), (1.0, 1.0), (1.0, 1.0)]).unwrap();
        assert!((agg.mean - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_bayesian_all_zeros() {
        // All confidence = 0.0
        // alpha = 1 + 0 = 1.0
        // beta  = 1 + 3 = 4.0
        // mean  = 1/5 = 0.2
        let agg = bayesian_aggregate(&[(0.0, 1.0), (0.0, 1.0), (0.0, 1.0)]).unwrap();
        assert!((agg.mean - 0.2).abs() < 1e-10);
    }

    #[test]
    fn test_bayesian_custom_prior() {
        // Informative prior Beta(5, 5) -> prior mean 0.5
        // One strong observation (0.9, 5.0)
        // alpha = 5 + 5*0.9 = 9.5
        // beta  = 5 + 5*0.1 = 5.5
        // mean  = 9.5 / 15.0
        let agg = bayesian_aggregate_with_prior(&[(0.9, 5.0)], 5.0, 5.0).unwrap();
        assert!((agg.alpha - 9.5).abs() < 1e-10);
        assert!((agg.beta - 5.5).abs() < 1e-10);
        let expected = 9.5 / 15.0;
        assert!((agg.mean - expected).abs() < 1e-10);
    }

    #[test]
    fn test_bayesian_invalid_confidence_too_high() {
        let result = bayesian_aggregate(&[(1.5, 1.0)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_bayesian_invalid_confidence_negative() {
        let result = bayesian_aggregate(&[(-0.1, 1.0)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_bayesian_invalid_weight_zero() {
        let result = bayesian_aggregate(&[(0.5, 0.0)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_bayesian_invalid_weight_negative() {
        let result = bayesian_aggregate(&[(0.5, -1.0)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_bayesian_invalid_prior() {
        let result = bayesian_aggregate_with_prior(&[(0.5, 1.0)], 0.0, 1.0);
        assert!(result.is_err());

        let result = bayesian_aggregate_with_prior(&[(0.5, 1.0)], 1.0, -1.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_bayesian_convergence_with_many_observations() {
        // With many observations at 0.8, mean should converge
        // toward 0.8 regardless of prior
        let obs: Vec<(f64, f64)> = (0..100).map(|_| (0.8, 1.0)).collect();
        let agg = bayesian_aggregate(&obs).unwrap();
        assert!((agg.mean - 0.8).abs() < 0.01);
    }
}
