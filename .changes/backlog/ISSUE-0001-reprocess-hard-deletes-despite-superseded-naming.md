# ISSUE-0001: `reprocess` hard-deletes despite "superseded" naming

**Status:** open
**Filed:** 2026-05-05
**Filed during:** change 0002-restore-code-extraction-data (discovery phase)
**Kind:** tech-debt

## What

`SourceService::reprocess` at `engine/crates/covalence-core/src/services/source/reprocess.rs:30-110` hard-deletes prior data when a source is reprocessed:

- Line 54: `ExtractionRepo::delete_by_source(&*self.repo, id).await?`
- Line 55: `NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?`
- Line 56: `ChunkRepo::delete_by_source(&*self.repo, id).await?`
- Line 57: `LedgerRepo::delete_by_source(&*self.repo, id).await?`

Yet the result type's field at line 16 is named `extractions_superseded: u64`, implying soft supersession.

## Why it matters

This contradicts CLAUDE.md's **Epistemic data lifecycle**:
> "Never automatically delete old source versions, orphan nodes, or duplicates. Old observations aren't false — they're prior state. Use `/admin/data-health` to preview what's stale, then make conscious cleanup decisions."

Two consequences:
1. **Naming drift:** the API's terminology says one thing; the implementation does another. Anyone reading `ReprocessResult.extractions_superseded` reasonably expects soft supersession; they get hard deletion.
2. **Audit trail loss:** when reprocess fixes a pipeline regression (as in change 0002 for issue #176), the prior LLM-noise extractions disappear. This may be the right call case-by-case, but it should be a deliberate decision, not the default.

## Context

Surfaced during change [`0002-restore-code-extraction-data`](../active/0002-restore-code-extraction-data/spec.md) discovery. Quote from that spec:

> The framework's discovery loop elevated this from "rerun reprocess on 178 sources" to a real architectural question: should reprocess delete or supersede, and what does that mean for chunks/extractions/aliases/ledger?

Change 0002 chose Option D (apply A now, file this backlog issue) — the immediate fix uses hard-deletion as a case-specific exception (the LLM-noise nodes for 178 broken code sources are not valid prior state worth preserving). This issue tracks the general fix.

## Proposed action

Treat as candidate future architectural change. Likely scope corresponds to **Option B** from change 0002's spec:

1. Add a `superseded_by` (UUID FK) and `superseded_at` (timestamptz) column to `chunks` and `extractions` (sqlx migration).
2. Replace the four `delete_by_source` calls in `reprocess.rs:54-57` with `mark_superseded` calls.
3. Audit every read site of `chunks` and `extractions` (search dimensions, indexing, ingestion, services) and add `WHERE superseded_at IS NULL` to the default read paths.
4. Verify INV-2 (provenance) still holds: a superseded chunk's provenance link is still valid for historical lookups.
5. Optional: introduce a `cove admin reprocess --supersede` flag (default) vs `--hard-delete` (explicit opt-in for noise cleanup like change 0002's case).
6. Optional: add a `cleanup_superseded(older_than: Duration)` admin operation for cases where prior state is genuinely noise and should eventually be GC'd.

Estimated multi-session work (schema change + query audit + migration + tests).

This corresponds to claude-ultra change `0003-supersede-on-reprocess` (or whatever the next change number turns out to be) when prioritized. Likely warrants ADR-0025 ("Soft supersession for chunks and extractions on reprocess").

## Resolution (filled when closed)

_Pending — file when the architectural change lands._
