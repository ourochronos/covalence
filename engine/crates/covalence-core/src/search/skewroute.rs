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
    // Filter non-finite values (NaN, Inf) before computing.
    let mut sorted: Vec<f64> = scores.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.len() < 2 {
        return 0.0;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let total: f64 = sorted.iter().sum();
    if total == 0.0 {
        return 0.0;
    }

    let n_f = sorted.len() as f64;
    let weighted_sum: f64 = sorted
        .iter()
        .enumerate()
        .map(|(i, &s)| (i as f64 + 1.0) * s)
        .sum();

    (2.0 * weighted_sum) / (n_f * total) - (n_f + 1.0) / n_f
}

/// Min-max normalize scores to the [0, 1] range.
///
/// This makes the Gini coefficient model-agnostic: embedding
/// models with narrow score ranges (e.g., Voyage cosine 0.7–0.9)
/// are rescaled to use the full [0, 1] range, amplifying relative
/// differences between results.
fn normalize_scores(scores: &[f64]) -> Vec<f64> {
    if scores.len() < 2 {
        return scores.to_vec();
    }
    let min = scores.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    if range < 1e-12 {
        // All scores identical → perfectly uniform.
        return vec![0.0; scores.len()];
    }
    scores.iter().map(|&s| (s - min) / range).collect()
}

/// Select a strategy based on vector search score distribution.
///
/// Scores are min-max normalized before computing the Gini
/// coefficient. This makes the selection model-agnostic —
/// embedding models with narrow score ranges (like Voyage's
/// cosine similarity in 0.7–0.9) are properly discriminated.
///
/// - Gini > 0.5: concentrated (one dominant result) -> Precise
/// - Gini < 0.15: diffuse (truly uniform scores) -> Global
/// - 0.15 <= Gini <= 0.5: Balanced (default)
/// - Fewer than 5 results: always Balanced (insufficient signal)
///
/// Thresholds are calibrated for min-max normalized scores.
/// Before normalization, embedding models with narrow score
/// ranges (Voyage: 0.7-0.9) produced Gini ~0.05 for all queries.
/// After normalization, typical queries produce Gini 0.2-0.4,
/// truly uniform results give 0.0, and concentrated results
/// give > 0.5.
pub fn select_strategy(vector_scores: &[f64]) -> SearchStrategy {
    if vector_scores.len() < 5 {
        return SearchStrategy::Balanced;
    }

    let normalized = normalize_scores(vector_scores);
    let gini = gini_coefficient(&normalized);

    tracing::debug!(
        raw_gini = gini_coefficient(vector_scores),
        normalized_gini = gini,
        n_scores = vector_scores.len(),
        "skewroute gini analysis"
    );

    if gini > 0.5 {
        SearchStrategy::Precise
    } else if gini < 0.15 {
        SearchStrategy::Global
    } else {
        SearchStrategy::Balanced
    }
}

/// Keywords that signal a recency-focused query.
const RECENCY_KEYWORDS: &[&str] = &[
    "latest", "recent", "newest", "new", "last",
    "today", "yesterday", "updated", "fresh",
];

/// Keywords that signal an entity/exploratory query.
const ENTITY_KEYWORDS: &[&str] = &[
    "what is", "who is", "define", "explain",
    "tell me about", "describe", "overview of",
];

/// Detect query intent from keywords to supplement score-based
/// SkewRoute.
///
/// Returns `Some(strategy)` if keywords clearly indicate intent,
/// or `None` to defer to the score-based selector.
///
/// This is a lightweight heuristic — it only fires for clear
/// signals. Ambiguous queries fall through to SkewRoute.
pub fn detect_intent(query: &str) -> Option<SearchStrategy> {
    let q = query.to_lowercase();

    // Check for recency signals first (strongest signal).
    let has_recency = RECENCY_KEYWORDS.iter().any(|kw| q.contains(kw));
    if has_recency {
        return Some(SearchStrategy::Recent);
    }

    // Check for entity/definition queries → Exploratory is
    // better than Balanced because these benefit from graph
    // context (the entity's relationships).
    let has_entity = ENTITY_KEYWORDS.iter().any(|kw| q.contains(kw));
    if has_entity {
        return Some(SearchStrategy::Exploratory);
    }

    None
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
        let normalized = normalize_scores(&scores);
        let g = gini_coefficient(&normalized);
        // Linear 1..20 normalized gives Gini ~0.333, should be Balanced.
        assert!((0.3..=0.6).contains(&g), "expected moderate Gini, got {g}");
        assert_eq!(select_strategy(&scores), SearchStrategy::Balanced);
    }

    #[test]
    fn normalize_scores_narrow_range() {
        // Voyage-like scores: everything in 0.78-0.82
        let scores = vec![0.78, 0.79, 0.80, 0.81, 0.82];
        let normalized = normalize_scores(&scores);
        assert!((normalized[0] - 0.0).abs() < 1e-10);
        assert!((normalized[4] - 1.0).abs() < 1e-10);
        // This should give moderate Gini on the normalized scores
        let g = gini_coefficient(&normalized);
        assert!(g > 0.1, "normalized narrow range should not be near-zero Gini, got {g}");
    }

    #[test]
    fn normalize_scores_identical() {
        let scores = vec![0.85; 10];
        let normalized = normalize_scores(&scores);
        // All identical → all zero
        assert!(normalized.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn detect_intent_recency() {
        assert_eq!(
            detect_intent("latest research on GraphRAG"),
            Some(SearchStrategy::Recent)
        );
        assert_eq!(
            detect_intent("what are the most recent papers"),
            Some(SearchStrategy::Recent)
        );
    }

    #[test]
    fn detect_intent_entity() {
        assert_eq!(
            detect_intent("what is subjective logic"),
            Some(SearchStrategy::Exploratory)
        );
        assert_eq!(
            detect_intent("explain entity resolution"),
            Some(SearchStrategy::Exploratory)
        );
        assert_eq!(
            detect_intent("tell me about GraphRAG"),
            Some(SearchStrategy::Exploratory)
        );
    }

    #[test]
    fn detect_intent_none_for_ambiguous() {
        assert_eq!(detect_intent("search result quality"), None);
        assert_eq!(detect_intent("how to improve chunking"), None);
        assert_eq!(detect_intent("entity resolution algorithms"), None);
    }

    #[test]
    fn detect_intent_recency_over_entity() {
        // Recency takes priority when both signals present.
        assert_eq!(
            detect_intent("what is the latest research"),
            Some(SearchStrategy::Recent)
        );
    }

    #[test]
    fn gini_coefficient_filters_nan() {
        // NaN and Inf values should be filtered, not corrupt the
        // result.
        let scores = vec![1.0, 2.0, f64::NAN, 3.0, f64::INFINITY];
        let g = gini_coefficient(&scores);
        assert!(g.is_finite(), "gini with NaN/Inf input should be finite, got {g}");
        // Should equal gini of [1.0, 2.0, 3.0]
        let clean = vec![1.0, 2.0, 3.0];
        let g_clean = gini_coefficient(&clean);
        assert!(
            (g - g_clean).abs() < 1e-10,
            "gini should match clean scores: {g} vs {g_clean}"
        );
    }

    #[test]
    fn gini_coefficient_all_nan() {
        let scores = vec![f64::NAN, f64::NAN, f64::INFINITY];
        let g = gini_coefficient(&scores);
        assert_eq!(g, 0.0, "all non-finite scores should return 0.0");
    }

    #[test]
    fn narrow_range_not_always_global() {
        // Voyage-like: narrow range with moderate spread
        let scores: Vec<f64> = (0..20)
            .map(|i| 0.78 + (i as f64) * 0.01)
            .collect();
        // With raw Gini this would be near-zero (Global).
        // With normalization, the linear spread should give Balanced.
        let strategy = select_strategy(&scores);
        assert_eq!(
            strategy,
            SearchStrategy::Balanced,
            "narrow linear spread should be Balanced, not Global"
        );
    }
}
