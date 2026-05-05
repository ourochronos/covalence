# ADR-0024: Adopting claude-ultra for architectural change management

**Status:** Accepted

**Date:** 2026-05-05

## Context

Covalence has crossed the size and complexity threshold where single-PR review is insufficient evidence that a change ships in a consistent state across all the modules and cross-cutting concerns it touches. The codebase is 11+ modules (engine_core, search, ingestion, graph_consolidation, epistemic, api, workers, clients, dashboard, extensions, infra, documentation), and changes routinely cross several of them — e.g., adding a new search dimension touches engine_core (types/storage), engine_search (algorithms), engine_ingestion (whatever the dimension indexes), engine_api (handler/schema), clients (CLI flag), and documentation (spec/06-search.md, ADR if novel). The "Holistic changes" checklist in `CLAUDE.md` already names this need; what was missing was a durable, machine-checkable artifact that makes "did this change actually update everything it should have?" verifiable.

The `claude-ultra` plugin (locally developed at `/home/covalence/claude-ultra/`) provides exactly this: a manifest-driven, phase-gated workflow where the **manifest** — a TOML file enumerating every cell in the affected-modules × all-concerns cross-product — is the authoritative state of a change. A change is complete when its manifest says so, not when its diff merges. The framework adds three phases (spec change → implementation → reconciliation) with explicit gates between them.

## Decision

Adopt claude-ultra for architectural-change management in Covalence. The framework is opt-in apparatus on top of the existing development practice: local changes (typo fixes, single-module refactors, isolated bug fixes) proceed without it; **architectural** changes (multi-module, public-contract, invariant-touching, module/concern-altering) flow through discovery → proposal → alignment → freeze, then implementation, then reconciliation.

### Module catalog (12 modules)

`engine_core`, `engine_search`, `engine_ingestion`, `engine_graph_consolidation`, `engine_epistemic`, `engine_api`, `engine_workers`, `clients`, `dashboard`, `extensions`, `infra`, `documentation`. Path globs in `.changes/catalogs/modules.toml`. The split is fine-grained: `covalence-core` is split into 5 modules (core, search, ingestion, graph+consolidation, epistemic) so that subsystem-local changes have small cell footprints. `process` artifacts (`.changes/`) are scoped under `documentation` rather than as a separate module — a deviation from claude-ultra's reference partition.

### Concern catalog (11 concerns)

Claude-ultra's 8 defaults (`data_model`, `persistence`, `observability`, `tests`, `documentation`, `cli_ux`, `error_handling`, `security`) plus 3 Covalence-specific concerns:

- **provenance** — canonical byte-offset sourcing, the Offset Projection Ledger
- **epistemic_model** — opinion tuples, confidence propagation, decay
- **graph_integrity** — PG-as-source-of-truth, sidecar sync, no graph algorithms in SQL

These three reflect Covalence's hybrid-GraphRAG architecture: every architectural change must *consider* whether it affects provenance, the epistemic model, or graph integrity, even if the answer is `n/a` with a written justification.

### Invariants (8)

INV-1 through INV-8, sourced from `CLAUDE.md`'s "Hard Rules" section. Documented at `docs/architecture/invariants.md`. No new invariants are introduced by this ADR — the bootstrap codifies what Covalence already commits to.

### Holistic spec at `docs/architecture/`

A new `docs/architecture/` directory holds the claude-ultra holistic spec: `overview.md`, `invariants.md`, `modules/<name>.md` (11 stubs), `concerns/<name>.md` (11 stubs). This is **separate** from `spec/` (13 mature subsystem designs) — the architecture spec is a higher-level companion that names the partition and the cross-cutting properties; per-module stubs cross-reference the relevant `spec/NN-name.md`.

### Branch convention

- `change/<change-id>` for managed architectural changes (e.g., `change/0001-bootstrap`)
- `feature/issue-N` (existing convention) for non-managed work

### Relationship to existing artifacts

- `CLAUDE.md` adds a short "Change Management" section pointing at `.changes/` and the workflow; it remains authoritative for Covalence-specific discipline (Hard Rules, Anti-Patterns, the Meta Loop).
- `spec/` is unchanged — 13 mature design specs continue to anchor subsystem-level details.
- `docs/adr/` continues as the home for accepted Covalence decisions; this ADR is the only Covalence-side decision recorded for the framework adoption itself. Claude-ultra's own framework decisions (manifest as durable state, phase-one alignment loop, local backlog, assistant-driven workflow) live in the plugin's own ADR-0001 through ADR-0004 and are not duplicated.

## Consequences

**Positive:**
- Cross-module changes ship in verifiable, consistent states. "Done" is mechanical, not vibes.
- New concerns (provenance, epistemic_model, graph_integrity) are forced to be considered for every change, even when they're trivially `n/a` — the discipline is in the considering.
- The backlog under `.changes/backlog/` becomes the local authoritative tracker for tech debt and observations not yet promoted to GitHub issues.
- Discovery → proposal → alignment surfaces real disagreement before code is written; the "Wait, why are we doing this?" conversation happens at the right time.

**Negative / cost:**
- Each architectural change incurs ~132 cells of authoring/justifying. Most are auto-`n/a`, but the discipline tax is real for small-but-architectural changes.
- Two concurrent changes amending the same `docs/architecture/` section must rebase the second through change-spec authoring at close time — a serialization bottleneck.
- The framework adds an artifact layer (`.changes/`) that needs to stay in sync with reality. Drift between manifest and code is its own bug class.

**Mitigations:**
- The escape hatch: explicitly saying "skip the process" / "local change, no apparatus" bypasses claude-ultra. Bypass-rate trending up is signal the framework is too heavy for some change shapes; surface in retros.
- The classifier defaults to architectural when uncertain — cheaper to over-process than to drift.
- Backlog curation is active (per CLAUDE.md's "Track What You Find" plus claude-ultra's own backlog rules); issues don't evaporate.

## Alternatives Considered

1. **Bare-minimum scaffold** — empty catalogs, stub overview/invariants, no manifest discipline. Rejected: catalogs would be guessed rather than reflect Covalence, and every future change would start by fixing them.

2. **Don't adopt; rely on the existing "Holistic changes" checklist in `CLAUDE.md`.** Rejected: the checklist is good intent but produces no durable artifact. There's no machine-checkable proof that a change updated everything; reviewers eyeball it, miss things, and inconsistencies ship.

3. **Build a Covalence-specific change-tracking system.** Rejected: claude-ultra is mature enough for this purpose, fits the workflow, and dogfooding it provides feedback to the plugin itself.

## Implementation

This ADR is the only artifact in `docs/adr/` produced by change `0001-bootstrap`. The full apparatus is delivered together: catalogs, holistic spec stubs, manifest, branch, GitHub issue. See `.changes/active/0001-bootstrap/spec.md` for the full bootstrap proposal and `.changes/active/0001-bootstrap/manifest.toml` for the cell-by-cell completion state.
