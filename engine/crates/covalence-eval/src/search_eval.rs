//! Search layer evaluator.
//!
//! Evaluates ranked search results against ground-truth relevance
//! judgments, producing P@K, nDCG, and MRR metrics.

use crate::LayerEvaluator;
use crate::metrics::SearchMetrics;

/// Configuration for the search evaluator.
#[derive(Debug, Clone)]
pub struct SearchEval {
    /// K value for Precision@K.
    pub k: usize,
}

impl SearchEval {
    /// Create a new search evaluator with the given K.
    pub fn new(k: usize) -> Self {
        Self { k }
    }
}

impl Default for SearchEval {
    fn default() -> Self {
        Self { k: 10 }
    }
}

/// A ranked search result for evaluation.
#[derive(Debug, Clone)]
pub struct RankedResult {
    /// Identifier of the result (string form of UUID).
    pub id: String,
    /// Score assigned by the search system.
    pub score: f64,
}

/// Input to the search evaluator.
#[derive(Debug, Clone)]
pub struct SearchInput {
    /// The search query.
    pub query: String,
}

/// Output from the search evaluator.
#[derive(Debug, Clone)]
pub struct SearchOutput {
    /// Ranked list of results.
    pub results: Vec<RankedResult>,
    /// Relevance grades per result ID (from ground truth).
    /// Maps result ID to relevance grade (0 = irrelevant).
    pub relevance: Vec<(String, u32)>,
}

impl LayerEvaluator for SearchEval {
    type Input = SearchInput;
    type Output = SearchOutput;
    type Metrics = SearchMetrics;

    fn evaluate(&self, _input: &Self::Input) -> Self::Output {
        // Search evaluation requires a running search engine.
        // In evaluation mode, callers supply actual results and
        // ground truth to `score()`.
        SearchOutput {
            results: Vec::new(),
            relevance: Vec::new(),
        }
    }

    fn score(&self, output: &Self::Output, expected: &Self::Output) -> Self::Metrics {
        compute_search_metrics(&output.results, &expected.relevance, self.k)
    }
}

/// Compute search quality metrics from ranked results and
/// relevance judgments.
///
/// - **P@K**: fraction of top-K results that are relevant
///   (grade > 0).
/// - **nDCG**: normalized DCG using the provided relevance
///   grades, comparing to the ideal ranking.
/// - **MRR**: reciprocal of the rank of the first relevant
///   result.
pub fn compute_search_metrics(
    results: &[RankedResult],
    relevance: &[(String, u32)],
    k: usize,
) -> SearchMetrics {
    let rel_map: std::collections::HashMap<&str, u32> = relevance
        .iter()
        .map(|(id, grade)| (id.as_str(), *grade))
        .collect();

    let top_k: Vec<&RankedResult> = results.iter().take(k).collect();

    // Precision@K
    let relevant_in_top_k = top_k
        .iter()
        .filter(|r| rel_map.get(r.id.as_str()).copied().unwrap_or(0) > 0)
        .count();
    let precision_at_k = if top_k.is_empty() {
        0.0
    } else {
        relevant_in_top_k as f64 / top_k.len() as f64
    };

    // nDCG
    let ndcg = compute_ndcg(results, &rel_map, k);

    // MRR
    let mrr = compute_mrr(results, &rel_map);

    SearchMetrics {
        precision_at_k,
        ndcg,
        mrr,
        result_count: results.len(),
        k,
    }
}

/// Compute normalized Discounted Cumulative Gain at K.
fn compute_ndcg(
    results: &[RankedResult],
    rel_map: &std::collections::HashMap<&str, u32>,
    k: usize,
) -> f64 {
    let dcg = compute_dcg(results, rel_map, k);

    // Ideal DCG: sort relevance grades descending
    let mut ideal_grades: Vec<u32> = rel_map.values().copied().collect();
    ideal_grades.sort_unstable_by(|a, b| b.cmp(a));

    let idcg: f64 = ideal_grades
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &grade)| grade as f64 / (2.0_f64 + i as f64).log2())
        .sum();

    if idcg > 0.0 { dcg / idcg } else { 0.0 }
}

/// Compute Discounted Cumulative Gain at K.
fn compute_dcg(
    results: &[RankedResult],
    rel_map: &std::collections::HashMap<&str, u32>,
    k: usize,
) -> f64 {
    results
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, r)| {
            let grade = rel_map.get(r.id.as_str()).copied().unwrap_or(0);
            grade as f64 / (2.0_f64 + i as f64).log2()
        })
        .sum()
}

/// Compute Mean Reciprocal Rank.
fn compute_mrr(results: &[RankedResult], rel_map: &std::collections::HashMap<&str, u32>) -> f64 {
    for (i, r) in results.iter().enumerate() {
        let grade = rel_map.get(r.id.as_str()).copied().unwrap_or(0);
        if grade > 0 {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked(id: &str, score: f64) -> RankedResult {
        RankedResult {
            id: id.to_string(),
            score,
        }
    }

    #[test]
    fn perfect_precision_at_k() {
        let results = vec![ranked("a", 0.9), ranked("b", 0.8)];
        let relevance = vec![("a".to_string(), 3), ("b".to_string(), 2)];
        let metrics = compute_search_metrics(&results, &relevance, 2);
        assert_eq!(metrics.precision_at_k, 1.0);
    }

    #[test]
    fn zero_precision_when_irrelevant() {
        let results = vec![ranked("x", 0.9), ranked("y", 0.8)];
        let relevance = vec![("a".to_string(), 3), ("b".to_string(), 2)];
        let metrics = compute_search_metrics(&results, &relevance, 2);
        assert_eq!(metrics.precision_at_k, 0.0);
    }

    #[test]
    fn partial_precision() {
        let results = vec![
            ranked("a", 0.9),
            ranked("x", 0.8),
            ranked("b", 0.7),
            ranked("y", 0.6),
        ];
        let relevance = vec![("a".to_string(), 3), ("b".to_string(), 2)];
        let metrics = compute_search_metrics(&results, &relevance, 4);
        assert!((metrics.precision_at_k - 0.5).abs() < 1e-10);
    }

    #[test]
    fn mrr_first_result_relevant() {
        let results = vec![ranked("a", 0.9), ranked("b", 0.8)];
        let relevance = vec![("a".to_string(), 1)];
        let metrics = compute_search_metrics(&results, &relevance, 10);
        assert_eq!(metrics.mrr, 1.0);
    }

    #[test]
    fn mrr_second_result_relevant() {
        let results = vec![ranked("x", 0.9), ranked("a", 0.8)];
        let relevance = vec![("a".to_string(), 1)];
        let metrics = compute_search_metrics(&results, &relevance, 10);
        assert!((metrics.mrr - 0.5).abs() < 1e-10);
    }

    #[test]
    fn mrr_none_relevant() {
        let results = vec![ranked("x", 0.9)];
        let relevance = vec![("a".to_string(), 1)];
        let metrics = compute_search_metrics(&results, &relevance, 10);
        assert_eq!(metrics.mrr, 0.0);
    }

    #[test]
    fn ndcg_perfect_ranking() {
        let results = vec![ranked("a", 0.9), ranked("b", 0.8)];
        let relevance = vec![("a".to_string(), 3), ("b".to_string(), 1)];
        let metrics = compute_search_metrics(&results, &relevance, 2);
        // Perfect ranking -> nDCG = 1.0
        assert!((metrics.ndcg - 1.0).abs() < 1e-10);
    }

    #[test]
    fn ndcg_reversed_ranking() {
        let results = vec![ranked("b", 0.9), ranked("a", 0.8)];
        let relevance = vec![("a".to_string(), 3), ("b".to_string(), 1)];
        let metrics = compute_search_metrics(&results, &relevance, 2);
        // Reversed ranking -> nDCG < 1.0
        assert!(metrics.ndcg < 1.0);
        assert!(metrics.ndcg > 0.0);
    }

    #[test]
    fn empty_results() {
        let results: Vec<RankedResult> = vec![];
        let relevance = vec![("a".to_string(), 1)];
        let metrics = compute_search_metrics(&results, &relevance, 10);
        assert_eq!(metrics.precision_at_k, 0.0);
        assert_eq!(metrics.mrr, 0.0);
        assert_eq!(metrics.ndcg, 0.0);
        assert_eq!(metrics.result_count, 0);
    }

    #[test]
    fn k_limits_precision_window() {
        let results = vec![ranked("x", 0.9), ranked("a", 0.8)];
        let relevance = vec![("a".to_string(), 1)];
        let at_1 = compute_search_metrics(&results, &relevance, 1);
        let at_2 = compute_search_metrics(&results, &relevance, 2);
        assert_eq!(at_1.precision_at_k, 0.0);
        assert!((at_2.precision_at_k - 0.5).abs() < 1e-10);
    }

    #[test]
    fn evaluate_returns_empty_passthrough() {
        let eval = SearchEval::default();
        let input = SearchInput {
            query: "test query".to_string(),
        };
        let output = eval.evaluate(&input);
        assert!(output.results.is_empty());
    }
}
