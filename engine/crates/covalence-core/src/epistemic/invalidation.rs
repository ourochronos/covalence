//! Bi-temporal edge invalidation.
//!
//! When contradicting information is detected, old edges are
//! invalidated rather than deleted. This preserves temporal
//! history and enables "what was true at time T?" queries.
//!
//! Based on Graphiti/Zep's bi-temporal model (Rasmussen et al.,
//! Jan 2025).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ids::EdgeId;

/// An existing edge's temporal metadata for conflict detection.
///
/// Fields: `(edge_id, target_node_id, valid_from, valid_until,
/// invalid_at)`.
pub type ExistingEdgeRecord = (
    EdgeId,
    EdgeId,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);

/// A pending invalidation action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidationAction {
    /// The edge being invalidated (old fact).
    pub target_edge_id: EdgeId,
    /// The edge that supersedes it (new fact).
    pub superseding_edge_id: EdgeId,
    /// When the invalidation occurred.
    pub invalidated_at: DateTime<Utc>,
    /// Reason for invalidation.
    pub reason: InvalidationReason,
}

/// Why an edge was invalidated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InvalidationReason {
    /// Direct contradiction detected.
    Contradiction {
        /// Description of the contradiction.
        description: String,
    },
    /// Superseded by newer information.
    Supersession {
        /// The newer source that provided updated info.
        source_description: String,
    },
    /// Corrected by an authoritative source.
    Correction {
        /// The correcting source.
        source_description: String,
    },
    /// Content takedown (source removed).
    Takedown,
}

/// Result of checking for temporal conflicts between edges.
#[derive(Debug, Clone)]
pub struct ConflictCheck {
    /// Whether a conflict was detected.
    pub has_conflict: bool,
    /// The conflicting edges (if any).
    pub conflicts: Vec<EdgeConflict>,
}

/// A detected conflict between two edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeConflict {
    /// The existing edge.
    pub existing_edge_id: EdgeId,
    /// The new edge that conflicts.
    pub new_edge_id: EdgeId,
    /// Type of conflict.
    pub conflict_type: ConflictType,
}

/// Type of temporal conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictType {
    /// Same subject-predicate-object with different values.
    DirectContradiction,
    /// Same subject-predicate with overlapping temporal ranges.
    TemporalOverlap,
    /// Newer source provides updated information.
    Update,
}

/// Check if a new edge conflicts with existing edges sharing the
/// same subject and predicate.
///
/// Two edges conflict when they share the same source node and
/// relationship type but point to different target nodes, and
/// their temporal ranges overlap.
///
/// # Arguments
///
/// * `new_edge_id` - ID of the new edge being inserted
/// * `new_target_node` - Target node of the new edge
/// * `_new_rel_type` - Relationship type (reserved for future use)
/// * `new_valid_from` - When the new edge's fact becomes valid
/// * `existing_edges` - Existing edges as tuples of
///   `(edge_id, target_node_id, valid_from, valid_until,
///   invalid_at)`
pub fn detect_conflicts(
    new_edge_id: EdgeId,
    new_target_node: EdgeId,
    _new_rel_type: &str,
    new_valid_from: Option<DateTime<Utc>>,
    existing_edges: &[ExistingEdgeRecord],
) -> ConflictCheck {
    let mut conflicts = Vec::new();

    for &(ref edge_id, ref target_node, valid_from, valid_until, invalid_at) in existing_edges {
        // Skip already-invalidated edges.
        if invalid_at.is_some() {
            continue;
        }

        // Different target = potential contradiction.
        if *target_node != new_target_node {
            // Check temporal overlap.
            let overlaps = temporal_overlap(
                new_valid_from,
                None, // new edge has no end yet
                valid_from,
                valid_until,
            );

            if overlaps {
                conflicts.push(EdgeConflict {
                    existing_edge_id: *edge_id,
                    new_edge_id,
                    conflict_type: ConflictType::DirectContradiction,
                });
            }
        }
    }

    ConflictCheck {
        has_conflict: !conflicts.is_empty(),
        conflicts,
    }
}

/// Check if two temporal ranges overlap.
///
/// Ranges are `[from, until)` where `None` means unbounded.
/// An unbounded start is treated as negative infinity.
/// An unbounded end is treated as positive infinity.
/// Two ranges `[a_start, a_end)` and `[b_start, b_end)` overlap
/// iff `a_start < b_end AND b_start < a_end`.
pub fn temporal_overlap(
    from_a: Option<DateTime<Utc>>,
    until_a: Option<DateTime<Utc>>,
    from_b: Option<DateTime<Utc>>,
    until_b: Option<DateTime<Utc>>,
) -> bool {
    let a_before_b_ends = match until_b {
        None => true,
        Some(ub) => from_a.is_none_or(|fa| fa < ub),
    };

    let b_before_a_ends = match until_a {
        None => true,
        Some(ua) => from_b.is_none_or(|fb| fb < ua),
    };

    a_before_b_ends && b_before_a_ends
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .unwrap_or_else(|| panic!("invalid date"))
    }

    #[test]
    fn temporal_overlap_both_unbounded() {
        assert!(temporal_overlap(None, None, None, None));
    }

    #[test]
    fn temporal_overlap_no_overlap() {
        // A: [Jan, Mar), B: [Apr, Jun) — no overlap
        let from_a = Some(dt(2025, 1, 1));
        let until_a = Some(dt(2025, 3, 1));
        let from_b = Some(dt(2025, 4, 1));
        let until_b = Some(dt(2025, 6, 1));
        assert!(!temporal_overlap(from_a, until_a, from_b, until_b));
    }

    #[test]
    fn temporal_overlap_partial() {
        // A: [Jan, Apr), B: [Mar, Jun) — overlap in [Mar, Apr)
        let from_a = Some(dt(2025, 1, 1));
        let until_a = Some(dt(2025, 4, 1));
        let from_b = Some(dt(2025, 3, 1));
        let until_b = Some(dt(2025, 6, 1));
        assert!(temporal_overlap(from_a, until_a, from_b, until_b));
    }

    #[test]
    fn temporal_overlap_one_unbounded() {
        // A: [Jan, None), B: [Mar, Jun) — overlap
        let from_a = Some(dt(2025, 1, 1));
        let from_b = Some(dt(2025, 3, 1));
        let until_b = Some(dt(2025, 6, 1));
        assert!(temporal_overlap(from_a, None, from_b, until_b));
    }

    #[test]
    fn detect_conflicts_no_conflict() {
        // Same target node → no contradiction
        let new_edge = EdgeId::new();
        let target = EdgeId::new();
        let existing = vec![(EdgeId::new(), target, Some(dt(2025, 1, 1)), None, None)];

        let result = detect_conflicts(
            new_edge,
            target,
            "related_to",
            Some(dt(2025, 2, 1)),
            &existing,
        );
        assert!(!result.has_conflict);
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn detect_conflicts_contradiction() {
        // Different target, overlapping time → conflict
        let new_edge = EdgeId::new();
        let new_target = EdgeId::new();
        let existing_target = EdgeId::new();
        let existing_edge = EdgeId::new();

        let existing = vec![(
            existing_edge,
            existing_target,
            Some(dt(2025, 1, 1)),
            None, // unbounded end
            None, // not invalidated
        )];

        let result = detect_conflicts(
            new_edge,
            new_target,
            "related_to",
            Some(dt(2025, 2, 1)),
            &existing,
        );
        assert!(result.has_conflict);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].existing_edge_id, existing_edge,);
    }

    #[test]
    fn detect_conflicts_skip_invalidated() {
        // Existing edge already invalidated → no conflict
        let new_edge = EdgeId::new();
        let new_target = EdgeId::new();
        let existing_target = EdgeId::new();

        let existing = vec![(
            EdgeId::new(),
            existing_target,
            Some(dt(2025, 1, 1)),
            None,
            Some(dt(2025, 3, 1)), // already invalidated
        )];

        let result = detect_conflicts(
            new_edge,
            new_target,
            "related_to",
            Some(dt(2025, 2, 1)),
            &existing,
        );
        assert!(!result.has_conflict);
        assert!(result.conflicts.is_empty());
    }
}
