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

    /// Convert from integer, defaulting to `LocalStrict` and logging
    /// a warning if the value is invalid. Use this in `from_row`
    /// functions where an invalid DB value should surface as a
    /// diagnostic rather than silently defaulting.
    pub fn from_i32_or_default(value: i32) -> Self {
        Self::from_i32(value).unwrap_or_else(|| {
            tracing::warn!(
                value,
                "invalid clearance_level in database, defaulting to LocalStrict"
            );
            Self::default()
        })
    }

    /// Get the integer value.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_i32_valid_values() {
        assert_eq!(ClearanceLevel::from_i32(0), Some(ClearanceLevel::LocalStrict));
        assert_eq!(ClearanceLevel::from_i32(1), Some(ClearanceLevel::FederatedTrusted));
        assert_eq!(ClearanceLevel::from_i32(2), Some(ClearanceLevel::FederatedPublic));
    }

    #[test]
    fn from_i32_invalid_values() {
        assert_eq!(ClearanceLevel::from_i32(-1), None);
        assert_eq!(ClearanceLevel::from_i32(3), None);
        assert_eq!(ClearanceLevel::from_i32(99), None);
    }

    #[test]
    fn from_i32_or_default_valid() {
        assert_eq!(
            ClearanceLevel::from_i32_or_default(1),
            ClearanceLevel::FederatedTrusted
        );
    }

    #[test]
    fn from_i32_or_default_invalid_falls_back() {
        // Invalid value should return LocalStrict (default).
        assert_eq!(
            ClearanceLevel::from_i32_or_default(99),
            ClearanceLevel::LocalStrict
        );
    }

    #[test]
    fn roundtrip_as_i32() {
        for val in 0..=2 {
            let level = ClearanceLevel::from_i32(val).unwrap();
            assert_eq!(level.as_i32(), val);
        }
    }

    #[test]
    fn ordering() {
        assert!(ClearanceLevel::LocalStrict < ClearanceLevel::FederatedTrusted);
        assert!(ClearanceLevel::FederatedTrusted < ClearanceLevel::FederatedPublic);
    }

    #[test]
    fn display() {
        assert_eq!(ClearanceLevel::LocalStrict.to_string(), "local_strict");
        assert_eq!(ClearanceLevel::FederatedPublic.to_string(), "federated_public");
    }

    #[test]
    fn default_is_local_strict() {
        assert_eq!(ClearanceLevel::default(), ClearanceLevel::LocalStrict);
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
