//! Pearl's Causal Hierarchy levels for edge classification.

use serde::{Deserialize, Serialize};

/// Causal level per Pearl's hierarchy.
///
/// Determines the strength and type of relationship an edge represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalLevel {
    /// L0: Correlational. "X is associated with Y."
    Association,

    /// L1: Causal/evidential. "Doing X causes Y."
    Intervention,

    /// L2: Hypothetical. "Had X not happened, Y would not have."
    Counterfactual,
}

impl CausalLevel {
    /// Parse from string (as stored in PG).
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "association" => Some(Self::Association),
            "intervention" => Some(Self::Intervention),
            "counterfactual" => Some(Self::Counterfactual),
            _ => None,
        }
    }

    /// Convert to string for PG storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Association => "association",
            Self::Intervention => "intervention",
            Self::Counterfactual => "counterfactual",
        }
    }
}
