# Contributing to Covalence

## Prerequisites

- **Rust 1.85+** (edition 2024)
- **Go 1.22+** (for the CLI)
- **PostgreSQL 17** with extensions: `pgvector`, `pg_trgm`, `ltree`
- **Docker** (for the dev database container)

## Development Setup

```bash
# Clone and configure
git clone https://github.com/ourochronos/covalence.git
cd covalence
cp .env.example .env        # edit with your credentials
cp covalence.conf.example covalence.conf

# Start dev database and run migrations
make dev-db
make migrate

# Start the engine
make run
```

The dev database runs on port 5435 (Docker). The engine listens on port 8431.

## Quality Gates

Run `make check` before every commit. It executes:

1. `cargo fmt --all -- --check` -- formatting
2. `cargo clippy --workspace -- -D warnings` -- lints
3. `cargo test --workspace` -- unit tests

All three must pass. No exceptions.

## Branch Conventions

- **Never commit directly to `main`.** Always create a feature branch.
- Branch naming: `feature/<description>` or `fix/<description>`.

```bash
git checkout -b feature/add-pdf-sidecar
# ... make changes ...
make check
git push -u origin feature/add-pdf-sidecar
```

## Commit Messages

Format: `<verb> <what> (#<issue>)` when referencing an issue.

```
Add PDF sidecar validation (#125)
Fix vector dimension mismatch in chunk storage
Refactor search service into strategy modules
```

Use active voice. Keep the first line under 72 characters.

## Pull Request Process

1. Create a feature branch off `main`.
2. Make your changes. Run `make check` until clean.
3. Push and open a PR.
4. PR description should explain **why**, not just **what**.
5. Address review feedback, then merge.

## Code Style

- **Edition 2024**, line width 100 (configured in `rustfmt.toml`).
- **`thiserror`** for errors in library code (`covalence-core`).
- **`anyhow`** for errors in binary crates only (`covalence-api`, `covalence-migrations`, etc.).
- **No `unwrap()` or `expect()`** in library code. Use `?` or explicit error handling.
- **Doc comments** (`///` or `//!`) on every public item.
- **Newtype IDs** for domain identifiers: `NodeId(Uuid)`, `EdgeId(Uuid)`, etc.
- **Typed errors** via `thiserror` with the `Error` enum in `covalence-core`.

## Extension Development

Covalence is extensible via declarative YAML manifests. If you're adding domain-specific functionality (new entity types, relationship types, domains, or services), package it as an extension rather than modifying the core engine.

See [docs/extension-author-guide.md](docs/extension-author-guide.md) for the full guide.

## Testing

```bash
make test          # unit tests (no DB required)
make lint          # clippy only
make check         # fmt + clippy + tests (the full gate)
cd cli && go test ./...  # CLI tests
```

Integration tests (require a running dev database) are marked `#[ignore]` and run with:

```bash
cd engine && cargo test --workspace -- --ignored
```

## Project Structure

See the [README](README.md) for the workspace layout, architecture overview, and CLI usage.
