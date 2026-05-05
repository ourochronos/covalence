# Change 0001 — Bootstrap claude-ultra in Covalence

**Status:** proposal (phase one — alignment loop)
**Kind:** meta (framework being installed in this repo)
**Created:** 2026-05-05
**Branch (at freeze):** `change/0001-bootstrap`

## Intent

Stand up the claude-ultra framework in the Covalence repo so subsequent architectural changes can be managed under its discipline: manifest-driven, phase-gated, with a module × concern catalog and explicit invariants. This change creates the framework apparatus only — it does not modify engine code, CLI, MCP server, or any product surface.

## Origin

Chris asked to "set up claude-ultra in this project." Two paths were considered: a bare-minimum scaffold with empty catalogs, or a dogfooded bootstrap where the install itself is the framework's first managed change. We chose the latter — it forces the catalogs to reflect Covalence's actual structure rather than be guessed, matches how claude-ultra was built for itself, and produces a complete first artifact (filed manifest, decisions log, proposed amendments) before the framework is "live."

Discovery was executed by two parallel Explore agents (module partition; concerns + invariants extraction) sourced from `CLAUDE.md`, `docs/adr/`, `spec/README.md`, and the workspace layout. Their drafts were path-checked against real directory contents and reconciled into this proposal.

## Affected scope

Every module proposed below is touched at least minimally — the bootstrap establishes their identities. No engine code is modified.

- **rules** — no edits to `CLAUDE.md` (Covalence's existing project instructions stay authoritative for Covalence-specific discipline). claude-ultra rules ship via the plugin's `skills/architectural-change/SKILL.md`.
- **architecture** — `docs/architecture/` created from scratch: `overview.md`, `invariants.md`, per-module stubs under `modules/`, per-concern stubs under `concerns/`.
- **process** — `.changes/` populated: `catalogs/{modules,concerns}.toml`, this active change directory, empty `archive/`, empty `backlog/`.
- **documentation** — Covalence's `docs/adr/` gains one new ADR (proposed below) recording the adoption decision.
- All other modules — touched only as `n/a` cells in the manifest (the bootstrap doesn't change their code or behavior).

## Implications

- The hook (`UserPromptSubmit`/`Stop`) starts firing the moment `.changes/` exists in the repo. Per the dispatch contract, every failure path exits 0 silently, so this is non-disruptive — but assistant context will start carrying `[claude-ultra]` annotations.
- Future work that crosses the classifier (multi-module, public-contract, invariant-touching) flows through discovery → proposal → alignment → freeze. Local changes don't.
- Covalence's existing `spec/` directory (13 design specs) and `docs/adr/` (23 ADRs) are not displaced — they retain their roles. `docs/architecture/` is a *separate* artifact serving claude-ultra's manifest companion: it holds the module/concern catalog with one-page stubs, not full subsystem designs.
- The Covalence `CLAUDE.md` and the claude-ultra skill `SKILL.md` will both load. Where they conflict (they shouldn't, but if so), Covalence's project instructions win for Covalence-specific discipline; claude-ultra's rules win for change-management discipline.
- Scope-detection for cells uses path globs in `modules.toml`. Files outside any module (e.g., top-level `CHANGELOG.md`, `LICENSE`, the generated `openapi.json`) are out-of-scope for any change's enforcement and treated as informational by hooks.

## Migration story

None. The repo currently has no `.changes/` and no `docs/architecture/`. Greenfield wrt claude-ultra apparatus.

## Rollback story

`rm -rf .changes/ docs/architecture/`, drop the proposed ADR, drop the `change/0001-bootstrap` branch. Nothing in the engine, CLI, MCP server, dashboard, or extensions changes — the framework is opt-in apparatus on top.

## Proposed module catalog

12 modules. Granularity choice — fine-grained over coarse — is flagged in **Open Questions** below.

```toml
# .changes/catalogs/modules.toml
[modules.engine_core]
description = "covalence-core foundations: types, models, storage repos, config, errors, factory, services facade"
paths = [
    "engine/crates/covalence-core/src/types/**",
    "engine/crates/covalence-core/src/models/**",
    "engine/crates/covalence-core/src/storage/**",
    "engine/crates/covalence-core/src/config*.rs",
    "engine/crates/covalence-core/src/error.rs",
    "engine/crates/covalence-core/src/factory.rs",
    "engine/crates/covalence-core/src/lib.rs",
    "engine/crates/covalence-core/src/metrics.rs",
    "engine/crates/covalence-core/src/services/mod.rs",
    "engine/crates/covalence-core/src/services/adapter_service.rs",
    "engine/crates/covalence-core/src/services/admin/**",
    "engine/crates/covalence-core/src/services/agent_memory.rs",
    "engine/crates/covalence-core/src/services/analysis/**",
    "engine/crates/covalence-core/src/services/article.rs",
    "engine/crates/covalence-core/src/services/ask.rs",
    "engine/crates/covalence-core/src/services/chunk_quality.rs",
    "engine/crates/covalence-core/src/services/config_service.rs",
    "engine/crates/covalence-core/src/services/edge.rs",
    "engine/crates/covalence-core/src/services/health.rs",
    "engine/crates/covalence-core/src/services/hooks.rs",
    "engine/crates/covalence-core/src/services/memory.rs",
    "engine/crates/covalence-core/src/services/node.rs",
    "engine/crates/covalence-core/src/services/noise_filter.rs",
    "engine/crates/covalence-core/src/services/ontology_service.rs",
    "engine/crates/covalence-core/src/services/prompts.rs",
    "engine/crates/covalence-core/src/services/session.rs",
]

[modules.engine_search]
description = "Search engine: 6-dimensional fusion (vector, lexical, temporal, graph, structural, global), strategies, RRF, search service"
paths = [
    "engine/crates/covalence-core/src/search/**",
    "engine/crates/covalence-core/src/services/search/**",
    "engine/crates/covalence-core/src/services/search_helpers.rs",
]

[modules.engine_ingestion]
description = "Ingestion pipeline: chunking, fastcoref, offset projection, statement → triple extraction, embedding, queue, source adapters"
paths = [
    "engine/crates/covalence-core/src/ingestion/**",
    "engine/crates/covalence-core/src/services/pipeline/**",
    "engine/crates/covalence-core/src/services/queue/**",
    "engine/crates/covalence-core/src/services/source/**",
    "engine/crates/covalence-core/src/services/ingestion_helpers.rs",
    "engine/crates/covalence-core/src/services/statement_pipeline.rs",
]

[modules.engine_graph_consolidation]
description = "petgraph sidecar: traversal, algorithms (PageRank, TrustRank, communities), HDBSCAN entity resolution, batch/deep consolidation"
paths = [
    "engine/crates/covalence-core/src/graph/**",
    "engine/crates/covalence-core/src/consolidation/**",
    "engine/crates/covalence-core/src/services/consolidation.rs",
]

[modules.engine_epistemic]
description = "Subjective Logic, Dempster-Shafer fusion, DF-QuAD, decay, convergence, opinion tuples"
paths = [
    "engine/crates/covalence-core/src/epistemic/**",
]

[modules.engine_api]
description = "HTTP API + MCP server: Axum routing, handlers, OpenAPI (utoipa), middleware, error surfaces"
paths = [
    "engine/crates/covalence-api/**",
]

[modules.engine_workers]
description = "Async queue worker, AST extractor sidecar, evaluation harness, sqlx migrations, extension loader"
paths = [
    "engine/crates/covalence-worker/**",
    "engine/crates/covalence-ast-extractor/**",
    "engine/crates/covalence-eval/**",
    "engine/crates/covalence-migrations/**",
    "engine/crates/covalence-core/src/extensions/**",
]

[modules.clients]
description = "Client surfaces: Go cove CLI (Cobra) and Node.js MCP server bridge for Claude Code"
paths = [
    "cli/**",
    "mcp-server/**",
]

[modules.dashboard]
description = "Web dashboard: stats, observability, future interaction"
paths = [
    "dashboard/**",
]

[modules.extensions]
description = "Extension manifests: domain-specific entity types, relationship types, alignment rules, lifecycle hooks"
paths = [
    "extensions/**",
]

[modules.infra]
description = "Build, deploy, sidecar containers, scripts, top-level project metadata"
paths = [
    "Makefile",
    "docker-compose.yml",
    "covalence.conf.example",
    "deploy/**",
    "scripts/**",
    "sidecar/**",
    "sidecars/**",
    ".env.example",
]

[modules.documentation]
description = "Specs, ADRs, design docs, and project-level prose (README, MILESTONES, VISION, CHANGELOG, CONTRIBUTING, CLAUDE.md, claude-ultra holistic spec)"
paths = [
    "spec/**",
    "docs/adr/**",
    "docs/architecture/**",
    "docs/extension-author-guide.md",
    "docs/lifecycle-hooks.md",
    "docs/local-llm.md",
    "docs/providers.md",
    "docs/research-papers.md",
    "docs/sources/**",
    "docs/stdio-service-contract.md",
    "design/**",
    "README.md",
    "MILESTONES.md",
    "VISION.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "CLAUDE.md",
]

# Out of any module's enforcement scope (informational only):
#   LICENSE, openapi.json (generated), package-lock.json, .gitignore,
#   logs/** (per-session artifacts), .claude/**, .gemini/**.
```

## Proposed concerns catalog

11 concerns: claude-ultra's 8 defaults + 3 Covalence-specific (provenance, epistemic_model, graph_integrity).

```toml
# .changes/catalogs/concerns.toml
[concerns.data_model]
description = "Schemas, types, validation rules"

[concerns.persistence]
description = "PostgreSQL schema, migrations, file formats, serialization"

[concerns.observability]
description = "Tracing, structured logs, Prometheus metrics, processing metadata"

[concerns.tests]
description = "Unit tests, integration tests, evaluation harness coverage"

[concerns.documentation]
description = "User-facing guides, inline rationale, ADR if architectural"

[concerns.cli_ux]
description = "cove CLI command behavior, output formatting, error messages"

[concerns.error_handling]
description = "thiserror in library, anyhow in binaries, exit codes, recovery paths"

[concerns.security]
description = "Clearance levels, secrets handling, file-system safety, egress filtering"

[concerns.provenance]
description = "Canonical byte-offset sourcing, offset projection ledger, provenance link integrity"

[concerns.epistemic_model]
description = "Opinion tuples (b, d, u, a), confidence propagation, fusion, decay"

[concerns.graph_integrity]
description = "PG-as-source-of-truth invariant, sidecar sync, no graph algorithms in SQL"
```

## Proposed invariants

8 invariants. These are the existing CLAUDE.md "Hard Rules" translated into INV-N form. No new invariants are introduced by this change — the bootstrap codifies what Covalence already commits to.

**INV-1: PostgreSQL is the source of truth.** The petgraph sidecar is a derived, rebuildable cache. If it diverges from PG, PG wins.
*Implications:* Sidecar mutations are unidirectional from PG. Audit divergence; rebuild rather than reconcile in place. Tooling that asks "what is the current graph?" reads PG semantics, not the sidecar.

**INV-2: Every fact has pristine canonical provenance.** No node, edge, or chunk exists without a provenance link pointing to immutable byte offsets in the canonical source text. Mutated text (e.g., from fastcoref) must be reverse-projected through the Offset Projection Ledger before storage.
*Implications:* Ingestion paths that bypass the ledger are bugs. Synthetic facts (deduced edges, consolidated nodes) link to their derivation source. Chunks without offsets are rejected.

**INV-3: No attention dilution in extraction.** The pipeline uses a strict two-pass LLM extraction model (Statements → Triples). Statement generation and entity extraction must not be merged into a single prompt.
*Implications:* New extraction features extend the two-pass shape rather than collapse it. Single-prompt experiments require explicit ADR justification.

**INV-4: LLM selection is deliberate and provider-attributed.** Every LLM call uses the `ChatBackend` abstraction with `ChainChatBackend` for multi-provider failover. Default chain: Claude Haiku → Copilot → Gemini Flash. Every call records the provider in processing metadata.
*Implications:* Direct API calls bypassing `ChatBackend` are bugs. Provider attribution flows into observability.

**INV-5: No graph algorithms in SQL.** Graph traversal, ranking, and community detection execute against the in-memory petgraph sidecar. PostgreSQL Recursive CTEs are forbidden for graph operations.
*Implications:* Features needing traversal load relevant nodes/edges into the sidecar first. Performance work targets petgraph algorithms, not query plans.

**INV-6: Uncertainty is not disbelief.** The system uses Subjective Logic opinion tuples (b, d, u, a). "Unknown" (high u) is not "50% likely" (high d).
*Implications:* Confidence stored as opinion tuples or derived via explicit fusion rules. Operations that collapse uncertainty to point estimates require justification. Decay raises uncertainty rather than skepticism.

**INV-7: Secure by default with explicit promotion.** All data defaults to `clearance_level = 0` (local_strict). Promotion to federated clearance requires explicit action.
*Implications:* Egress filtering is enforced at query time. Promotion triggers recursive recalculation of derived entities/edges. Default deployments leak nothing.

**INV-8: No synthetic test data.** Tests use real data or clearly-marked fixtures. Benchmarks and results are never fabricated.
*Implications:* Performance comparisons run against real corpora. Quality gates (entity precision, chunk quality) measured on real sources or marked preliminary.

## Cells affected (cross-product)

12 modules × 11 concerns = **132 cells** in the bootstrap manifest. Distribution will be roughly:

- ~14 `complete` — concerns where the bootstrap actually delivers something: each non-`documentation` module's `documentation` cell (the per-module stub at `docs/architecture/modules/<name>.md`), plus the `documentation` module's `data_model`, `persistence`, and `documentation` cells (catalogs, file layout, overview/invariants/stubs/ADR-0024).
- ~118 `n/a` — every other cell, with justifications like "framework apparatus only; no engine code modified."

No cells `deferred`, no cells `pending`, no cells `in-progress`. The bootstrap is one-shot.

The full manifest is generated at freeze.

## Proposed amendments to `docs/architecture/`

This change creates `docs/architecture/` from scratch (analogous to claude-ultra's own bootstrap exception). Future changes use `proposed-amendments/` to stage edits; this change writes amendments directly because there is nothing yet to amend. The empty `proposed-amendments/` directory in this change's tree is intentional and recorded in `decisions.md`.

Files seeded:
- `docs/architecture/overview.md` — index pointing at invariants, modules, concerns
- `docs/architecture/invariants.md` — INV-1 through INV-8
- `docs/architecture/modules/<module>.md` — one stub per module (12 files)
- `docs/architecture/concerns/<concern>.md` — one stub per concern (11 files)

## Proposed ADRs

One new ADR for Covalence's `docs/adr/`:

- **ADR-0024: Adopting claude-ultra for architectural change management.** Records the decision to introduce the manifest-driven, phase-gated workflow; documents the chosen module/concern partition; notes the relationship between Covalence's existing `spec/` (subsystem designs) and `docs/architecture/` (claude-ultra holistic spec). This is the only Covalence-side decision recorded in the ADR system; claude-ultra's own framework decisions (manifest as durable state, phase-one alignment loop, local backlog, assistant-driven workflow) live in the plugin's `docs/adr/ADR-0001..0004` and are not duplicated here.

## Open questions for alignment

The user's call on each — I have a recommendation but want explicit alignment before freeze.

1. **Module granularity (fine vs coarse).** This proposal uses 12 modules with `engine_core` split into 5 (core, search, ingestion, graph_consolidation, epistemic) so that subsystem-local changes have small cell footprints. The alternative is 8 modules with all of `covalence-core` collapsed into one `engine_core`, giving 8 × 11 = 88 cells per change vs 132.
   *Recommendation:* Fine. Search and ingestion evolve on different cadences and have different test fixtures; collapsing them produces noisy "engine_core touched" signals that don't help reconciliation. The 132-cell tax is paid once per change at scaffold time, mostly auto-`n/a`.

2. **Concern count.** This proposal adds 3 Covalence-specific concerns (provenance, epistemic_model, graph_integrity) to claude-ultra's 8. The alternative is to push those into invariants only and keep 8 concerns.
   *Recommendation:* Keep all 11. Invariants describe what must hold; concerns are checklists for *every change*. A new ingestion feature must consider provenance even if no invariant fires — that's the discipline-of-considering point.

3. **`spec/` vs `docs/architecture/` relationship.** Covalence has 13 mature design specs in `spec/`. claude-ultra wants `docs/architecture/` for its holistic spec.
   *Recommendation:* Coexist as proposed. Per-module stubs in `docs/architecture/modules/` cross-reference the relevant `spec/NN-name.md`. The architecture overview is short and indexes into `spec/` for depth. We do not migrate or duplicate `spec/` content.

4. **CLAUDE.md changes.** Currently zero edits to `CLAUDE.md` are proposed.
   *Recommendation:* Add a short section near the top pointing at `.changes/` and the claude-ultra workflow, so a fresh agent session knows the framework is active. Could be 5-10 lines. Or — leave `CLAUDE.md` alone and rely on the skill for surfacing rules; the skill description already triggers on architectural-change keywords.
   *Pending your call.*

5. **Branch naming.** claude-ultra convention: `change/<change-id>` (so `change/0001-bootstrap`). Covalence convention from CLAUDE.md: `feature/issue-N`.
   *Recommendation:* `change/0001-bootstrap` for changes managed under claude-ultra; `feature/issue-N` for unmanaged work. Worth recording in CLAUDE.md as a convention update — let me know.

6. **Backlog seeding.** claude-ultra's own backlog has 13 issues tracking framework limitations. Should Covalence's backlog start empty, or import any of those (e.g., ISSUE-0008 "claude.md doesn't auto-load when plugin is active in a host project") if they manifest here?
   *Recommendation:* Start empty. Watch for the issues to manifest in real use, then file them locally as observations. Don't pre-import — they describe claude-ultra problems, not Covalence problems.

7. **GitHub issue.** CLAUDE.md mandates an issue for non-trivial work.
   *Recommendation:* Open a GitHub issue "Adopt claude-ultra change-management framework" referencing this change-id, labeled `enhancement`. Branch and ADR cite it.

## Reconciliation expectations

When the bootstrap reaches phase three:
- All 132 cells have valid states; every `n/a` has a written justification.
- `docs/architecture/overview.md` links resolve; per-module and per-concern stubs exist.
- `modules.toml` paths globs match real files (no dead paths, no uncovered code that should be in scope).
- INV-1 through INV-8 reference real artifacts (CLAUDE.md "Hard Rules", `spec/04-graph.md`, `spec/07-epistemic-model.md`, etc.).
- Proposed ADR-0024 is referenced from this spec (it is — see above).
- The hook fires and `claude-ultra doctor` (if available) passes for the Covalence repo.

Expected smooth: yes. The bootstrap is small in scope and conservative. If reconciliation surfaces inconsistencies, they likely point at module-partition mistakes (which path goes where) — fix the catalog and re-reconcile.
