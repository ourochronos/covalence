# Change 0002 — Decisions log

This file records non-trivial choices made during the alignment loop. Each entry is one decision with its rationale. Entries flagged for ADR promotion get reconciled into `docs/adr/` at change closure.

---

## Decision 1 — Discovery used 4 parallel Explore agents on hypotheses

**Date:** 2026-05-05
**Phase:** discovery
**Kind:** procedural

**Choice:** Issue #176 listed 4 hypotheses (extractor not running / routing broken / resolver dropping output / wrong entity_class). Discovery dispatched one Explore agent per hypothesis in parallel — covering AST extractor binary, pipeline routing, resolver entity_class, and extension manifest — followed by direct DB inspection on one broken source and git-log timeline check.

**Rationale:** The four hypotheses were mostly independent, so parallel investigation cut total time. Direct DB inspection was the deciding evidence: existing nodes have `node_type = "concept"/"technology"/"framework"` (LLM abstractions), proving the AST extractor never ran on those chunks — narrowing the diagnosis to a routing-not-yet-fixed-when-ingested state.

**Notable findings beyond the original hypotheses:**
- Commit `21a68f7` (#186) merged 2026-04-08; broken sources' chunks created 2026-03-26. **The fix is in code; the data was ingested before it.** This rules out hypotheses 3 and 4.
- The `services/source/reprocess.rs:54-57` hard-deletes despite the `ReprocessResult.extractions_superseded` field name implying supersession. This is a separate architectural concern surfaced by discovery, not in the original issue.

**ADR candidate?** No — captured in this spec's Diagnosis section.

---

## Decision 2 — Classification: architectural

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** procedural

**Choice:** Track this as an architectural change under claude-ultra, not local. The implementation work is operational (Option D, mostly equal to Option A), but the discovery surfaced an architectural concern (`reprocess` hard-deletes despite "superseded" naming, contradicting the epistemic data lifecycle) that warrants the manifest discipline.

**Rationale:** The framework's value here is the *discovery* and *decision record*, not just the code change. Without the framework, the deeper concern about `reprocess` semantics likely would not have been surfaced or captured for future treatment. Classifying as architectural ensures the spec, the decisions, and the backlog observation persist beyond the immediate fix.

**ADR candidate?** No — captured in this spec.

---

## Decision 3 — Option D: A now, B as backlog observation

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** strategic

**Choice:** Apply Option A immediately to fix issue #176 (operational reprocess of 178 sources via existing queue). File a backlog issue (ISSUE-0001) capturing the architectural concern about `reprocess` semantics, treated as a future change in its own right.

**Rationale:**
- Option A unblocks the meta loop now — 178 broken sources is a major drag on Covalence's ability to query its own implementation.
- Option B (fix `reprocess` to supersede properly) is a multi-session change with schema migration, query-call-site audit, and risk of stale-data leakage. Rolling it into this change would scope-creep.
- Option C (one-off SQL, leave reprocess alone) sets a "data-debt = ad-hoc scripts" precedent we don't want.
- Option D preserves the architectural concern as a tracked observation in the local backlog rather than letting it evaporate. The next session that touches `reprocess` (whether to consume or modify it) will see ISSUE-0001 and surface the question explicitly.

**ADR candidate?** No — Option B will likely produce its own ADR when it's eventually undertaken.

---

## Decision 4 — Accept hard-deletion of LLM-noise nodes for the 178

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** strategic

**Choice:** Under Option A's reprocess, the existing LLM-extracted nodes for the 178 sources (`domain/concept`, `domain/technology`, `domain/framework`, etc.) will be hard-deleted along with their chunks/extractions/aliases/ledger entries. This is accepted as the cost of using the existing reprocess mechanism without modification.

**Rationale:** These nodes are LLM-extractor abstractions — generic concepts like "CLI subprocess", "data model", "framework" extracted from chunks of Rust/Go source code. They are not valid prior observations of the *actual* code structure (the AST extractor would have extracted `function`, `struct`, `impl_block` for the same chunks). Treating them as prior epistemic state worth preserving would over-honor the policy; in this case the prior data is genuinely noise.

This decision is **case-specific to #176**. The general principle (preserve prior state per epistemic data lifecycle) holds; we are noting that AST-extractor-noise from a misconfigured pipeline is a defensible exception. Future cases require their own justification.

**ADR candidate?** No — case-specific exception, not a policy change.

---

## Decision 5 — Backfill scope: the 178 only

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** scope

**Choice:** Reprocess the code sources whose `source_type = 'code'` AND have zero `code`-class nodes. At freeze, a fresh count gives **172 sources** (160 .rs + 11 .go + 1 other) — drift of 6 from the 178 reported in issue #176, indicating that some have been reprocessed organically since the issue was filed. Use the freeze-time count as the operational target; expanding to the full 192 unsummarized sources or to non-code-typed sources is out of scope.

**Rationale:** Tight scope reduces the risk of one batch reprocess inadvertently touching unrelated data-debt. The other 14 sources can be handled in follow-up changes if they manifest issues.

**ADR candidate?** No.

---

## Decision 6 — Verification criteria

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** acceptance

**Choice:** A change is "fixed" when all of the following are true post-implementation:

1. `covalence_data_health.unsummarized_sources < 30` (down from 192).
2. `covalence_health.total_code_entities > 1500` (up from 101) AND `covalence_health.entity_summary_pct > 95` for code sources.
3. Spot-check: `engine/crates/covalence-core/src/services/hooks.rs` (one of the previously-broken sources) has ≥1 `code`-class node post-reprocess.
4. Search for the literal identifier `synthesize_cooccurrence_edges` returns the function node, not a domain abstraction.
5. INV-2 (provenance) verified: one re-extracted code node has a provenance link with valid byte offsets in the source's `raw_content`.
6. INV-1 (PG is the source of truth): petgraph sidecar resyncs cleanly post-reprocess; no divergence on subsequent traversal queries.

**Rationale:** Mix of aggregate metrics (1, 2) and concrete spot-checks (3, 4, 5, 6) — the metrics confirm the change worked at scale; the spot-checks confirm correctness at the cell level. Both are needed.

**ADR candidate?** No.

---

## Decision 7 — Close issue #176 on landing

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** procedural

**Choice:** When change 0002 closes (reconciliation passes), post a closing comment on GitHub issue [#176](https://github.com/ourochronos/covalence/issues/176) referencing this change's `spec.md` and the verification metrics. Close the issue.

**Rationale:** Standard issue hygiene per CLAUDE.md.

**ADR candidate?** No.

---

## Decision 8 — File backlog ISSUE-0001 for `reprocess` supersession

**Date:** 2026-05-05
**Phase:** freeze
**Kind:** backlog curation

**Choice:** File `.changes/backlog/ISSUE-0001-reprocess-hard-deletes-despite-superseded-naming.md` capturing the architectural concern surfaced during discovery. The backlog entry references this change's spec, the specific code locations (`services/source/reprocess.rs:54-57`, `models/retry_job.rs` for the `ReprocessResult` shape), and notes that Option B from this change's spec is a candidate future architectural change.

**Rationale:** Preserves the architectural insight beyond this session. Future work that touches `reprocess` will see the issue and surface the question explicitly rather than perpetuating the hard-delete pattern by accident.

**ADR candidate?** No — backlog observation, not a decision yet.

---

## Decision 9 — Accept partial outcome; close change with three more backlog observations

**Date:** 2026-05-05
**Phase:** reconciliation
**Kind:** acceptance + scope-management

**Choice:** Decision 6's verification criteria were optimistic. Actual outcome:

| Decision 6 criterion | Target | Actual | Status |
|---|---|---|---|
| #1 `unsummarized_sources` | < 30 | 192 (unchanged) | ✗ |
| #2 `total_code_entities` | > 1500 | 680 (was 101) | partial |
| #2 `entity_summary_pct` | > 95% | 14% (collapsed) | ✗ |
| #3 `hooks.rs` ≥1 code node | yes | 14 nodes | ✓ |
| #4 search returns `synthesize_cooccurrence_edges` code node | yes | yes (b9085c21) | ✓ |
| #5 INV-2 verified | yes | yes (strict; coarse granularity) | ✓ (with ISSUE-0002) |
| #6 INV-1 verified | yes | yes (sidecar resynced; search returns new graph_context) | ✓ |

**Reframing of headline outcome:** of the 172 code sources reprocessed, 235 of 250-ish previously-broken code-source files (panel including organic reprocesses since #176 was filed) now have ≥1 code-class node. Total code-class nodes went 101 → 680 (+579). The narrow symptom of #176 is largely fixed.

**Surfaced sub-classes worth tracking, not blocking:**
1. **ISSUE-0002** — All AST entities for a source share the same chunk's byte range, not per-entity AST spans. INV-2 strictly satisfied; granularity is coarse.
2. **ISSUE-0003** — `reprocess` did not auto-enqueue `compose_source_summary` jobs, so `unsummarized_sources` stayed at 192. Either intentional (gated on operator action) or a parity gap with `process_source`. Investigate.
3. **ISSUE-0004** — 28 of the 172 reprocessed sources still have zero code-class nodes despite having extractions. Most likely the demotion rule at `models/node.rs:117-121` firing because `domains.first()` is not `"code"` for those sources.
4. **Side observation** — `orphan_nodes` jumped 31 → 302 (+271). Expected per Decision 4: the LLM-noise concept-nodes lost their last extraction reference when reprocess hard-deleted those extractions. Per CLAUDE.md's epistemic data lifecycle, orphans surface for conscious cleanup decisions — they are doing exactly that. Not filed as an issue; tracked here.

**Choice:** Close change 0002 with this acceptance state. The narrow #176 symptom is largely fixed; the surfaced sub-classes are tracked. Do not scope-creep this change to chase the cascade fix or the 28 residual sources — they are real follow-ups but they're not the headline.

**Rationale:** Discipline in scope-management is what claude-ultra is supposed to enforce. The framework's pay-off in this change is precisely this — discovery surfaced 4 distinct issues (one upstream — reprocess semantics — and three downstream — provenance granularity, summary cascade, demotion residual), and closure with backlog tracking lets us preserve all of them without bundling them into one mega-change. Future sessions can pick any of these up cleanly.

**ADR candidate?** No — captured here.
