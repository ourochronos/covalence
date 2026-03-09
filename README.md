# Covalence

A hybrid GraphRAG knowledge engine. Ingests unstructured sources, builds a property graph with epistemic annotations, and provides multi-dimensional fused search.

> **Development name:** `graphrag`. Will be renamed to `covalence` when ready to replace the existing system.

## Architecture

Three layers:

- **Storage** — PostgreSQL 17 + pgvector. Single source of truth for all data.
- **Engine** — Rust (Axum + petgraph). Search fusion, graph sidecar, ingestion pipeline, consolidation.
- **API** — HTTP REST + MCP. Thin routing, OpenAPI via utoipa.

See `spec/` for detailed design documents, `docs/adr/` for architectural decisions.

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
```

## Project Structure

```
engine/                    Rust workspace
  crates/
    covalence-core/        Library: models, storage, graph, search, ingestion, epistemic
    covalence-api/         Binary: Axum server, OpenAPI, routes
    covalence-migrations/  Binary: sqlx migration runner
cli/                       Go CLI (Cobra)
spec/                      Design specifications
docs/adr/                  Architecture Decision Records
```

## Development

```bash
make check    # fmt + clippy + tests
make test     # unit tests
make lint     # clippy
```

## Coexistence

This instance runs on different ports from the existing Covalence:

| Resource | This (new) | Existing |
|----------|-----------|----------|
| PG port  | 5435      | 5434     |
| API port | 8431      | 8430     |
| CLI      | `cove`    | `cov`    |

## Links

- [Milestones](MILESTONES.md)
- [Architecture Decisions](docs/adr/)
- [Specifications](spec/)
