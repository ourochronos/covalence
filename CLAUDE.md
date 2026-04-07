# Covalence — Project Instructions

## Overview

Covalence is a hybrid GraphRAG knowledge engine. It ingests unstructured sources (code, specs, research papers, design docs), builds a property graph with rich epistemic annotations (Subjective Logic, causal hierarchy, provenance), and provides multi-dimensional fused search via Reciprocal Rank Fusion. Includes an MCP server for Claude Code integration, grounded Q&A (`/ask`), cross-domain alignment analysis, and an async pipeline with provider-attributed LLM calls.

**Repo:** `ourochronos/covalence`
**License:** MIT

## Architecture

Three-layer design:

```
API Layer     (Axum HTTP + MCP) — thin routing, no business logic
Engine Layer  (Rust) — search, graph sidecar, ingestion, consolidation
Storage Layer (PostgreSQL 17 + pgvector) — single source of truth
```

### Workspace Layout

```
engine/crates/covalence-core/       Library crate — all domain logic
  src/types/                        Newtype IDs, Opinion, ClearanceLevel, CausalLevel
  src/models/                       Domain models (Source, Node, Edge, Chunk, Article, etc.)
  src/storage/traits.rs             Repository traits (8 repos)
  src/storage/postgres/             PostgreSQL implementations of all repo traits
  src/graph/                        petgraph sidecar, algorithms, traversal, community, sync
  src/search/                       RRF fusion, strategies, 6 dimensions (vector, lexical, temporal, graph, structural, global)
  src/epistemic/                    Subjective Logic, DS fusion, DF-QuAD, decay, convergence
  src/ingestion/                    Statement-first pipeline: fastcoref, offset projection, windowed Gemini Flash 3.0 extraction
  src/consolidation/                HDBSCAN Tier 5 entity resolution, batch/deep consolidation
  src/services/                     Service layer (source, node, edge, article, admin, search)
  src/extensions/                   Extension manifest loading, metadata validation, seeding
  src/config.rs                     Environment-driven configuration
  src/config_loader.rs              Layered figment config (covalence.conf + conf.d/ + env)
  src/error.rs                      Typed errors via thiserror
engine/crates/covalence-api/        Binary crate — Axum server, utoipa OpenAPI
engine/crates/covalence-migrations/ Binary crate — sqlx migration runner
engine/crates/covalence-eval/       Binary crate — layer-by-layer evaluation harness
engine/crates/covalence-worker/     Binary crate — async queue worker (per-kind concurrency)
engine/crates/covalence-ast-extractor/ Binary crate — standalone AST extraction service (STDIO)
extensions/                         Extension manifests (5 default: core, code-analysis, spec-design, research, agent-memory)
cli/                                Go CLI (Cobra) — binary name: cove
  cmd/                              Subcommands: source, search, node, admin, memory, ask, llm, graph, audit
  internal/                         HTTP client + output helpers
mcp-server/                         MCP server for Claude Code integration (Node.js, 10 tools)
dashboard/                          Web dashboard (stats, observability, future interaction)
spec/                               Design specs (13 specs + README)
docs/adr/                           Architecture Decision Records (23 ADRs)
```

### Key Dependencies

| Crate/Module | Purpose |
|-------------|---------|
| axum | HTTP framework |
| sqlx | Async PostgreSQL (runtime string queries, SQLX_OFFLINE for tests) |
| petgraph | In-memory directed graph |
| utoipa + utoipa-swagger-ui | OpenAPI spec generation + Swagger UI |
| serde / serde_json | Serialization |
| uuid | Entity identifiers |
| thiserror | Typed errors in library code |
| anyhow | Errors in binary crates only |
| figment | Layered YAML + env config (ADR-0023) |
| dotenvy | Legacy .env loading (pre-figment compat) |
| tokio | Async runtime |
| sha2 | SHA-256 content hashing for dedup |
| unicode-normalization | Unicode NFC normalization in ingestion |
| async-trait | Async trait support for Embedder, Extractor, etc. |
| chrono | Timestamp handling |
| tracing / tracing-subscriber | Structured logging |
| reqwest | HTTP client for OpenAI/Voyage API (embedder + extractor) |
| futures | Concurrent extraction via join_all |
| cobra (Go) | CLI framework |

## Development & Deployment

Configure via `covalence.conf` (see `covalence.conf.example`) or environment variables (`COVALENCE_*`). Legacy `.env` files still supported. Ports, hosts, and deployment targets are environment-specific — not hardcoded.

### Key Commands

```bash
# Development
make dev-db          # Start dev PostgreSQL (Docker)
make migrate         # Run migrations on dev
make reset-db        # Drop + recreate dev DB
make run             # Start engine (reads .env)
make check           # fmt + clippy + tests

# Deployment
make deploy          # Pull, build, migrate, restart on remote host
make promote         # check + migrate-prod + deploy (full pipeline)
make eval-search     # Search regression against prod

# Ingestion
make ingest-codebase # Ingest all .rs and .go files
make ingest-specs    # Ingest spec/*.md
make ingest-adrs     # Ingest docs/adr/*.md
make ingest-prod     # All of the above
```

### Claude Code Directives

When working on Covalence:
- **Use dev for all development work.** `make run` starts the engine.
- **Never modify prod data** without explicit user approval.
- **After adding migrations**, run `make migrate` (dev) first, verify, then `make promote` for prod.
- **Deploy with `make deploy`** — pulls, builds, migrates, and restarts on the configured remote host.

## The Meta Loop

Covalence builds Covalence. The system is its own knowledge substrate — we develop it *through* it. Every session should use Covalence to inform its own improvement, and improve Covalence's ability to inform the next session.

The vision (`VISION.md`) defines what success looks like. The spec describes how to get there. The code implements the spec. The loop ensures all three converge.

### The Cycle

Each autonomous session follows this loop. The loop itself is a target of improvement — if a step is weak, fix the step.

1. **Assess** — Start from the vision, not from the code. Read `MILESTONES.md` to see what Wave is currently active. Query Covalence to understand the current state: search quality, entity precision, graph health. Use `/admin/metrics`, graph stats, knowledge-gaps. Ask: what's the biggest gap between where we are and where VISION.md says we should be?
2. **Execute the Plan** — If there is an active Wave in `MILESTONES.md` with unchecked boxes, your highest priority is to execute the next logical step of that plan. Do not deviate to build side-features until the active plan is complete. Build iteratively, ensuring tests pass at each step, and manually check off the boxes `[x]` in `MILESTONES.md` as you complete them.
3. **Research** — Find and ingest material relevant to the gap. Papers, documentation, RFCs. **Prefer depth over breadth** — one paper read thoroughly beats five skimmed. Verify relevance before committing to ingestion.
4. **Learn** — The most important step. Read deeply enough to extract non-obvious insights — methodology, tradeoffs, failures, adjacent ideas. If you can't articulate what you learned beyond a one-sentence summary, you haven't learned enough.
5. **Build** — Implement the improvement. Measure before building so you know what success looks like. Fix the weakest link first.
6. **Evaluate** — Measurable, not vibes. Before/after comparisons on concrete metrics. "It works now" is not evidence of improvement.
7. **Reflect** — What about the loop itself could be better? What was wasted effort? Be specific.
8. **Update** — Update `CLAUDE.md`, `VISION.md`, issues, and append a comprehensive summary of your session's accomplishments, failures, and insights to `logs/session.md`. The insight is freshest immediately after the work.

### Vision-Driven Prioritization

The vision defines four priorities in order: (1) knowledge quality, (2) observability, (3) agent integration, (4) self-improvement infrastructure. When choosing what to work on, always ask: is the foundation solid enough to support this work? Building a web dashboard while search returns bibliography noise wastes effort. Building agent integration while entity extraction creates garbage pollutes agent memory.

**Quality gates before features:**
- Entity precision >90% before building new query modes
- Search precision@5 >0.8 before building the web search interface
- Chunk quality >95% before expanding ingestion to new source types

### CLI-First Interaction

Use the `cove` CLI as the primary interface to Covalence. This dogfoods the CLI and surfaces usability issues.

```bash
# Search
cove search "subjective logic confidence propagation"
cove search "entity resolution" --strategy precise
cove search "chunking strategies" --mode context

# Inspect sources and graph
cove source list
cove node list --type concept --limit 20
cove graph stats

# Ingest a local file
cove source add /path/to/paper.md

# Admin
cove admin health
cove admin metrics

# Grounded Q&A
cove ask "How does entity resolution work?"
```

If the CLI is missing a feature you need (e.g., URL-based ingestion, bulk operations), add it. That's the loop working.

### Fetching Content for Ingestion

When ingesting web content:
- **Use `firecrawl scrape <url> --only-main-content -o .firecrawl/<name>.md`** as the default for all web content. It handles JavaScript rendering, returns clean markdown, and works reliably across sites including ArXiv.
- **ArXiv `Accept: text/markdown` header no longer works reliably** — always use firecrawl instead.
- **For PDFs**, download the file and ingest directly — the PDF sidecar handles conversion.
- **Prefer markdown over HTML over PDF** when multiple formats are available. Markdown flows through the pipeline with the least loss.
- **Verify before ingesting** — check that fetched content is actual article text (not an error page or login wall) with a quick `head -10` before sending to `cove source add`.

### What to Ingest

Ingest broadly across three categories:

**Domain knowledge** — the theory behind what Covalence does:
- Graph theory, knowledge graphs, hybrid retrieval, GraphRAG
- Subjective Logic, epistemic uncertainty, Dempster-Shafer theory
- Entity resolution, coreference, ontology alignment
- Embedding models, dimensionality reduction, Matryoshka representations
- Information retrieval, ranking, fusion algorithms
- Memory systems, forgetting, consolidation (cognitive science + CS)
- Community detection, graph partitioning, spectral methods

**Software engineering** — how to build well:
- Modular design, composability, separation of concerns
- API design patterns, REST conventions, error handling
- Testing strategies (unit, integration, property-based, fuzzing)
- Refactoring techniques, code smells, design patterns
- Rust-specific idioms, async patterns, trait design, error handling
- Go CLI patterns, command design, user experience
- Web frontend architecture (for the dashboard — see below)
- Database schema design, migration patterns, query optimization

**Process knowledge** — how to improve systems like this:
- Retrieval-augmented generation patterns and anti-patterns
- Knowledge management system design
- Evaluation methodology for retrieval systems
- AI agent architectures, tool use, autonomous operation
- Meta-cognition and reflective AI patterns

Every paper ingested makes the next session smarter. Compound returns.

### Keep the Repo in the Graph

The Covalence codebase itself should be ingested into Covalence. This enables using search to find improvement opportunities, architectural patterns, and code quality issues. After significant changes (new modules, refactors, new crates), re-ingest the affected files so the graph stays current.

Use `make ingest-codebase` for bulk re-ingestion, or `cove source add` for individual files.

### Engineering Discipline

Autonomous sessions should proactively maintain engineering quality:
- **Build the right solution.** Prefer architecturally sound approaches over quick fixes. If the right solution requires more upfront work but produces a better system, do it. Synchronous where async is needed, heuristics where models exist, scripts where APIs should be — these are smells. When you notice one, fix the architecture rather than working around it. The system should get better with every change, not accrue workarounds.
- **Async by default.** Operations that involve external services (LLM calls, embedding, sidecar requests) should be non-blocking. Accept input immediately, enqueue processing, return a handle. The retry queue exists for this — use it. Synchronous pipelines are acceptable only for simple, fast operations.
- **Use feature branches.** Never commit directly to `main`. Always create a new feature branch (e.g., `git checkout -b feature/issue-106`) before starting work.
- **Refactor when you see the need.** If code is duplicated, poorly structured, or violating patterns documented here, fix it. Don't ask — just do it and explain in the commit.
- **Use issues.** Every non-trivial piece of work gets an issue. Reference it in commits. Close it when done. This is how Chris tracks what happened between sessions.
- **Close the loop on issues.** At the end of every session, review which issues were touched. Update them with progress notes, close completed ones, add blockers to deferred ones. Append a final summary to `logs/session.md` so the next agent understands where you left off. An open issue with no recent activity is invisible work.
- **Respect conventions.** You MUST explicitly run `cd engine && cargo fmt` and `cd engine && cargo clippy --workspace` and ensure all tests pass before making any commit.
- **Code Review Protocol.** Before executing a `git push` or merge, you MUST run a code review via `cove llm` (e.g., `cove llm "Review the unpushed commits on my branch for architectural alignment and code quality"`). Default backend is Claude Haiku. If quota is exhausted, fall back to the internal code review agent. You must address any feedback or fixes requested and re-run the review. Once the reviewer has provided a green light, you may push the branch, merge it into `main`, and push `main`.
- **Test in dev first.** Use `make run` for development. Don't touch prod data without explicit approval. Use `make promote` to move verified changes to prod.
- **Keep CI green locally.** Run `make check` before pushing. We don't rely on GitHub Actions for gating — the project isn't released yet — but the local checks are non-negotiable.
- **Modular design.** Prefer small, focused modules. If a file is growing past ~500 lines, consider splitting. If a function does three things, make it three functions.
- **Don't ignore failures.** If a consolidation run fails, a test is flaky, or an ingestion produces warnings — investigate immediately or create an issue. Moving past failures silently compounds debt.
- **Run edge synthesis after bulk ingestion.** New sources create disconnected subgraphs. Run `POST /api/v1/admin/edges/synthesize` with `{"min_cooccurrences": 1}` after ingesting multiple sources to connect them via co-occurrence edges.
- **Clear the search cache after deploys.** `POST /api/v1/admin/cache/clear` — otherwise stale cached results hide quality improvements.
- **Holistic changes.** Every feature change must update all affected artifacts together. Use this checklist before pushing:
  - [ ] Code: implementation complete, `cargo fmt` + `cargo clippy` clean
  - [ ] Tests: new/updated tests covering the change
  - [ ] Spec: relevant spec section updated (or aspirational parts marked as such)
  - [ ] Design docs: ADR if architectural decision, design doc if non-trivial
  - [ ] CLAUDE.md: updated if config, environment, workflow, or conventions change
  - [ ] README: updated if user-visible capability changes
  - [ ] MILESTONES.md: updated if it completes a wave/milestone item
  - [ ] GitHub issues: referenced in commits, updated with progress, closed when done
  - [ ] Ingestion: re-ingest changed specs/docs after deploy so the graph stays current
  A feature that's implemented but not documented creates drift. A spec change without code is aspirational — mark it explicitly.
- **Spec-code-design triangle.** Every feature should be traceable: spec describes the concept, design docs record the decision, code implements it, tests verify it. When changing any vertex, check the other two. Use `/analysis/alignment` to detect drift.
- **Epistemic data lifecycle.** Never automatically delete old source versions, orphan nodes, or duplicates. Old observations aren't false — they're prior state. Use `/admin/data-health` to preview what's stale, then make conscious cleanup decisions.

### Completeness Rule

A feature is NOT done until ALL of these are updated together:
- [ ] Code: implemented, `cargo fmt` + `cargo clippy` clean
- [ ] Tests: covering the change
- [ ] CLAUDE.md: directives reflecting the new pattern
- [ ] Docs: user-facing guide updated if applicable
- [ ] README: feature listed if user-visible
- [ ] Extension manifests: entity types match what code actually emits
- [ ] MCP tools: parity with REST endpoints

If any artifact is stale relative to the code, the feature is incomplete. Do not move on to the next task until all artifacts are synchronized. This prevents the pattern of features existing in code but being invisible to future sessions.

### Track What You Find

Every insight, misalignment, or gap discovered through Covalence queries (or any other means) should be tracked:
- **Misalignment between spec and implementation** → create an issue
- **Research finding that contradicts a design choice** → create an issue
- **Knowledge gap that blocks understanding** → ingest the material, then create an issue if it reveals needed changes
- **Opportunity for improvement** → create an issue
- **Loop friction** — anything that slows down the query→ingest→build cycle → fix it or create an issue

Nothing evaporates. The system improves because we're honest about what needs improving.

### Improving the Loop

The meta loop is subject to its own optimization. Explicitly look for:
- **Friction in the cycle** — if querying is slow, fix search. If ingestion drops quality, fix the pipeline. If the CLI is awkward, improve it.
- **Missing feedback signals** — are we actually measuring whether ingested research improves outcomes? If not, build that measurement.
- **Knowledge decay** — is old research still accurate? Do opinions need updating? Is consolidation surfacing the right articles?
- **Process patterns** — what worked well in this session? What was wasted effort? Update these directives accordingly.

**Known failure modes** (learned from experience):
- **Shallow learning** — skimming 5 papers instead of deeply reading 1. The Learn step gets shortcut because Build feels more productive. It isn't — shallow learning produces shallow improvements.
- **Vibes-based evaluation** — "the search returns results now" is not measurement. Use the eval harness, compare chunk distributions, run before/after queries on a fixed set.
- **Ad hoc gap identification** — searching for topics you already know about only finds gaps you already suspect. Use the graph's diagnostic tools (`/admin/knowledge-gaps`, node degree analysis) to find gaps you don't expect.
- **Orphaned issues** — starting work related to an issue but not updating or closing it. The cycle should end with issue hygiene.
- **Ignoring failures** — consolidation errors, ingestion warnings, clippy hints. If something failed during the loop, investigate it or track it. Don't move on.

The goal is not just to build Covalence but to make each session measurably more effective than the last. The loop is the product as much as the code is.

### The Goal

See `VISION.md` for the full vision. In short: Covalence solves both an academic problem (interconnected GraphRAG quality issues unified by epistemic uncertainty) and a market problem (AI agents need persistent, structured, trustworthy memory). The architecture is sound — it needs to mature through quality, not features.

## Hard Rules

1. **PG is the source of truth.** The petgraph sidecar is a derived, rebuildable cache. If it diverges, PG wins.
2. **Every fact has pristine canonical provenance.** No node or edge exists without a provenance link pointing exactly to the immutable byte offsets in the canonical source text. We do not store noisy chunks. All mutated text (e.g. from fastcoref) MUST be mathematically reverse-projected through the Offset Projection Ledger before being stored in PostgreSQL.
3. **No attention dilution in extraction.** The pipeline relies on a strict Two-Pass LLM extraction model (Statements -> Triples). Do not attempt to merge statement generation and entity extraction into a single prompt.
4. **LLM selection is deliberate.** Use the `ChatBackend` abstraction with `ChainChatBackend` for multi-provider failover. Default chain: Claude Haiku → Copilot → Gemini Flash. Every call records the provider in processing metadata.
5. **No Graph Algorithms in SQL.** Graph traversal strictly goes through the in-memory petgraph sidecar. PostgreSQL Recursive CTEs are forbidden.
6. **Uncertainty ≠ disbelief.** The system uses Subjective Logic opinion tuples (b, d, u, a). "Unknown" is not "50% likely."
7. **Secure by default.** All data defaults to `clearance_level = 0` (local_strict). Promotion to federated requires explicit action.
8. **No synthetic test data.** Tests use real data or clearly-marked fixtures. Never fabricate benchmarks or results.

## Extension System

Covalence is infrastructure, not a fixed solution. Domain-specific functionality is packaged as **extensions** — declarative YAML manifests that add entity types, relationship types, domains, alignment rules, services, and lifecycle hooks without modifying the core engine.

Extensions live in `extensions/` at the repo root. Each extension has an `extension.yaml` manifest. The engine loads all manifests at startup and seeds the database. 5 default extensions ship:
- **core** — MAGMA category primitives and relationship universals
- **code-analysis** — Code entity types (function, struct, module, etc.), structural edges, AST extractor service
- **spec-design** — Spec/design domains, bridge types, alignment rules
- **research** — Research domains, epistemic edges, evidence grouping
- **agent-memory** — Long-term agent memory (store, recall, consolidate, forget)

Manifests support: `domains`, `entity_types` (with `metadata_schema`), `relationship_types`, `view_edges`, `noise_patterns`, `domain_rules`, `domain_groups`, `alignment_rules`, `service` (with `extractor_for`), `hooks`, `config_schema`, and `source_schemas`.

See [ADR-0023](docs/adr/0023-extensions-and-config.md) and [Extension Author Guide](docs/extension-author-guide.md) for the full design.

## Agent Memory

The `agent-memory` extension provides persistent memory for AI agents. Entity types: `memory`, `lesson`, `preference`, `skill`. Three MCP tools (`covalence_memory_store`, `covalence_memory_recall`, `covalence_memory_forget`) and corresponding REST endpoints under `/api/v1/memory`.

CLI: `cove memory store`, `cove memory recall`, `cove memory forget`, `cove memory status`.

Configurable via `config_schema`: `retention_days`, `consolidation_threshold`, `auto_record_ask`, `max_memories_per_agent`.

## Configuration

Figment is the sole config path (`Config::from_env()` removed). Layered loading per ADR-0023:

1. Hardcoded defaults in `RawConfig`
2. `covalence.conf` (instance base, YAML)
3. `covalence.conf.d/*.conf` (fragments, alphabetical, last value wins)
4. Environment variables (`COVALENCE_*`, double-underscore for nesting: `COVALENCE_DATABASE__URL`)

See `covalence.conf.example` for all options. Legacy `.env` / `dotenvy` still loaded before figment so `DATABASE_URL` works.

## Code Rules

- Doc comments (`///` or `//!`) on every public item
- Typed errors via `thiserror` in library code (`covalence-core`). `anyhow` only in binary crates (`covalence-api`, `covalence-migrations`).
- sqlx runtime string queries (not compile-time macros). `SQLX_OFFLINE=true` enables unit tests without a live DB.
- Newtypes for domain IDs: `NodeId(Uuid)`, `EdgeId(Uuid)`, `SourceId(Uuid)`, etc.
- No `unwrap()` or `expect()` in library code. Use `?` or explicit error handling.
- Line length: 100 characters (configured in `rustfmt.toml`)
- Edition: 2024

## Anti-Patterns

- **No raw PG connections.** Always use the sqlx pool.
- **No computed/derived state stored in PG.** Topological confidence, PageRank, communities are computed by the sidecar or at query time.
- **No circular crate dependencies.** `covalence-api` depends on `covalence-core`, never the reverse.
- **No graph algorithms in SQL.** Graph traversal strictly goes through the in-memory petgraph sidecar. PostgreSQL Recursive CTEs are too inefficient for scale.
- **No hardcoded embedding dimensions.** Per-table dimensions are configured via `COVALENCE_EMBED_DIM_SOURCE` (default 2048), `COVALENCE_EMBED_DIM_CHUNK` (default 1024), `COVALENCE_EMBED_DIM_NODE` (default 256), etc. Embeddings are generated at max dimension and truncated + renormalized per table (matryoshka property).
- **No raw embedding storage without validation.** Always use `truncate_and_validate()` (from `ingestion::embedder`) before storing embeddings. Never call `truncate_embedding()` directly at storage boundaries — `truncate_and_validate` wraps it with a dimension check that catches mismatches before they reach PostgreSQL. This is the single gatekeeper for dimension consistency.
- **No conflation of UUID with NodeIndex.** UUIDs are PG identifiers. NodeIndex is petgraph-internal. The `index: HashMap<Uuid, NodeIndex>` map bridges them.

## Patterns to Follow

These patterns come from the existing Covalence and should be maintained:

- **Service layer per domain** — Each domain (sources, nodes, search, ingestion) has a service struct that owns business logic.
- **Thin handlers** — Axum handlers extract params, call the service, format the response. No logic in handlers.
- **utoipa for OpenAPI** — Derive `ToSchema` on response/request types, `#[utoipa::path]` on handlers.
- **Cobra CLI with global flags** — `--api-url` and `--json` are global. Subcommands: `source`, `search`, `node`, `admin`, `graph`, `audit`, `memory`, `ask`, `llm`.
- **Layered figment config** — `Config::from_figment()` loads `covalence.conf` + `conf.d/` fragments + `COVALENCE_*` env vars. Legacy `.env` still supported via `dotenvy`.
- **Embedding dimension discipline** — Embeddings flow through a consistent pipeline: (1) embedder generates at max dimension (e.g., 2048), (2) `truncate_and_validate()` truncates + L2-renormalizes to the target per-table dimension, (3) validated vector is stored. All storage call sites (`source.rs`, `pg_resolver.rs`) and search queries (`vector.rs`) must go through `truncate_and_validate`. When adding new embedding storage paths, always validate dimensions before the INSERT/UPDATE.
- **Run migrations after schema changes** — After adding new migrations, run `make migrate` (or `make reset-db` for a clean slate). The DB schema must match what the code expects — dimension mismatches between column definitions and stored vectors cause silent failures.
- **Validate sidecars at startup** — Every HTTP sidecar integration (fastcoref, PDF converter, future extractors) must include a `validate()` method that sends a test request and verifies the response format. Call `validate()` at engine startup. If it fails, disable the backend with an ERROR log — never silently degrade. See Lesson 20 in spec/10.

## Testing

```bash
# Unit tests (no DB required, uses SQLX_OFFLINE=true)
cd engine && cargo test --workspace
# Current: 1,574 passing tests (1,491 core + 21 api + 13 ast-extractor + 49 eval), 18 ignored integration tests

# Integration tests (requires running dev PG — see .env)
cd engine && cargo test --workspace -- --ignored

# Clippy
cd engine && cargo clippy --workspace -- -D warnings

# Format check
cd engine && cargo fmt --all -- --check

# Full check
make check

# CLI
cd cli && go test ./...
```

## Database

```bash
make dev-db       # Start dev PostgreSQL container
make migrate      # Run migrations on dev
make reset-db     # Drop + recreate dev DB
make promote      # check + migrate-prod + deploy
```

Extensions required: `pgvector`, `pg_trgm`, `ltree`

Connection details are environment-specific — see `.env` and `.env.example`.

## Spec References

Design specs in `spec/`:
- `01-architecture.md` — Three-layer design, theoretical foundations, data flow
- `02-data-model.md` — Entity model, hybrid property graph + provenance view
- `03-storage.md` — PG schema, indexes, migrations, stored procedures
- `04-graph.md` — petgraph sidecar, algorithms (PageRank, TrustRank, community detection)
- `05-ingestion.md` — 9-stage pipeline, source update classes, three-timescale consolidation
- `06-search.md` — 6 search dimensions, RRF fusion, query strategies
- `07-epistemic-model.md` — Subjective Logic, confidence propagation, forgetting (BMR)
- `08-api.md` — HTTP endpoints, MCP tools, error responses
- `09-federation.md` — Clearance levels, egress filtering, ZK edges, federation protocol
- `10-lessons-learned.md` — Implementation lessons and design trade-offs
- `11-evaluation.md` — Evaluation harness, fixture-based testing, metrics
- `12-code-ingestion.md` — AST-aware code ingestion, Tree-sitter chunking, semantic summary wrapper, Component bridge layer
- `13-cross-domain-analysis.md` — Erosion detection, coverage analysis, blast radius, whitespace roadmap, dialectical critique

## ADR Process

Architecture Decision Records live in `docs/adr/` (23 ADRs). Use the template at `docs/adr/0000-template.md`.

To add a new ADR:
1. Copy the template
2. Number sequentially (next available number)
3. Fill in Context, Decision, Consequences, Alternatives
4. Set Status to "Accepted"

## Issue Tracking

All work is tracked via GitHub issues. This is mandatory — not bureaucracy, but integrity. Issues are how we stay honest about what needs doing.

### When to Create Issues

- **Always create an issue** for: new features, bug fixes, refactoring, infrastructure changes, process changes, research ingestion tasks, spec-implementation misalignments, knowledge gaps.
- **Fix inline without an issue** only for: typo fixes, formatting, trivial one-line changes that don't affect behavior.
- **If you discover something** while working on something else — a bug, a misalignment, a gap, an opportunity — create a new issue for it. Decide whether to fix it now (if quick and non-disruptive) or defer. Either way, it's tracked.
- **If a Covalence query reveals a problem** (e.g., the spec says X but the code does Y), that's an issue.
- **If research contradicts a design choice**, that's an issue.

### Issue Workflow

1. Create the issue with a clear title, context, and task checklist.
2. Reference the issue number in commit messages (e.g., `Fix source deletion cascade (#81)`).
3. Close the issue when the work is complete and verified.
4. If work is blocked or deferred, add a comment explaining why and leave it open.

### Labels

Use existing labels: `enhancement`, `bug`, `future`, `deferred`, `spec`. Create new labels only when a clear category is needed.

### Commits

Reference issue numbers in commit messages. Format: `<verb> <what> (#<issue>)`.

## Web Dashboard

Covalence should have a web interface for observability and eventually interaction. This lives in a `dashboard/` directory at the repo root.

### Phase 1 — Stats & Observability (largely complete)
A read-only dashboard showing:
- Knowledge graph stats (sources, nodes, edges, chunks, articles)
- Search dimension health and recent query performance
- Ingestion pipeline status and recent activity
- Graph topology visualization (communities, connectivity)
- Epistemic health (confidence distributions, opinion coverage)
- Prometheus metrics via `GET /metrics` (counters, histograms for search, queue, LLM, cache)

### Phase 2 — Interaction (future)
- Search interface with strategy selection and result exploration
- Source browser with chunk/provenance drill-down
- Node/edge explorer with graph neighborhood visualization

### Phase 3 — Configuration (future)
- Environment management (dev/prod switching)
- Embedding provider configuration
- Consolidation scheduling
- Ingestion pipeline tuning

The dashboard is served by the existing Axum engine (alongside the API and Swagger UI). Keep it simple — static assets or a lightweight frontend framework. The API already exposes everything the dashboard needs.

## Milestones

See `MILESTONES.md` for the phased roadmap (M0–M11) and post-milestone waves.
Current phase: **M0-M11 + Waves 1–26 complete.** 1,574 tests passing (1,491 core + 21 api + 13 ast-extractor + 49 eval). See `MILESTONES.md` for the full wave history. Recent waves: architecture evolution (multi-binary split, 67 SPs, per-kind concurrency, source adapters, config management, WebUI dashboard, codebase cleanup, lifecycle hooks, SSE streaming, sessions, STDIO sidecars, Prometheus metrics, input validation, extensions, agent memory).
