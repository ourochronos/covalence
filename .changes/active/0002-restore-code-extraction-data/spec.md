# Change 0002 — Restore code-extraction data for 178 stale sources

**Status:** proposal (phase one — alignment loop)
**Kind:** data + architectural (see Open Questions on classification)
**Created:** 2026-05-05
**Branch (at freeze):** `change/0002-restore-code-extraction-data`
**Tracks:** GitHub issue [#176](https://github.com/ourochronos/covalence/issues/176)
**Depends on:** change [`0001-bootstrap`](../0001-bootstrap/spec.md) (manifest apparatus)

## Diagnosis

The pipeline is correct as of 2026-04-08. The 178 broken code sources are **data-debt**, not code-debt.

**Evidence chain (verified end-to-end during discovery):**

1. **Symptom (issue #176):** 178/193 unsummarized code sources have zero `code`-class nodes despite having chunks and extractions.
2. **Routing audit:** `services/queue/handlers.rs:258-346` correctly dispatches code chunks to `AstExtractor` based on `source_is_code` (URI extension OR `source_type == "code"`). Routing is currently sound.
3. **Resolver audit:** `services/pipeline/entity_resolution.rs:228-235` correctly calls `derive_entity_class_with_context(entity_type, source_domain)`. The demotion rule at `models/node.rs:117-121` (Code → Domain when `source_domain ≠ "code"`) is intact and correct.
4. **Direct DB inspection** of one broken source (`engine/crates/covalence-core/src/services/hooks.rs`):
   - `source_type = 'code'` ✓
   - `domains = '{code}'` ✓ (text array, contains `"code"`)
   - 0 `code`-class nodes; 145 `domain/concept`, 79 `domain/technology`, 41 `domain/framework`, etc.
   - These `node_type` values (concept, technology, framework, event, algorithm) are LLM abstractions — **the AST extractor never ran** on these chunks. They're LLM-extractor noise.
5. **Timeline:** Chunks for the broken source were created **2026-03-26 12:11**. The fix that wired AST dispatch into the async queue ([#186](https://github.com/ourochronos/covalence/pull/187), commit `21a68f7`) merged **2026-04-08 10:59** — 13 days *after* these chunks were created. Companion fixes #188 (commit `92cb8b3`) and #190 (commit `8ad795f`) merged the same day.

The broken sources were ingested by the LLM-only pipeline (correct routing did not yet exist for the async queue); their chunks have been LLM-extracted ever since. Fresh ingestions since 2026-04-08 work correctly, which is why 15 of 193 code sources do have `code`-class nodes.

## Surfaced architectural concern (discovered during diagnosis)

`SourceService::reprocess` at `services/source/reprocess.rs:30-110` hard-deletes prior data:

- Line 54: `ExtractionRepo::delete_by_source(&*self.repo, id).await?`
- Line 55: `NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?`
- Line 56: `ChunkRepo::delete_by_source(&*self.repo, id).await?`
- Line 57: `LedgerRepo::delete_by_source(&*self.repo, id).await?`

Yet the `ReprocessResult` struct exposes a field named `extractions_superseded: u64` (line 16), implying soft supersession.

This contradicts `CLAUDE.md`'s **Epistemic data lifecycle**:
> "Never automatically delete old source versions, orphan nodes, or duplicates. Old observations aren't false — they're prior state. Use `/admin/data-health` to preview what's stale, then make conscious cleanup decisions."

The framework's discovery loop elevated this from "rerun reprocess on 178 sources" to a real architectural question: *should reprocess delete or supersede, and what does that mean for chunks/extractions/aliases/ledger?*

This concern is what makes the change architectural rather than purely operational — see Open Question 1 below.

## Affected scope

Modules affected (subset of the bootstrap's 12-module catalog):

- **engine_workers** — `reprocess_source` queue handler at `services/queue/handlers.rs:58-65`. Worker behavior may change depending on chosen option.
- **engine_core** — `services/source/reprocess.rs` (the function), `models/retry_job.rs` (job kind/payload schema), possibly `models/extraction.rs`/`models/chunk.rs` if a `superseded_by` field is added.
- **engine_api** — admin endpoint that triggers reprocess (if a batch endpoint is added).
- **clients** — `cove admin` may grow a `reprocess-stale` subcommand depending on chosen option.
- **documentation** — `spec/05-ingestion.md` may need an "epistemic data lifecycle on reprocess" section; this change's `decisions.md`; possibly an ADR.

Modules unaffected (manifest cells will be `n/a`): engine_search, engine_ingestion (the *pipeline* is correct; reprocess just re-runs it), engine_graph_consolidation, engine_epistemic, dashboard, extensions, infra.

## Affected concerns (subset)

The cells most likely to be touched (rough cell shape — full enumeration at freeze):

- `engine_core.data_model` — adding `superseded_by`/`superseded_at` to chunks/extractions if Option B is chosen
- `engine_core.persistence` — sqlx migrations under Option B; idempotency of reprocess-batch operations
- `engine_core.tests` — regression test that reprocess preserves prior data when superseding (or hard-deletes, depending on chosen semantics)
- `engine_workers.observability` — counter for reprocessed sources, broken-down by reason; `unsummarized_sources` metric should drop after batch reprocess
- `engine_workers.error_handling` — what happens if 1 of 178 reprocess jobs fails? The batch shouldn't roll back the rest, but the failure should be visible.
- `documentation.documentation` — record the diagnosis, the architectural decision, and the operational runbook
- `clients.cli_ux` — if a CLI subcommand is added, its UX needs design (interactive confirm? `--dry-run`?)
- `engine_core.provenance` (INV-2) — provenance must be preserved across reprocess in any option; this is the invariant most at risk and must be explicitly checked.

## Options

### Option A — Operational reprocess (minimum viable)

Enqueue 178 `reprocess_source` jobs against the existing handler. Old chunks/extractions are hard-deleted; new ones are created from the now-correct pipeline.

- **Pros:** Mechanism already exists. ~5 lines of code (a SQL-driven enqueue script, or one admin call). Fastest path to fixing the immediate symptom.
- **Cons:** Inconsistent with epistemic data lifecycle. Sets a precedent that pipeline-fix data-debt = silent destruction. The deeper concern (`reprocess` mis-naming + hard-deletion) goes unaddressed.
- **Scope per manifest:** ~5 cells touched, mostly in `engine_workers` + `documentation`.
- **Time:** ~30 minutes of human + agent work.

### Option B — Fix `reprocess` to supersede properly, then enqueue the 178

Refactor `services/source/reprocess.rs` to:
1. Add a `superseded_by` (UUID) and `superseded_at` (timestamptz) column to `chunks` and `extractions` (sqlx migration).
2. Replace the four `delete_by_source` calls with `mark_superseded` calls.
3. Update queries (search, indexing, etc.) to filter `WHERE superseded_at IS NULL` by default.
4. Ensure provenance traversal still works (INV-2).
5. Re-run the 178 sources through the now-correct + non-destructive reprocess.

- **Pros:** Fixes the underlying concern. Aligns with epistemic data lifecycle. Future pipeline-fix data-debt cleanups will be safe by default.
- **Cons:** Schema change. Touches many query call sites that read chunks/extractions (every search dimension touches these). Risk of missing a call site → silent inclusion of stale data in search results.
- **Scope per manifest:** ~20-30 cells touched across engine_core, engine_workers, engine_search, engine_ingestion, documentation.
- **Time:** Multi-session — at minimum schema change + audit of every chunk/extraction reader, plus regression tests.

### Option C — One-off SQL backfill, leave `reprocess` as-is

Identify the 178 sources, then via a one-time SQL/admin script: delete only their LLM-noise extractions/nodes (where `entity_class = 'domain'` and `source.source_type = 'code'`), then re-run `process_source` on them as if they were fresh. Skip touching `reprocess` altogether.

- **Pros:** Surgical. Doesn't propagate the architectural concern.
- **Cons:** Doesn't fix `reprocess` for next time. Sets up the same trap. Bespoke SQL becomes precedent for "data-debt cleanups happen via ad-hoc scripts."
- **Scope per manifest:** ~3-5 cells.
- **Time:** ~1 hour.

### Option D — A now + B as backlog (split the work)

Apply Option A to fix #176 immediately. File a backlog issue (`ISSUE-0001` in Covalence's backlog) titled "reprocess hard-deletes despite 'superseded' naming — align with epistemic data lifecycle." Treat it as a future architectural change.

- **Pros:** Unblocks the meta loop now. Preserves the architectural concern as a tracked observation.
- **Cons:** "We'll fix it later" risk — the bug stays in `reprocess` and the next pipeline-fix cleanup will hit it again.
- **Scope per manifest:** Same as Option A, plus one backlog file.

## Open questions for alignment

1. **Classification: architectural or local?** Option A is borderline-local (single-module operational fix). Options B and C are architectural. The framework's *discovery* itself was architecture-quality work — it surfaced a real concern that ad-hoc fixing wouldn't have. Even if we pick Option A, the architectural insight is captured in this spec and the backlog observation. *Recommendation: classify as architectural for the discovery + decision record, even if implementation is operational.*

2. **Which option?** The framework's value pays off most clearly in Option B — it's the option where we commit to a non-destructive pattern that aligns with the epistemic lifecycle. But B is multi-session work; A is hours.
   *Recommendation:* **Option D.** Apply A now (unblock the meta loop, restore code-class nodes for 178 sources). File the backlog observation. Treat B as a future architectural change in its own right. This makes the test of claude-ultra concrete: discovery surfaced a deeper issue, and the framework let us defer it cleanly with a tracked observation rather than scope-creep this change.

3. **What about the existing LLM-extracted "noise" nodes (concept/technology/framework)?** Under Option A's hard-delete, they're gone. Under Option B's supersession, they remain visible-with-supersession-mark. Under Option C, we explicitly delete only the noise.
   *Recommendation:* Tied to the Option choice. If A or D, accept the deletion (the nodes are LLM-extractor abstractions on code chunks, not valid prior state worth preserving). If B, supersession is the right answer.

4. **Backfill scope.** The 178 are the headline number, but `data_health` reports 192 unsummarized sources total — 14 are not `source_type = 'code'`. Do we expand scope to all 192, or stay strictly on the 178 broken-code ones?
   *Recommendation:* Stay on the 178. Other unsummarized sources have different root causes (see issue #176 for the breakdown by extension: 166 .rs, 11 .go, 1 .html). Treat the .html source separately if at all.

5. **Verification criteria for "fixed."** What proves the change worked?
   *Recommendation (post-implementation):*
   - `covalence_data_health` reports `unsummarized_sources < 30` (down from 192)
   - `covalence_health` reports `total_code_entities > 1500` (up from 101) and `entity_summary_pct > 95%` for code sources
   - Spot-check one previously-broken source (`hooks.rs`): `code`-class node count > 0
   - Search for an explicit code identifier (e.g., `synthesize_cooccurrence_edges`) returns the corresponding code node, not a domain abstraction

6. **GitHub issue.** Issue #176 is already open and accurately describes the symptom. We reference it; we don't open a new one.
   *Recommendation:* Close #176 when the fix lands; reference both `change/0002-restore-code-extraction-data` and the manifest in the closing comment.

## Cells affected (rough cross-product, full enumeration at freeze)

12 modules × 11 concerns = **132 cells**. Distribution by chosen option:

- **Option A or D:** ~5–8 `complete`/`in-progress`, ~124–127 `n/a` (most modules untouched)
- **Option B:** ~25–35 `complete`/`in-progress`, ~97–107 `n/a` (engine_core + engine_workers + engine_search bear the weight)
- **Option C:** ~3–5 `complete`, ~127–129 `n/a`

Provisional cells likely to be `complete` under Option D:
- `engine_workers.observability` — verify reprocessed-source telemetry exists / add counter
- `engine_workers.error_handling` — partial-failure semantics for the 178-job batch
- `engine_workers.cli_ux` — `cove admin reprocess --filter "source_type=code AND no_code_nodes"` (if added; or a SQL-driven one-off)
- `engine_workers.tests` — regression test that reprocess on a fresh code source produces ≥1 code-class node
- `engine_core.provenance` — verify INV-2 holds across reprocess (provenance preserved or recreated correctly)
- `documentation.documentation` — this spec, decisions.md, possibly a backlog issue, possibly a Wave-27 ingestion-fix retrospective entry

## Proposed amendments to `docs/architecture/`

- **`docs/architecture/concerns/provenance.md`** — clarify that provenance must survive reprocess (currently silent on this).
- **`docs/architecture/modules/engine_workers.md`** — add a paragraph naming the data-lifecycle policy as a concern owned by this module's reprocess flow.

## Proposed ADRs

None for Option A/C/D. **For Option B**, propose ADR-0025: "Soft supersession for chunks and extractions on reprocess" — records the decision to add `superseded_by`/`superseded_at` and the read-path filter convention.

## Backlog issue (filed regardless of option)

`.changes/backlog/ISSUE-0001-reprocess-hard-deletes-despite-superseded-naming.md` — observation that `services/source/reprocess.rs:54-57` hard-deletes despite the result type's `extractions_superseded: u64` field name implying supersession. Track for future architectural treatment (likely Option B's body of work).

## Reconciliation expectations

Tied to chosen option. For Option D:
- All 132 cells valid; ~125 `n/a` with justifications
- The 178 broken sources show `code`-class nodes after reprocess
- `data_health` and `health` metrics improve as predicted in Open Question 5
- Backlog issue is filed and references this change
- INV-2 (provenance) verified to hold post-reprocess

If reconciliation reveals new issues (e.g., reprocess fails for some sources), surface them as in-flight discoveries; either patch the spec or split into a follow-up change.
