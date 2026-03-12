//! Abstention / insufficient context detection.
//!
//! Determines when the system should decline to answer rather
//! than risk hallucination. Score-based gate + coverage check.

use serde::{Deserialize, Serialize};

/// Configuration for abstention detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstentionConfig {
    /// Minimum relevance score for the top result.
    /// Below this threshold, flag as potentially insufficient.
    pub min_relevance_score: f64,
    /// Minimum number of results required.
    pub min_results: usize,
}

impl Default for AbstentionConfig {
    fn default() -> Self {
        Self {
            // CC fusion produces scores in the 0.0-1.0 range (sum
            // of normalized dimension weights). A threshold of 0.05
            // means the top result scored below 5% of the maximum
            // possible fused score, indicating very poor retrieval.
            //
            // For reference, even marginal matches typically score
            // 0.1+ with CC fusion (single dimension, moderate
            // similarity). Previously calibrated for RRF (0.001)
            // which was effectively disabled under CC fusion.
            min_relevance_score: 0.05,
            min_results: 1,
        }
    }
}

/// Result of abstention check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstentionCheck {
    /// Whether the system should abstain from answering.
    pub should_abstain: bool,
    /// Reason for abstention (if applicable).
    pub reason: Option<String>,
    /// The top result score that was evaluated.
    pub top_score: Option<f64>,
    /// Number of results available.
    pub result_count: usize,
}

/// Check whether search results are sufficient to generate an
/// answer.
///
/// Returns an `AbstentionCheck` indicating whether to proceed
/// with generation or abstain.
pub fn check_abstention(result_scores: &[f64], config: &AbstentionConfig) -> AbstentionCheck {
    if result_scores.is_empty() {
        return AbstentionCheck {
            should_abstain: true,
            reason: Some("No search results found".to_string()),
            top_score: None,
            result_count: 0,
        };
    }

    if result_scores.len() < config.min_results {
        return AbstentionCheck {
            should_abstain: true,
            reason: Some(format!(
                "Insufficient results: {} < {}",
                result_scores.len(),
                config.min_results
            )),
            top_score: result_scores.first().copied(),
            result_count: result_scores.len(),
        };
    }

    let top_score = result_scores
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);

    if top_score < config.min_relevance_score {
        return AbstentionCheck {
            should_abstain: true,
            reason: Some(format!(
                "Top result score {:.3} below threshold {:.3}",
                top_score, config.min_relevance_score
            )),
            top_score: Some(top_score),
            result_count: result_scores.len(),
        };
    }

    AbstentionCheck {
        should_abstain: false,
        reason: None,
        top_score: Some(top_score),
        result_count: result_scores.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstention_empty_results() {
        let config = AbstentionConfig::default();
        let check = check_abstention(&[], &config);
        assert!(check.should_abstain);
        assert_eq!(check.result_count, 0);
        assert!(check.top_score.is_none());
        assert!(
            check
                .reason
                .as_deref()
                .unwrap()
                .contains("No search results")
        );
    }

    #[test]
    fn abstention_low_score() {
        let config = AbstentionConfig::default();
        // CC fusion scores below 0.05 indicate very poor retrieval.
        let check = check_abstention(&[0.03, 0.01], &config);
        assert!(check.should_abstain);
        assert_eq!(check.top_score, Some(0.03));
        assert!(check.reason.as_deref().unwrap().contains("below threshold"));
    }

    #[test]
    fn abstention_sufficient() {
        let config = AbstentionConfig::default();
        // CC fusion scores in the 0.1-0.5 range are typical results.
        let check = check_abstention(&[0.35, 0.22], &config);
        assert!(!check.should_abstain);
        assert!(check.reason.is_none());
        assert_eq!(check.top_score, Some(0.35));
        assert_eq!(check.result_count, 2);
    }

    #[test]
    fn abstention_insufficient_count() {
        let config = AbstentionConfig {
            min_relevance_score: 0.3,
            min_results: 3,
        };
        let check = check_abstention(&[0.5], &config);
        assert!(check.should_abstain);
        assert_eq!(check.result_count, 1);
        assert!(
            check
                .reason
                .as_deref()
                .unwrap()
                .contains("Insufficient results")
        );
    }
}
