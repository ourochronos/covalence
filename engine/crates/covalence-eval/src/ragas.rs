//! RAGAS (Retrieval Augmented Generation Assessment) metric traits.
//!
//! Provides trait definitions and stub implementations for the four
//! core RAGAS metrics: faithfulness, answer relevancy, context
//! precision, and context recall.

use serde::{Deserialize, Serialize};

/// Composite RAGAS score containing all four metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagasScore {
    /// Ratio of claims in the answer supported by the context.
    pub faithfulness: f64,
    /// Cosine similarity between the question and the answer.
    pub answer_relevancy: f64,
    /// Fraction of retrieved contexts that are relevant.
    pub context_precision: f64,
    /// Fraction of ground-truth facts covered by the context.
    pub context_recall: f64,
}

/// Measures the ratio of claims in an answer that are supported by
/// the provided context.
pub trait Faithfulness {
    /// Compute faithfulness score (0.0 to 1.0).
    fn score(&self, answer: &str, contexts: &[String]) -> f64;
}

/// Measures cosine similarity between the question embedding and the
/// answer embedding.
pub trait AnswerRelevancy {
    /// Compute answer relevancy score (0.0 to 1.0).
    fn score(&self, question: &str, answer: &str) -> f64;
}

/// Measures the fraction of retrieved contexts that are actually
/// relevant to the question.
pub trait ContextPrecision {
    /// Compute context precision score (0.0 to 1.0).
    fn score(&self, question: &str, contexts: &[String], relevant_contexts: &[String]) -> f64;
}

/// Measures the fraction of ground-truth statements covered by the
/// retrieved contexts.
pub trait ContextRecall {
    /// Compute context recall score (0.0 to 1.0).
    fn score(&self, ground_truth: &[String], contexts: &[String]) -> f64;
}

/// Stub implementation of all RAGAS metrics.
///
/// Returns placeholder scores for testing and development. Replace
/// with LLM-backed implementations for production evaluation.
pub struct StubRagas;

impl Faithfulness for StubRagas {
    fn score(&self, _answer: &str, _contexts: &[String]) -> f64 {
        // Placeholder: assume all claims are supported.
        1.0
    }
}

impl AnswerRelevancy for StubRagas {
    fn score(&self, _question: &str, _answer: &str) -> f64 {
        // Placeholder: assume perfect relevancy.
        1.0
    }
}

impl ContextPrecision for StubRagas {
    fn score(&self, _question: &str, contexts: &[String], relevant_contexts: &[String]) -> f64 {
        if contexts.is_empty() {
            return 0.0;
        }
        // Simple set intersection ratio.
        let relevant_count = contexts
            .iter()
            .filter(|c| relevant_contexts.contains(c))
            .count();
        relevant_count as f64 / contexts.len() as f64
    }
}

impl ContextRecall for StubRagas {
    fn score(&self, ground_truth: &[String], contexts: &[String]) -> f64 {
        if ground_truth.is_empty() {
            return 1.0;
        }
        // Simple containment check.
        let covered = ground_truth
            .iter()
            .filter(|gt| contexts.iter().any(|c| c.contains(gt.as_str())))
            .count();
        covered as f64 / ground_truth.len() as f64
    }
}

/// Compute all RAGAS metrics using stub implementations.
///
/// Returns a composite score for quick evaluation during
/// development.
pub fn evaluate_ragas(
    question: &str,
    answer: &str,
    contexts: &[String],
    relevant_contexts: &[String],
    ground_truth: &[String],
) -> RagasScore {
    let stub = StubRagas;
    RagasScore {
        faithfulness: Faithfulness::score(&stub, answer, contexts),
        answer_relevancy: AnswerRelevancy::score(&stub, question, answer),
        context_precision: ContextPrecision::score(&stub, question, contexts, relevant_contexts),
        context_recall: ContextRecall::score(&stub, ground_truth, contexts),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_faithfulness_returns_one() {
        let stub = StubRagas;
        let score = Faithfulness::score(&stub, "answer", &["context".to_string()]);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stub_answer_relevancy_returns_one() {
        let stub = StubRagas;
        let score = AnswerRelevancy::score(&stub, "question", "answer");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn context_precision_with_all_relevant() {
        let stub = StubRagas;
        let contexts = vec!["a".to_string(), "b".to_string()];
        let relevant = vec!["a".to_string(), "b".to_string()];
        let score = ContextPrecision::score(&stub, "q", &contexts, &relevant);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn context_precision_with_none_relevant() {
        let stub = StubRagas;
        let contexts = vec!["a".to_string(), "b".to_string()];
        let relevant: Vec<String> = vec![];
        let score = ContextPrecision::score(&stub, "q", &contexts, &relevant);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn context_precision_empty_contexts() {
        let stub = StubRagas;
        let contexts: Vec<String> = vec![];
        let relevant: Vec<String> = vec![];
        let score = ContextPrecision::score(&stub, "q", &contexts, &relevant);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn context_recall_with_full_coverage() {
        let stub = StubRagas;
        let ground_truth = vec!["fact1".to_string()];
        let contexts = vec!["This contains fact1 in it.".to_string()];
        let score = ContextRecall::score(&stub, &ground_truth, &contexts);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn context_recall_with_no_coverage() {
        let stub = StubRagas;
        let ground_truth = vec!["fact1".to_string()];
        let contexts = vec!["unrelated text".to_string()];
        let score = ContextRecall::score(&stub, &ground_truth, &contexts);
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn context_recall_empty_ground_truth() {
        let stub = StubRagas;
        let ground_truth: Vec<String> = vec![];
        let contexts = vec!["something".to_string()];
        let score = ContextRecall::score(&stub, &ground_truth, &contexts);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_ragas_returns_composite() {
        let score = evaluate_ragas(
            "What is X?",
            "X is Y.",
            &["context about X and Y".to_string()],
            &["context about X and Y".to_string()],
            &["Y".to_string()],
        );
        assert!(score.faithfulness >= 0.0);
        assert!(score.answer_relevancy >= 0.0);
        assert!(score.context_precision >= 0.0);
        assert!(score.context_recall >= 0.0);
    }
}
