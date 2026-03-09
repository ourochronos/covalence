# ADR-0014: Cargo Workspace with Three Crates

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/01-architecture.md

## Context

The Rust engine needs to be organized into crates. Following the existing Covalence pattern, the engine lives in a subdirectory with a workspace structure.

## Decision

Three crates in a Cargo workspace under `engine/`:

- **covalence-core** (library): All domain logic — models, storage traits and impls, graph algorithms, search, ingestion, epistemic model, consolidation.
- **covalence-api** (binary): Axum HTTP server, utoipa OpenAPI, Swagger UI, thin route handlers. Depends on covalence-core.
- **covalence-migrations** (binary): sqlx migration runner. Minimal dependencies.

## Consequences

### Positive

- Clean separation: library logic is testable without HTTP framework
- API crate is thin (handlers + routing only), following existing Covalence pattern
- Migrations are independent, can run without the full engine
- No circular dependencies (api → core, migrations → sqlx only)

### Negative

- Three Cargo.toml files to maintain
- Cross-crate changes require careful dependency management
- Compilation time slightly higher than single crate (but parallel builds help)

## Alternatives Considered

- **Single crate:** Simpler but mixes concerns, harder to test library logic independently
- **Many small crates (per domain):** Too many crates for the current team size, coordination overhead
- **Two crates (lib + bin):** Migrations bundled with either lib or bin awkwardly
