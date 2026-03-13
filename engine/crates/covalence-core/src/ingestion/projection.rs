//! Offset Projection Engine — reverse-projects byte spans from mutated
//! (post-coreference) text back to canonical (original) source positions.
//!
//! When fastcoref replaces pronouns with their referents, the text
//! changes length. A span like `(100, 115)` in the mutated text may
//! correspond to `(100, 102)` in the canonical text if "he" was
//! expanded to "Albert Einstein" at that position.
//!
//! The ledger records every such mutation with both canonical and
//! mutated byte ranges. This module uses the ledger to translate
//! any mutated-text span back to canonical coordinates.

use crate::models::projection::LedgerEntry;

/// Reverse-project a byte span from mutated text coordinates back
/// to canonical (original) text coordinates.
///
/// The algorithm walks the sorted ledger entries and accumulates
/// the byte offset delta. For each mutation before the span, the
/// span boundaries are shifted backward (or forward) by the delta.
///
/// If the span overlaps a mutation boundary, the canonical span is
/// expanded to cover the full canonical range of the overlapping
/// mutation — this is conservative but avoids partial-token artifacts.
///
/// # Arguments
/// * `mutated_start` — Start byte offset in the mutated text.
/// * `mutated_end` — End byte offset in the mutated text.
/// * `ledger` — Sorted (by `mutated_span_start`) list of mutations.
///
/// # Returns
/// `(canonical_start, canonical_end)` in the original source text.
pub fn reverse_project(
    mutated_start: usize,
    mutated_end: usize,
    ledger: &[LedgerEntry],
) -> (usize, usize) {
    if ledger.is_empty() {
        return (mutated_start, mutated_end);
    }

    // Delta from mutations entirely before the span — affects both
    // start and end equally.
    let mut delta_before: isize = 0;
    // Delta from mutations fully contained within the span — affects
    // only end (the mutation is after start but before end).
    let mut delta_contained: isize = 0;
    let mut canonical_start = mutated_start as isize;
    let mut canonical_end = mutated_end as isize;
    // Track whether boundaries were snapped to canonical coordinates
    // directly. Snapped boundaries are already in canonical space and
    // must NOT be shifted by delta afterwards.
    let mut start_snapped = false;
    let mut end_snapped = false;

    for entry in ledger {
        let m_start = entry.mutated_span_start;
        let m_end = entry.mutated_span_end;
        let delta = entry.delta();

        if m_end <= mutated_start {
            // Mutation entirely before our span — accumulate delta.
            delta_before += delta;
        } else if m_start >= mutated_end {
            // Mutation entirely after our span — stop processing.
            break;
        } else {
            // Mutation overlaps our span.
            if m_start == mutated_start && m_end == mutated_end {
                // Span exactly matches this mutation — return the
                // canonical range directly.
                return (entry.canonical_span_start, entry.canonical_span_end);
            }
            // Expand to cover the full canonical range of the
            // overlapping mutation.
            if m_start < mutated_start {
                // Mutation starts before our span — snap start to
                // the canonical start of this mutation (already in
                // canonical space, no delta adjustment needed).
                canonical_start = entry.canonical_span_start as isize;
                start_snapped = true;
            }
            if m_end > mutated_end {
                // Mutation ends after our span — snap end to the
                // canonical end of this mutation (already in
                // canonical space, no delta adjustment needed).
                canonical_end = entry.canonical_span_end as isize;
                end_snapped = true;
                break;
            }
            // Mutation fully contained within our span — its delta
            // only affects the end boundary (the start is before
            // this mutation).
            delta_contained += delta;
        }
    }

    // Apply accumulated deltas only to boundaries still in mutated
    // coordinates. Snapped boundaries are already canonical.
    if !start_snapped {
        canonical_start -= delta_before;
    }
    if !end_snapped {
        canonical_end -= delta_before + delta_contained;
    }

    // Clamp to non-negative.
    let cs = canonical_start.max(0) as usize;
    let ce = canonical_end.max(cs as isize) as usize;

    (cs, ce)
}

/// Sort ledger entries by mutated span start position.
///
/// The projection algorithm requires entries to be sorted by their
/// position in the mutated text. This helper ensures that invariant.
pub fn sort_ledger(ledger: &mut [LedgerEntry]) {
    ledger.sort_by_key(|e| e.mutated_span_start);
}

/// Reverse-project a set of byte spans, reusing one sorted ledger.
///
/// More efficient than calling `reverse_project` repeatedly because
/// the ledger is sorted once.
pub fn reverse_project_batch(
    spans: &[(usize, usize)],
    ledger: &[LedgerEntry],
) -> Vec<(usize, usize)> {
    // Sort a local copy.
    let mut sorted = ledger.to_vec();
    sort_ledger(&mut sorted);
    spans
        .iter()
        .map(|&(start, end)| reverse_project(start, end, &sorted))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ids::SourceId;

    fn make_entry(
        canonical: (usize, usize),
        canonical_token: &str,
        mutated: (usize, usize),
        mutated_token: &str,
    ) -> LedgerEntry {
        LedgerEntry::new(
            SourceId::new(),
            canonical,
            canonical_token.to_string(),
            mutated,
            mutated_token.to_string(),
        )
    }

    #[test]
    fn empty_ledger_returns_identity() {
        assert_eq!(reverse_project(10, 20, &[]), (10, 20));
    }

    #[test]
    fn mutation_before_span_shifts_backward() {
        // "he" (2 bytes at 5..7) → "Einstein" (8 bytes at 5..13)
        // delta = +6
        // Span at mutated (20, 30) should map to canonical (14, 24)
        let ledger = vec![make_entry((5, 7), "he", (5, 13), "Einstein")];
        assert_eq!(reverse_project(20, 30, &ledger), (14, 24));
    }

    #[test]
    fn mutation_after_span_has_no_effect() {
        let ledger = vec![make_entry((50, 52), "he", (50, 58), "Einstein")];
        assert_eq!(reverse_project(10, 20, &ledger), (10, 20));
    }

    #[test]
    fn multiple_mutations_before_span() {
        // Two expansions: each adds 6 bytes → total delta = +12
        let ledger = vec![
            make_entry((5, 7), "he", (5, 13), "Einstein"),    // +6
            make_entry((20, 22), "it", (26, 34), "the atom"), // +6
        ];
        // Span at mutated (40, 50) → canonical (28, 38)
        assert_eq!(reverse_project(40, 50, &ledger), (28, 38));
    }

    #[test]
    fn span_within_mutation_expands_to_canonical() {
        // "he" at canonical (10, 12) → "Albert Einstein" at mutated (10, 25)
        // If our span is (15, 20) — that's inside the mutation.
        // The overlapping mutation's canonical range is (10, 12).
        // Since mutation starts before span: canonical_start snaps to 10.
        // Mutation ends after span: canonical_end snaps to 12.
        let ledger = vec![make_entry((10, 12), "he", (10, 25), "Albert Einstein")];
        assert_eq!(reverse_project(15, 20, &ledger), (10, 12));
    }

    #[test]
    fn span_exactly_covers_mutation() {
        // Span exactly matches the mutation range.
        // "he" (2 bytes at 10..12) → "Albert Einstein" (15 bytes at 10..25)
        // When span = mutation, we should return the canonical range directly.
        let ledger = vec![make_entry((10, 12), "he", (10, 25), "Albert Einstein")];
        assert_eq!(reverse_project(10, 25, &ledger), (10, 12));
    }

    #[test]
    fn contraction_shifts_forward() {
        // "Albert Einstein" (15 bytes at 10..25) → "he" (2 bytes at 10..12)
        // delta = -13
        // Span at mutated (20, 30) → canonical (33, 43)
        let ledger = vec![make_entry((10, 25), "Albert Einstein", (10, 12), "he")];
        assert_eq!(reverse_project(20, 30, &ledger), (33, 43));
    }

    #[test]
    fn batch_projection() {
        let ledger = vec![make_entry((5, 7), "he", (5, 13), "Einstein")]; // +6
        let spans = vec![(20, 30), (40, 50), (0, 3)];
        let results = reverse_project_batch(&spans, &ledger);
        assert_eq!(results[0], (14, 24)); // After mutation
        assert_eq!(results[1], (34, 44)); // After mutation
        assert_eq!(results[2], (0, 3)); // Before mutation
    }

    #[test]
    fn snapped_start_not_shifted_by_prior_delta() {
        // Mutation 1: "he" → "Einstein" at mutated (5,13), delta=+6
        // Mutation 2: "he" → "Albert Einstein" at mutated (20,35), canonical (14,16)
        // Span (25,40): entry 1 entirely before (cumul delta=+6), entry 2
        // overlaps left. Start snaps to 14 (canonical). Entry 2 delta (+13)
        // also accumulated (it ends before span end). canonical_end = 40-19=21.
        // Bug before fix: prior delta was applied to snapped start → (8, 21).
        let ledger = vec![
            make_entry((5, 7), "he", (5, 13), "Einstein"),
            make_entry((14, 16), "he", (20, 35), "Albert Einstein"),
        ];
        assert_eq!(reverse_project(25, 40, &ledger), (14, 21));
    }

    #[test]
    fn snapped_end_not_shifted_by_prior_delta() {
        // Mutation 1: "he" → "Einstein" at mutated (5,13), delta=+6
        // Mutation 2: "he" → "Albert Einstein" at mutated (20,35), canonical (14,16)
        // Span (10,30): entry 1 overlaps left (mutated 5..13 covers byte 10),
        // so start snaps to 5 (canonical). Entry 2 overlaps right (mutated 35 > 30),
        // so end snaps to 16 (canonical). Both snapped → no delta adjustment.
        let ledger = vec![
            make_entry((5, 7), "he", (5, 13), "Einstein"),
            make_entry((14, 16), "he", (20, 35), "Albert Einstein"),
        ];
        assert_eq!(reverse_project(10, 30, &ledger), (5, 16));
    }

    #[test]
    fn both_snapped_ignores_prior_delta() {
        // Mutation 1: "he" → "Einstein" at mutated (5,13), delta=+6
        // Mutation 2: "he" → "Albert Einstein" at mutated (20,35), canonical (14,16)
        // Span (25,30) is entirely inside mutation 2.
        // Both boundaries should snap to mutation 2's canonical range.
        let ledger = vec![
            make_entry((5, 7), "he", (5, 13), "Einstein"),
            make_entry((14, 16), "he", (20, 35), "Albert Einstein"),
        ];
        assert_eq!(reverse_project(25, 30, &ledger), (14, 16));
    }

    #[test]
    fn contained_mutation_only_shifts_end() {
        // Span (10, 50) contains a mutation at mutated (20, 35) that
        // maps to canonical (20, 25). delta = +10.
        // The mutation is AFTER the start, so start should NOT be
        // shifted. Only end should absorb the delta.
        // canonical_start = 10 (no delta_before, no snap)
        // canonical_end   = 50 - 10 = 40 (delta_contained = 10)
        let ledger = vec![make_entry((20, 25), "he", (20, 35), "Albert Einstein")];
        assert_eq!(reverse_project(10, 50, &ledger), (10, 40));
    }

    #[test]
    fn contained_mutation_with_prior_delta() {
        // Mutation 1 before span: "he"→"Einstein" at mutated (2,10), +6
        // Mutation 2 inside span: "he"→"Albert Einstein" at mutated (25,40), canonical (19,21), +13
        // Span (15, 55):
        //   delta_before = 6, delta_contained = 13
        //   canonical_start = 15 - 6 = 9
        //   canonical_end   = 55 - 6 - 13 = 36
        let ledger = vec![
            make_entry((2, 4), "he", (2, 10), "Einstein"),
            make_entry((19, 21), "he", (25, 40), "Albert Einstein"),
        ];
        assert_eq!(reverse_project(15, 55, &ledger), (9, 36));
    }

    #[test]
    fn zero_length_span() {
        let ledger = vec![make_entry((5, 7), "he", (5, 13), "Einstein")];
        let (start, end) = reverse_project(20, 20, &ledger);
        assert_eq!(start, end); // Zero-length remains zero-length
    }
}
