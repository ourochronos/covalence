//! Update class for source lifecycle management.

use serde::{Deserialize, Serialize};

/// Update class for source lifecycle management.
///
/// Determines how updates to a source propagate through the
/// graph. See `spec/05-ingestion.md` for full semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateClass {
    /// Content is only ever appended (e.g., log files).
    AppendOnly,
    /// Content is versioned; new versions supersede old ones.
    Versioned,
    /// A correction to a previous source.
    Correction,
    /// Structural refactor without semantic change.
    Refactor,
    /// Source has been taken down; all derived edges should be
    /// invalidated.
    Takedown,
}

impl UpdateClass {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AppendOnly => "append_only",
            Self::Versioned => "versioned",
            Self::Correction => "correction",
            Self::Refactor => "refactor",
            Self::Takedown => "takedown",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "append_only" => Some(Self::AppendOnly),
            "versioned" => Some(Self::Versioned),
            "correction" => Some(Self::Correction),
            "refactor" => Some(Self::Refactor),
            "takedown" => Some(Self::Takedown),
            _ => None,
        }
    }

    /// Whether this update class requires cascading edge
    /// invalidation.
    pub fn requires_invalidation(&self) -> bool {
        matches!(self, Self::Correction | Self::Takedown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_class_roundtrip() {
        let classes = [
            UpdateClass::AppendOnly,
            UpdateClass::Versioned,
            UpdateClass::Correction,
            UpdateClass::Refactor,
            UpdateClass::Takedown,
        ];
        for uc in &classes {
            let s = uc.as_str();
            let parsed = UpdateClass::from_str_opt(s);
            assert_eq!(parsed, Some(uc.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn update_class_from_str_unknown() {
        assert!(UpdateClass::from_str_opt("unknown").is_none());
    }

    #[test]
    fn requires_invalidation() {
        assert!(!UpdateClass::AppendOnly.requires_invalidation());
        assert!(!UpdateClass::Versioned.requires_invalidation());
        assert!(UpdateClass::Correction.requires_invalidation());
        assert!(!UpdateClass::Refactor.requires_invalidation());
        assert!(UpdateClass::Takedown.requires_invalidation());
    }
}
