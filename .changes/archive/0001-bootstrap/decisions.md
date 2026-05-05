# Change 0001 — Decisions log

This file records non-trivial choices made during the alignment loop. Each entry is one decision with its rationale. Entries flagged for ADR promotion get reconciled into `docs/adr/` at change closure.

---

## Decision 1 — Bootstrap pattern follows claude-ultra's own first change

**Date:** 2026-05-05
**Phase:** discovery
**Kind:** procedural

**Choice:** Author the bootstrap manually (no `change init` tooling exists yet for brownfield) but follow the same artifact shape as `claude-ultra/.changes/archive/0001-bootstrap/`: `spec.md`, `decisions.md`, `manifest.toml` (at freeze), `proposed-amendments/` (empty by exception), `proposed-adrs/`. This change number resets to 0001 because change-id space is per-host-project, not global.

**Rationale:** The reference implementation is well-tested through the claude-ultra dogfood; reusing its shape minimizes per-project framework drift. Brownfield-specific scaffolding (`change init`) is tracked upstream as claude-ultra change 0015 — until then, manual is correct.

**ADR candidate?** No — the choice is local to this bootstrap, not a Covalence-wide policy.

---

## Decision 2 — Discovery used parallel Explore agents over the workspace

**Date:** 2026-05-05
**Phase:** discovery
**Kind:** procedural

**Choice:** Two Explore agents dispatched in parallel — one for module partition (paths globs, granularity), one for concerns + invariants extraction (cross-cutting properties, INV-N translation). Both returned drafts; the assistant reconciled and path-checked against real directory contents.

**Rationale:** Module identity and invariant identity are independent investigations with no information dependency. Parallelizing kept the discovery turn-around short. Path-checking before proposal avoids alignment-loop time spent on dead globs.

**Notable Explore findings:**
- Module agent missed several services files (`statement_pipeline.rs`, `ask.rs`, `source/`, etc.) and got `ingestion_helpers.rs` path wrong — fixed in the proposal.
- Concerns agent introduced specific implementation claims (e.g., "outbox pattern target <1s") that were not in CLAUDE.md — those were stripped from the invariants in the proposal, keeping invariant text close to the literal Hard Rules.

**ADR candidate?** No.

---

## Decision 3 — Module granularity: fine (12 modules)

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** structural

**Choice:** 12 fine-grained modules, with `covalence-core` split into 5 (engine_core, engine_search, engine_ingestion, engine_graph_consolidation, engine_epistemic). Coarse alternative was 8 modules with covalence-core collapsed.

**Rationale:** Search and ingestion evolve on different cadences and have different test fixtures; collapsing them produces noisy "engine_core touched" signals during reconciliation. The 132-cell tax (12 × 11) is paid once per change at scaffold time, mostly auto-`n/a`.

**Note:** The proposal initially said "11 modules" — an off-by-one error in counting. The catalog has had 12 modules since the proposal was first drafted; the count was corrected during freeze. No structural change, just arithmetic.

**ADR candidate?** No — captured in ADR-0024.

---

## Decision 4 — Concern count: 11 (claude-ultra defaults + 3 Covalence-specific)

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** structural

**Choice:** 11 concerns: claude-ultra's 8 defaults plus `provenance`, `epistemic_model`, `graph_integrity`. Alternative was 8 concerns with the Covalence-specific properties existing only as invariants.

**Rationale:** Invariants describe what must hold; concerns are checklists every change must consider. A new ingestion feature must consider provenance even if no invariant fires — that's the discipline-of-considering point. Pushing them to invariants only would lose the per-change cell discipline.

**ADR candidate?** No — captured in ADR-0024.

---

## Decision 5 — `spec/` and `docs/architecture/` coexist

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** structural

**Choice:** Keep both. `spec/` retains its 13 mature subsystem designs (architecture, data model, storage, graph, ingestion, search, epistemic model, API, federation, lessons, evaluation, code ingestion, cross-domain analysis). `docs/architecture/` is a higher-level companion containing the holistic spec that the claude-ultra framework reads: overview, invariants, per-module + per-concern stubs. Per-module stubs cross-reference the relevant `spec/NN-name.md`.

**Rationale:** The two artifacts serve different purposes — `spec/` is depth, `docs/architecture/` is breadth. Migrating or duplicating spec/ content would create drift. Cross-referencing keeps both authoritative for their scope.

**ADR candidate?** No — captured in ADR-0024.

---

## Decision 6 — `CLAUDE.md` gains a "Change Management" section

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** documentation

**Choice:** Insert a ~15-line "Change Management (claude-ultra)" section into `CLAUDE.md` directly after the project Overview. The section names the four `.changes/` subdirectories, points at `docs/architecture/`, summarizes when to classify a change as architectural, and records the branch-naming convention.

**Rationale:** A fresh assistant session needs to know the framework is active without depending on the skill description firing. Placing it near the top of CLAUDE.md ensures it loads with the project instructions.

**ADR candidate?** No.

---

## Decision 7 — Branch convention: `change/<id>` for managed work

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** convention

**Choice:** Architectural (managed) changes use `change/<change-id>` branches (e.g., `change/0001-bootstrap`). Non-managed work continues to use `feature/issue-N` per Covalence's existing convention. The split is documented in CLAUDE.md and in ADR-0024.

**Rationale:** Branch name is a fast classifier signal — anyone reading `git branch -a` can immediately tell which branches are tracked through the manifest apparatus. Maintains the existing `feature/` convention for non-managed work.

**ADR candidate?** No — captured in ADR-0024.

---

## Decision 8 — Backlog seeded empty

**Date:** 2026-05-05
**Phase:** alignment → freeze
**Kind:** procedural

**Choice:** `.changes/backlog/` starts with only `README.md` and `_TEMPLATE.md`. None of claude-ultra's 13 framework-limitation issues are pre-imported.

**Rationale:** Those issues describe claude-ultra's own state, not Covalence's. They will manifest organically during real use here (or won't); when they do, file them locally as observations. Pre-importing would clutter the backlog with theoretical concerns.

**ADR candidate?** No.

---

## Decision 9 — `.changes/**` paths scoped under `documentation` module

**Date:** 2026-05-05
**Phase:** freeze
**Kind:** structural deviation

**Choice:** `.changes/**` paths are included in the `documentation` module's globs, rather than introducing a separate `process` module (as claude-ultra's reference catalog does).

**Rationale:** Pragmatic — keeps the module count down and conflates two related artifact kinds (documentation and change-management state) under one bucket. Both evolve together for any architectural change. The deviation is recorded here and in ADR-0024 as a soft signal: if this conflation produces friction in practice (e.g., scope-detection misfires, or `documentation` cells become high-noise from manifest updates), promote `process` to its own module in a future change.

**ADR candidate?** No — captured as a noted deviation in ADR-0024.

---

## Decision 10 — GitHub issue opened for the bootstrap

**Date:** 2026-05-05
**Phase:** freeze
**Kind:** procedural

**Choice:** GitHub issue [#201](https://github.com/ourochronos/covalence/issues/201) opened ("Adopt claude-ultra change-management framework", label `enhancement`) and referenced from the change branch's commit message. CLAUDE.md mandates issues for non-trivial work — adopting a framework qualifies.

**Rationale:** Project convention. Issue is the durable cross-session artifact in Covalence's existing tracking system; the change manifest is the durable artifact in claude-ultra's. Both are referenced.

**ADR candidate?** No.

---

## Decision 11 — Bootstrap exceptions to closure-only artifact rules

**Date:** 2026-05-05
**Phase:** freeze
**Kind:** bypass (recorded per the framework's escape-hatch convention)

**Choice:** Two artifact-placement exceptions are taken in this bootstrap, deviating from the framework's normal closure-gated flow:

1. **`docs/architecture/` written directly, not via `proposed-amendments/`.** Normally, edits to the holistic spec stage in `.changes/active/<id>/proposed-amendments/` during phase two and merge at closure. The bootstrap creates `docs/architecture/` from scratch — there is nothing yet to amend — so the holistic spec stubs are written directly to `docs/architecture/`. The `proposed-amendments/` directory in this change is intentionally empty; its `README.md` records the exception.

2. **ADR-0024 written directly to `docs/adr/`, not via `proposed-adrs/`.** Normally, proposed ADRs stage in `.changes/active/<id>/proposed-adrs/` and promote at closure. ADR-0024 records the bootstrap decision itself; it is written directly to `docs/adr/0024-adopting-claude-ultra.md` so the decision is discoverable from the moment the framework is installed (without the consumer having to know about an in-flight `proposed-adrs/` path). The `proposed-adrs/` directory contains only `.gitkeep`.

**Rationale:** Both exceptions are intrinsic to bootstrapping the framework — there is no way to "stage and merge at closure" something that creates the staging mechanism itself. Claude-ultra's own `0001-bootstrap` change took the same pair of exceptions; we follow that pattern.

**Implications:**
- Future changes do *not* inherit these exceptions. They use `proposed-amendments/` and `proposed-adrs/` per the normal closure flow.
- Reconciliation for change 0001 explicitly checks that the exceptions are limited to this bootstrap (no later change should bypass closure for amendment placement).

**ADR candidate?** No — the exceptions are recorded in this decision and noted in ADR-0024's "Implementation" section, but they are not architectural decisions on their own.
