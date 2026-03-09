//! SkewRoute: training-free adaptive strategy selection.
//!
//! Analyzes the vector search score distribution (Gini coefficient of
//! top-20 results) to automatically select the best query strategy.
//! Based on Wang et al., May 2025.

use super::strategy::SearchStrategy;

/// Compute the Gini coefficient of a set of scores.
///
/// Returns a value in `[0, 1]` where 0 = perfect equality (all
/// scores identical) and 1 = perfect inequality (one dominant
/// result). Returns 0.0 for empty or single-element inputs.
pub fn gini_coefficient(scores: &[f64]) -> f64 {
    let n = scores.len();
    if n < 2 {
        return 0.0;
    }

    let mut sorted = scores.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let total: f64 = sorted.iter().sum();
    if total == 0.0 {
        return 0.0;
    }

    let n_f = n as f64;
    let weighted_sum: f64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &s)| (i as f64 + 1.0) * s)
        .sum();

    (2.0 * weighted_sum) / (n_f * total) - (n_f + 1.0) / n_f
}

/// Select a strategy based on vector search score distribution.
///
/// - Gini > 0.6: concentrated (one dominant result) -> Precise
/// - Gini < 0.3: diffuse (many equally relevant) -> Global
/// - 0.3 <= Gini <= 0.6: Balanced (default)
/// - Fewer than 5 results: always Balanced (insufficient signal)
pub fn select_strategy(vector_scores: &[f64]) -> SearchStrategy {
    if vector_scores.len() < 5 {
        return SearchStrategy::Balanced;
    }

    let gini = gini_coefficient(vector_scores);

    if gini > 0.6 {
        SearchStrategy::Precise
    } else if gini < 0.3 {
        SearchStrategy::Global
    } else {
        SearchStrategy::Balanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gini_coefficient_uniform() {
        let scores = vec![1.0; 20];
        let g = gini_coefficient(&scores);
        assert!(
            g.abs() < 1e-10,
            "uniform scores should give Gini ~0, got {g}"
        );
    }

    #[test]
    fn gini_coefficient_concentrated() {
        let mut scores = vec![0.0; 19];
        scores.push(100.0);
        let g = gini_coefficient(&scores);
        assert!(g > 0.8, "one dominant score should give high Gini, got {g}");
    }

    #[test]
    fn gini_coefficient_empty() {
        assert_eq!(gini_coefficient(&[]), 0.0);
    }

    #[test]
    fn gini_coefficient_single() {
        assert_eq!(gini_coefficient(&[42.0]), 0.0);
    }

    #[test]
    fn select_strategy_precise_for_concentrated() {
        let mut scores = vec![0.01; 19];
        scores.push(10.0);
        let strategy = select_strategy(&scores);
        assert_eq!(strategy, SearchStrategy::Precise);
    }

    #[test]
    fn select_strategy_global_for_diffuse() {
        let scores = vec![1.0; 20];
        let strategy = select_strategy(&scores);
        assert_eq!(strategy, SearchStrategy::Global);
    }

    #[test]
    fn select_strategy_balanced_for_few_results() {
        let scores = vec![1.0, 2.0, 3.0, 4.0];
        let strategy = select_strategy(&scores);
        assert_eq!(strategy, SearchStrategy::Balanced);
    }

    #[test]
    fn select_strategy_balanced_for_moderate_gini() {
        // Build a distribution with moderate inequality.
        let scores: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let g = gini_coefficient(&scores);
        // Linear 1..20 gives Gini ~0.317, should be Balanced.
        assert!((0.3..=0.6).contains(&g), "expected moderate Gini, got {g}");
        assert_eq!(select_strategy(&scores), SearchStrategy::Balanced);
    }
}
