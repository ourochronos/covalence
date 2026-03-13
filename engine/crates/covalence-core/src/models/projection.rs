//! Offset Projection Ledger model — tracks character-level mutations
//! from coreference resolution so byte spans can be reverse-projected
//! from mutated text back to canonical source positions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::ids::SourceId;

/// A single ledger entry recording one text mutation from coreference
/// resolution (e.g., replacing "he" with "Albert Einstein").
///
/// The canonical span is the position in the original source text.
/// The mutated span is the position in the post-coref text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Unique identifier.
    pub id: Uuid,
    /// Source this mutation belongs to.
    pub source_id: SourceId,
    /// Start byte offset in canonical (original) text.
    pub canonical_span_start: usize,
    /// End byte offset in canonical (original) text.
    pub canonical_span_end: usize,
    /// The original token that was replaced.
    pub canonical_token: String,
    /// Start byte offset in mutated (post-coref) text.
    pub mutated_span_start: usize,
    /// End byte offset in mutated (post-coref) text.
    pub mutated_span_end: usize,
    /// The replacement token (resolved coreference).
    pub mutated_token: String,
    /// When this entry was created.
    pub created_at: DateTime<Utc>,
}

impl LedgerEntry {
    /// Create a new ledger entry.
    pub fn new(
        source_id: SourceId,
        canonical_span: (usize, usize),
        canonical_token: String,
        mutated_span: (usize, usize),
        mutated_token: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            source_id,
            canonical_span_start: canonical_span.0,
            canonical_span_end: canonical_span.1,
            canonical_token,
            mutated_span_start: mutated_span.0,
            mutated_span_end: mutated_span.1,
            mutated_token,
            created_at: Utc::now(),
        }
    }

    /// The byte-length change caused by this mutation.
    /// Positive = text grew, negative = text shrank.
    pub fn delta(&self) -> isize {
        let canonical_len = self.canonical_span_end as isize - self.canonical_span_start as isize;
        let mutated_len = self.mutated_span_end as isize - self.mutated_span_start as isize;
        mutated_len - canonical_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_positive_when_replacement_longer() {
        // "he" (2 bytes) → "Albert Einstein" (15 bytes) = +13
        let entry = LedgerEntry::new(
            SourceId::new(),
            (10, 12),
            "he".to_string(),
            (10, 25),
            "Albert Einstein".to_string(),
        );
        assert_eq!(entry.delta(), 13);
    }

    #[test]
    fn delta_negative_when_replacement_shorter() {
        // "Albert Einstein" (15 bytes) → "he" (2 bytes) = -13
        let entry = LedgerEntry::new(
            SourceId::new(),
            (10, 25),
            "Albert Einstein".to_string(),
            (10, 12),
            "he".to_string(),
        );
        assert_eq!(entry.delta(), -13);
    }

    #[test]
    fn delta_zero_when_same_length() {
        let entry = LedgerEntry::new(
            SourceId::new(),
            (0, 3),
            "foo".to_string(),
            (0, 3),
            "bar".to_string(),
        );
        assert_eq!(entry.delta(), 0);
    }

    #[test]
    fn serde_roundtrip() {
        let entry = LedgerEntry::new(
            SourceId::new(),
            (10, 12),
            "he".to_string(),
            (10, 25),
            "Albert Einstein".to_string(),
        );
        let json = serde_json::to_string(&entry).unwrap();
        let restored: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.canonical_token, "he");
        assert_eq!(restored.mutated_token, "Albert Einstein");
    }
}
