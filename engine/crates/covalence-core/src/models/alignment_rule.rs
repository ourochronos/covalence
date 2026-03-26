//! Alignment rule model — data-driven cross-domain checks.
//!
//! Rules define how to detect misalignment between domain groups
//! (e.g., code ahead of spec, design contradicted by research).
//! Loaded from the `alignment_rules` table and executed by the
//! analysis service.

use serde::{Deserialize, Serialize};

/// A configurable cross-domain alignment check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlignmentRule {
    /// Database primary key.
    pub id: i32,
    /// Unique rule name (e.g., "code_ahead").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Check type: "ahead", "contradiction", or "staleness".
    pub check_type: String,
    /// Source domain group for this check.
    pub source_group: String,
    /// Target domain group for this check.
    pub target_group: String,
    /// Additional parameters (e.g., specific domains within groups).
    pub parameters: serde_json::Value,
}

impl AlignmentRule {
    /// Build from a DB row tuple.
    pub fn from_row(
        row: (
            i32,
            String,
            String,
            String,
            String,
            String,
            serde_json::Value,
        ),
    ) -> Self {
        Self {
            id: row.0,
            name: row.1,
            description: row.2,
            check_type: row.3,
            source_group: row.4,
            target_group: row.5,
            parameters: row.6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_rule_serializes() {
        let rule = AlignmentRule {
            id: 1,
            name: "code_ahead".to_string(),
            description: "Code entities with no matching spec".to_string(),
            check_type: "ahead".to_string(),
            source_group: "implementation".to_string(),
            target_group: "specification".to_string(),
            parameters: serde_json::json!({}),
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("code_ahead"));
        assert!(json.contains("implementation"));
    }

    #[test]
    fn alignment_rule_from_row() {
        let row = (
            2,
            "spec_ahead".to_string(),
            "Spec concepts with no code".to_string(),
            "ahead".to_string(),
            "specification".to_string(),
            "implementation".to_string(),
            serde_json::json!({}),
        );
        let rule = AlignmentRule::from_row(row);
        assert_eq!(rule.id, 2);
        assert_eq!(rule.name, "spec_ahead");
        assert_eq!(rule.check_type, "ahead");
    }
}
