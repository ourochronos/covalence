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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_opt_valid() {
        assert_eq!(CausalLevel::from_str_opt("association"), Some(CausalLevel::Association));
        assert_eq!(CausalLevel::from_str_opt("intervention"), Some(CausalLevel::Intervention));
        assert_eq!(
            CausalLevel::from_str_opt("counterfactual"),
            Some(CausalLevel::Counterfactual)
        );
    }

    #[test]
    fn from_str_opt_invalid() {
        assert_eq!(CausalLevel::from_str_opt("unknown"), None);
        assert_eq!(CausalLevel::from_str_opt(""), None);
        assert_eq!(CausalLevel::from_str_opt("Association"), None); // case-sensitive
    }

    #[test]
    fn roundtrip() {
        for level in [
            CausalLevel::Association,
            CausalLevel::Intervention,
            CausalLevel::Counterfactual,
        ] {
            assert_eq!(CausalLevel::from_str_opt(level.as_str()), Some(level));
        }
    }
}
