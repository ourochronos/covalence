# Covalence

A hybrid GraphRAG knowledge engine. Ingests unstructured sources, builds a property graph with epistemic annotations (Subjective Logic, causal hierarchy, provenance), and provides multi-dimensional fused search via Reciprocal Rank Fusion.

## Features

- **6-dimension search fusion** — vector, lexical, temporal, graph, structural, and global dimensions fused via RRF with SkewRoute adaptive strategy selection
- **Statement-first ingestion** — two-pass LLM extraction (statements then triples) with fastcoref coreference resolution and offset projection. PDF, HTML, Markdown, and code via pluggable converter sidecars
- **Epistemic model** — Subjective Logic opinions, Dempster-Shafer fusion, DF-QuAD argumentation, Bayesian Model Reduction forgetting
- **5-tier entity resolution** — exact, alias, vector cosine, fuzzy trigram, HDBSCAN batch clustering
- **Graph type system** — entity classification (entity_class), domain labels (project/domain on sources), traceability edges (ADR-0018)
- **Apache AGE graph backend** — config-driven graph engine selection (petgraph or AGE) via GraphEngine trait
- **Async pipeline with retry queue** — per-entity jobs, fan-in triggers, watchdog, persistent error classification (permanent/rate-limit/transient)
- **Semantic code summaries** — per-method extraction from impl blocks, definition-pattern chunk matching, bottom-up file summary composition
- **Cross-domain alignment analysis** — coverage analysis, architecture erosion detection, blast-radius simulation, whitespace roadmap, dialectical critique
- **`/ask` endpoint with SSE streaming** — grounded Q&A with citations, per-request model override, ChainChatBackend multi-provider failover. `POST /ask/stream` returns Server-Sent Events (context, tokens, done)
- **Lifecycle hooks** — external HTTP hooks at pre_search, post_search, and post_synthesis pipeline points. Fail-open by default, concurrent execution, global or domain-scoped via adapter binding
- **Session/conversation primitives** — lightweight session + turn model for multi-turn `/ask` conversations. Conversation history injected into LLM context automatically
- **STDIO sidecar contract** — JSON-in/JSON-out stateless transforms alongside HTTP sidecars. SidecarRegistry manages named transports with startup validation
- **Prometheus metrics** — `GET /metrics` endpoint with counters and histograms for search, queue, LLM calls, and cache
- **MCP server for Claude Code** — 7 tools bridging Claude Code sessions to the Covalence API
- **Data health monitoring** — `/admin/data-health` endpoint, source supersession tracking
- **Input validation** — validator crate on all request DTOs with bounds checking
- **Provider attribution** — ChatResponse tracks which LLM provider answered each request
- **Incremental ingestion on deploy** — changed files auto-ingested via `make deploy`

### Quality Gates

| Metric | Gate | Current |
|--------|------|---------|
| Search precision@5 | >0.80 | 0.86 |
| Entity precision | >90% | 96% |
| Tests passing | — | 1,452 (1,382 core + 21 api + 49 eval) |

## Architecture

Three layers:

- **Storage** — PostgreSQL 17 + pgvector + Apache AGE. Single source of truth for all data.
- **Engine** — Rust (Axum + petgraph/AGE). Search fusion, graph sidecar, ingestion pipeline, consolidation, epistemic model.
- **API** — HTTP REST + MCP. Thin routing, OpenAPI via utoipa, Swagger UI at `/docs`.

See `spec/` for 14 design specifications, `docs/adr/` for 22 architectural decision records.

## Quick Start

### Prerequisites

- Rust 1.85+ (edition 2024)
- Go 1.22+
- PostgreSQL 17 with pgvector, pg_trgm, ltree extensions
- Docker (optional, for dev database)

### Setup

```bash
cp .env.example .env
# Edit .env with your database credentials

# Start dev database
make dev-db

# Run migrations
make migrate

# Build and run
make run
```

### CLI

```bash
make cli-install
cove search "your query"
cove source add --type document path/to/file.pdf
cove node list --type person
cove ask "How does entity resolution work?"
cove llm "Review this code for quality"
```

## Project Structure

```
engine/                    Rust workspace
  crates/
    covalence-core/        Library: models, storage, graph, search, ingestion, epistemic
    covalence-api/         Binary: Axum server, OpenAPI, routes
    covalence-migrations/  Binary: sqlx migration runner
    covalence-eval/        Binary: layer-by-layer evaluation harness
    covalence-worker/      Binary: async queue worker (per-kind concurrency)
cli/                       Go CLI (Cobra) — binary name: cove
mcp-server/                MCP server for Claude Code integration (Node.js)
dashboard/                 Web dashboard (stats, observability)
spec/                      Design specifications (14 specs)
docs/adr/                  Architecture Decision Records (22 ADRs)
```

## Development

```bash
cp .env.example .env    # configure database URL, API keys, sidecars
make dev-db             # start dev PostgreSQL (Docker)
make migrate            # run migrations
make run                # start engine

make check              # fmt + clippy + tests
make test               # unit tests only
make lint               # clippy only
```

### Deployment

Configure deployment targets in your Makefile or environment. The engine is a standard Rust binary managed by systemd or equivalent.

```bash
make promote  # check + migrate-prod + deploy (full pipeline)
make deploy   # pull, build, migrate, restart on remote host
```

## MCP Server

The MCP server at `mcp-server/index.js` bridges Claude Code sessions to the Covalence API. It provides 7 tools:

| Tool | Description |
|------|-------------|
| `covalence_search` | Multi-dimensional fused search across the knowledge graph |
| `covalence_ask` | Grounded Q&A with citations over the knowledge graph |
| `covalence_health` | Engine health check and system status |
| `covalence_alignment` | Cross-domain alignment analysis (coverage, erosion) |
| `covalence_node` | Node detail lookup with epistemic explanation |
| `covalence_blast_radius` | Blast-radius simulation from any node |
| `covalence_data_health` | Data quality metrics and source health |

Configure in Claude Code's MCP settings to enable Covalence-aware development sessions.

## Links

- [Milestones](MILESTONES.md)
- [Vision](VISION.md)
- [Architecture Decisions](docs/adr/)
- [Specifications](spec/)
- [Provider Docs](docs/providers.md)
