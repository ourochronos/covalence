//! Clearance levels for federation compartmentalization.

use serde::{Deserialize, Serialize};

/// Classification level controlling data visibility and federation sharing.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(i32)]
pub enum ClearanceLevel {
    /// Never leaves the local node. Default for all new data.
    #[default]
    LocalStrict = 0,

    /// Shared only with explicitly whitelisted peer nodes.
    FederatedTrusted = 1,

    /// Fully sharable in zero-trust broadcast.
    FederatedPublic = 2,
}

impl ClearanceLevel {
    /// Convert from integer value.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::LocalStrict),
            1 => Some(Self::FederatedTrusted),
            2 => Some(Self::FederatedPublic),
            _ => None,
        }
    }

    /// Get the integer value.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl std::fmt::Display for ClearanceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalStrict => write!(f, "local_strict"),
            Self::FederatedTrusted => write!(f, "federated_trusted"),
            Self::FederatedPublic => write!(f, "federated_public"),
        }
    }
}
