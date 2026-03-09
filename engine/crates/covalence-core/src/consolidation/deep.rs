//! Deep consolidation tier.
//!
//! Structural maintenance: TrustRank recalibration, community detection,
//! domain topology map, landmark articles, BMR forgetting,
//! cross-domain bridge discovery.

use serde::{Deserialize, Serialize};

/// Configuration for deep consolidation runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepConfig {
    /// Damping factor for TrustRank propagation.
    pub trust_rank_damping: f64,
    /// Resolution parameter for community detection.
    pub community_resolution: f64,
    /// Threshold below which BMR forgetting removes opinions.
    pub bmr_threshold: f64,
}

impl Default for DeepConfig {
    fn default() -> Self {
        Self {
            trust_rank_damping: 0.85,
            community_resolution: 1.0,
            bmr_threshold: 0.01,
        }
    }
}

/// Report produced by a deep consolidation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepReport {
    /// Number of communities detected.
    pub communities_found: usize,
    /// Number of nodes removed by BMR forgetting.
    pub nodes_forgotten: usize,
    /// Number of trust scores recalculated.
    pub trust_scores_updated: usize,
}

/// Trait for running deep consolidation on the knowledge graph.
#[async_trait::async_trait]
pub trait DeepConsolidator: Send + Sync {
    /// Execute a deep consolidation pass with the given configuration.
    async fn run_deep(&self, config: &DeepConfig) -> crate::error::Result<DeepReport>;
}
