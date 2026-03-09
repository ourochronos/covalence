# Covalence — Project Instructions

## Overview

Covalence is a hybrid GraphRAG knowledge engine replacing the existing `ourochronos/covalence` repo. It ingests unstructured sources, builds a property graph with rich epistemic annotations (Subjective Logic, causal hierarchy, provenance), and provides multi-dimensional fused search via Reciprocal Rank Fusion.

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
  src/ingestion/                    9-stage pipeline: accept, convert, parse, normalize, chunk, embed, extract, landscape, resolve
  src/consolidation/                Batch/deep consolidation traits + scheduler + ontology clustering
  src/services/                     Service layer (source, node, edge, article, admin, search)
  src/config.rs                     Environment-driven configuration
  src/error.rs                      Typed errors via thiserror
engine/crates/covalence-api/        Binary crate — Axum server, utoipa OpenAPI
engine/crates/covalence-migrations/ Binary crate — sqlx migration runner
engine/crates/covalence-eval/       Binary crate — layer-by-layer evaluation harness
cli/                                Go CLI (Cobra) — binary name: cove
  cmd/                              Subcommands: source, search, node, admin
  internal/                         HTTP client + output helpers
spec/                               Design specs (read-only reference)
docs/adr/                           Architecture Decision Records
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
| dotenvy | Environment-driven configuration |
| tokio | Async runtime |
| sha2 | SHA-256 content hashing for dedup |
| unicode-normalization | Unicode NFC normalization in ingestion |
| async-trait | Async trait support for Embedder, Extractor, etc. |
| chrono | Timestamp handling |
| tracing / tracing-subscriber | Structured logging |
| reqwest | HTTP client for OpenAI/Voyage API (embedder + extractor) |
| futures | Concurrent extraction via join_all |
| cobra (Go) | CLI framework |

## Ports & Coexistence

| Resource | This Instance | Existing Covalence |
|----------|--------------|-------------------|
| PG port | **5435** | 5434 |
| Engine port | **8431** | 8430 |
| CLI binary | **`cove`** | `cov` |

These must not conflict. The existing Covalence instance runs in production.

## Hard Rules

1. **PG is the source of truth.** The petgraph sidecar is a derived, rebuildable cache. If it diverges, PG wins.
2. **Every fact has provenance.** No node or edge exists without a provenance link (extraction → chunk → source).
3. **LLM calls are isolated.** LLMs are used only for ingestion extraction, batch compilation, and optional query synthesis. Search and graph operations never require an LLM.
4. **Uncertainty ≠ disbelief.** The system uses Subjective Logic opinion tuples (b, d, u, a). "Unknown" is not "50% likely."
5. **Secure by default.** All data defaults to `clearance_level = 0` (local_strict). Promotion to federated requires explicit action.
6. **No synthetic test data.** Tests use real data or clearly-marked fixtures. Never fabricate benchmarks or results.

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
- **No graph algorithms in SQL.** Graph traversal goes through petgraph. PG has a `graph_traverse()` fallback only for when the sidecar is unavailable.
- **No hardcoded embedding dimensions.** Dimensions are configured via `COVALENCE_EMBED_DIM` (default 1024) and `COVALENCE_NODE_EMBED_DIM` (default 256), not scattered as magic numbers.
- **No conflation of UUID with NodeIndex.** UUIDs are PG identifiers. NodeIndex is petgraph-internal. The `index: HashMap<Uuid, NodeIndex>` map bridges them.

## Patterns to Follow

These patterns come from the existing Covalence and should be maintained:

- **Service layer per domain** — Each domain (sources, nodes, search, ingestion) has a service struct that owns business logic.
- **Thin handlers** — Axum handlers extract params, call the service, format the response. No logic in handlers.
- **utoipa for OpenAPI** — Derive `ToSchema` on response/request types, `#[utoipa::path]` on handlers.
- **Cobra CLI with global flags** — `--api-url` and `--json` are global. Subcommands: `source`, `search`, `node`, `admin`.
- **Environment-driven config** — `dotenvy` loads `.env`, config struct reads from env vars with defaults.

## Testing

```bash
# Unit tests (no DB required, uses SQLX_OFFLINE=true)
cd engine && cargo test --workspace
# Current: 406 passing tests (378 core + 28 eval), 11 ignored integration tests

# Integration tests (requires running PG on port 5435)
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
# Create dev database
make dev-db

# Run migrations
make migrate

# PG connection
psql postgres://covalence:covalence@localhost:5435/covalence_dev
```

Extensions required: `pgvector`, `pg_trgm`, `ltree`

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

## ADR Process

Architecture Decision Records live in `docs/adr/`. Use the template at `docs/adr/0000-template.md`.

To add a new ADR:
1. Copy the template
2. Number sequentially (next available number)
3. Fill in Context, Decision, Consequences, Alternatives
4. Set Status to "Accepted"

## Milestones

See `MILESTONES.md` for the phased roadmap (M0–M11) and post-milestone waves.
Current phase: **M0-M11 + Waves 1–4 complete.** 406 tests passing. Post-milestone waves delivered: vector resolution (#9), ontology clustering (#12), GLiNER2 extractor (#5), format converters (#6), embedding dimension fix (#13), search dimension fix (#14). See GitHub issues for ongoing enhancements.
