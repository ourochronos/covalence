//! Extractor layer evaluator.
//!
//! Evaluates entity extraction quality by comparing predicted
//! entities against a gold-standard annotation set, producing
//! precision, recall, and F1 metrics.

use std::collections::HashSet;

use crate::LayerEvaluator;
use crate::metrics::ExtractorMetrics;

/// Configuration for the extractor evaluator.
#[derive(Debug, Clone)]
pub struct ExtractorEval {
    /// Whether entity matching is case-insensitive.
    pub case_insensitive: bool,
}

impl ExtractorEval {
    /// Create a new extractor evaluator.
    pub fn new(case_insensitive: bool) -> Self {
        Self { case_insensitive }
    }
}

impl Default for ExtractorEval {
    fn default() -> Self {
        Self {
            case_insensitive: true,
        }
    }
}

/// An entity for extraction evaluation (name + type pair).
#[derive(Debug, Clone)]
pub struct EvalEntity {
    /// Canonical entity name.
    pub name: String,
    /// Entity type classification.
    pub entity_type: String,
}

/// Input to the extractor evaluator.
#[derive(Debug, Clone)]
pub struct ExtractorInput {
    /// Text to extract entities from.
    pub text: String,
}

/// Output from the extractor evaluator.
#[derive(Debug, Clone)]
pub struct ExtractorOutput {
    /// Extracted entities.
    pub entities: Vec<EvalEntity>,
}

impl LayerEvaluator for ExtractorEval {
    type Input = ExtractorInput;
    type Output = ExtractorOutput;
    type Metrics = ExtractorMetrics;

    fn evaluate(&self, _input: &Self::Input) -> Self::Output {
        // The evaluate step for extraction requires an LLM or
        // extractor implementation. In evaluation mode, callers
        // supply both actual and expected output to `score()`.
        // This returns an empty set as a passthrough.
        ExtractorOutput {
            entities: Vec::new(),
        }
    }

    fn score(&self, output: &Self::Output, expected: &Self::Output) -> Self::Metrics {
        compute_extractor_metrics(&output.entities, &expected.entities, self.case_insensitive)
    }
}

/// Compute precision, recall, and F1 for entity extraction.
///
/// Matching is by (name, entity_type) pair. When
/// `case_insensitive` is true, names are lowercased before
/// comparison.
pub fn compute_extractor_metrics(
    predicted: &[EvalEntity],
    gold: &[EvalEntity],
    case_insensitive: bool,
) -> ExtractorMetrics {
    let normalize = |s: &str| -> String {
        if case_insensitive {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };

    let pred_set: HashSet<(String, String)> = predicted
        .iter()
        .map(|e| (normalize(&e.name), normalize(&e.entity_type)))
        .collect();

    let gold_set: HashSet<(String, String)> = gold
        .iter()
        .map(|e| (normalize(&e.name), normalize(&e.entity_type)))
        .collect();

    let true_positives = pred_set.intersection(&gold_set).count();
    let predicted_count = pred_set.len();
    let gold_count = gold_set.len();

    let precision = if predicted_count > 0 {
        true_positives as f64 / predicted_count as f64
    } else {
        0.0
    };

    let recall = if gold_count > 0 {
        true_positives as f64 / gold_count as f64
    } else {
        0.0
    };

    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    ExtractorMetrics {
        precision,
        recall,
        f1,
        predicted_count,
        gold_count,
        true_positives,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(name: &str, etype: &str) -> EvalEntity {
        EvalEntity {
            name: name.to_string(),
            entity_type: etype.to_string(),
        }
    }

    #[test]
    fn perfect_match() {
        let gold = vec![entity("Alice", "person"), entity("Acme", "organization")];
        let predicted = vec![entity("Alice", "person"), entity("Acme", "organization")];
        let metrics = compute_extractor_metrics(&predicted, &gold, true);
        assert_eq!(metrics.precision, 1.0);
        assert_eq!(metrics.recall, 1.0);
        assert_eq!(metrics.f1, 1.0);
        assert_eq!(metrics.true_positives, 2);
    }

    #[test]
    fn no_overlap() {
        let gold = vec![entity("Alice", "person")];
        let predicted = vec![entity("Bob", "person")];
        let metrics = compute_extractor_metrics(&predicted, &gold, true);
        assert_eq!(metrics.precision, 0.0);
        assert_eq!(metrics.recall, 0.0);
        assert_eq!(metrics.f1, 0.0);
        assert_eq!(metrics.true_positives, 0);
    }

    #[test]
    fn partial_match_precision_recall() {
        let gold = vec![entity("Alice", "person"), entity("Bob", "person")];
        let predicted = vec![entity("Alice", "person"), entity("Charlie", "person")];
        let metrics = compute_extractor_metrics(&predicted, &gold, true);
        assert_eq!(metrics.true_positives, 1);
        assert!((metrics.precision - 0.5).abs() < 1e-10);
        assert!((metrics.recall - 0.5).abs() < 1e-10);
    }

    #[test]
    fn case_insensitive_matching() {
        let gold = vec![entity("alice", "person")];
        let predicted = vec![entity("Alice", "Person")];
        let metrics = compute_extractor_metrics(&predicted, &gold, true);
        assert_eq!(metrics.true_positives, 1);
        assert_eq!(metrics.precision, 1.0);
    }

    #[test]
    fn case_sensitive_matching() {
        let gold = vec![entity("alice", "person")];
        let predicted = vec![entity("Alice", "person")];
        let metrics = compute_extractor_metrics(&predicted, &gold, false);
        assert_eq!(metrics.true_positives, 0);
        assert_eq!(metrics.precision, 0.0);
    }

    #[test]
    fn empty_predictions_zero_precision() {
        let gold = vec![entity("Alice", "person")];
        let predicted: Vec<EvalEntity> = vec![];
        let metrics = compute_extractor_metrics(&predicted, &gold, true);
        assert_eq!(metrics.precision, 0.0);
        assert_eq!(metrics.recall, 0.0);
        assert_eq!(metrics.predicted_count, 0);
    }

    #[test]
    fn empty_gold_zero_recall() {
        let gold: Vec<EvalEntity> = vec![];
        let predicted = vec![entity("Alice", "person")];
        let metrics = compute_extractor_metrics(&predicted, &gold, true);
        assert_eq!(metrics.precision, 0.0);
        assert_eq!(metrics.recall, 0.0);
        assert_eq!(metrics.gold_count, 0);
    }

    #[test]
    fn evaluate_returns_empty_passthrough() {
        let eval = ExtractorEval::default();
        let input = ExtractorInput {
            text: "Alice works at Acme.".to_string(),
        };
        let output = eval.evaluate(&input);
        assert!(output.entities.is_empty());
    }
}
