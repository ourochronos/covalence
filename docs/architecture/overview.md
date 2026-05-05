# Covalence — architecture overview (claude-ultra holistic spec)

This document is the index into Covalence's holistic spec from the perspective of the claude-ultra change-management framework — the living description of the system's intended state at the module/concern/invariant level. It evolves by amendment through change closure. It is **not** the place to describe in-flight work; that lives in `.changes/active/`.

For deep subsystem designs (search, ingestion, epistemic model, graph, etc.), see `spec/` (13 design specs). This document is a higher-level companion: it names the partition, captures the cross-cutting concerns, and pins the invariants. Per-module stubs cross-reference `spec/` where relevant.

For framework decisions (manifest as durable state, phase-one alignment loop, local backlog as authoritative, assistant-driven workflow), see the claude-ultra plugin's own ADRs at `.claude/plugins/cache/claude-ultra-local/claude-ultra/docs/adr/` (ADR-0001 through ADR-0004). They are not duplicated in Covalence's `docs/adr/`.

## Purpose

Covalence is a hybrid GraphRAG knowledge engine. It ingests unstructured sources, builds a property graph with rich epistemic annotations, and provides multi-dimensional fused search. The engine is large enough that single-PR review is insufficient evidence of cross-cutting consistency — claude-ultra's manifest discipline addresses this.

## Invariants

See [invariants.md](invariants.md). Eight invariants, sourced from `CLAUDE.md`'s "Hard Rules" section. These are the properties the system must hold; violations are bugs in the engine, not edge cases.

## Modules

12 modules. Catalog: [`.changes/catalogs/modules.toml`](../../.changes/catalogs/modules.toml).

- [engine_core](modules/engine_core.md) — covalence-core foundations: types, models, storage, config, services facade
- [engine_search](modules/engine_search.md) — 6-dimensional search fusion and strategies
- [engine_ingestion](modules/engine_ingestion.md) — pipeline, chunking, extraction, embedding
- [engine_graph_consolidation](modules/engine_graph_consolidation.md) — petgraph sidecar, algorithms, HDBSCAN consolidation
- [engine_epistemic](modules/engine_epistemic.md) — Subjective Logic, fusion, decay
- [engine_api](modules/engine_api.md) — HTTP + MCP server (Axum, utoipa)
- [engine_workers](modules/engine_workers.md) — worker queue, AST extractor, eval harness, migrations, extensions loader
- [clients](modules/clients.md) — cove CLI (Go) and MCP server bridge (Node.js)
- [dashboard](modules/dashboard.md) — web dashboard
- [extensions](modules/extensions.md) — extension manifests
- [infra](modules/infra.md) — build, deploy, sidecar containers, scripts
- [documentation](modules/documentation.md) — specs, ADRs, design docs, claude-ultra apparatus

## Concerns

11 concerns. Catalog: [`.changes/catalogs/concerns.toml`](../../.changes/catalogs/concerns.toml).

- [data_model](concerns/data_model.md) — schemas, types, validation
- [persistence](concerns/persistence.md) — PostgreSQL schema, migrations, serialization
- [observability](concerns/observability.md) — tracing, logs, Prometheus metrics
- [tests](concerns/tests.md) — unit, integration, evaluation harness coverage
- [documentation](concerns/documentation.md) — guides, inline rationale, ADR when warranted
- [cli_ux](concerns/cli_ux.md) — cove CLI behavior, output, error messages
- [error_handling](concerns/error_handling.md) — thiserror/anyhow split, exit codes
- [security](concerns/security.md) — clearance levels, secrets, egress
- [provenance](concerns/provenance.md) — canonical byte-offset sourcing
- [epistemic_model](concerns/epistemic_model.md) — opinion tuples, confidence propagation
- [graph_integrity](concerns/graph_integrity.md) — PG-as-source-of-truth, sidecar sync

## Workflow

Three phases per architectural change, driven by the claude-ultra plugin:

1. **Spec change** — discovery → proposal → alignment → freeze. Output: change spec, manifest, proposed amendments, proposed ADRs.
2. **Implementation** — work the manifest cell by cell. Decision log captures non-trivial choices.
3. **Reconciliation** — cross-check implementation against spec; iterate until clean. On close: amendments merge into this directory; ADRs promote to `docs/adr/`.

The full workflow rules live in the claude-ultra skill (`skills/architectural-change/SKILL.md`) shipped by the plugin and surfaced via system reminders.

## Relationship to other artifacts

| Artifact | Purpose |
|----------|---------|
| `CLAUDE.md` | Project instructions for the assistant — rules, anti-patterns, patterns, the meta loop |
| `spec/` | 13 mature design specs for subsystems (architecture, data model, storage, graph, ingestion, search, epistemic model, API, federation, lessons, evaluation, code ingestion, cross-domain analysis) |
| `docs/adr/` | 23+ accepted Covalence architecture decisions |
| `docs/architecture/` (this directory) | claude-ultra holistic spec — modules, concerns, invariants |
| `.changes/` | Active changes, archive, backlog, catalogs |
| `MILESTONES.md` | Phased roadmap (M0–M11 + waves) |
| `VISION.md` | Longer-term vision |
