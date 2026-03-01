# Covalence

**Graph-native knowledge substrate for AI agent persistent memory.**

Rust engine · Go CLI · PostgreSQL (AGE + PGVector + pg_textsearch) · REST/OpenAPI

---

## What is Covalence?

Covalence is a persistent knowledge management system designed for AI agents. It provides a graph-native substrate where knowledge is stored as rich nodes with typed edges, enabling multi-dimensional retrieval across graph structure, semantic similarity, and lexical matching — all within a single PostgreSQL instance.

## Architecture

```
┌─────────────────────────────────────────────┐
│              OpenClaw Plugin (TS)            │
├─────────────────────────────────────────────┤
│              Go CLI (thin REST client)       │
├─────────────────────────────────────────────┤
│              REST API (OpenAPI spec)         │
├─────────────────────────────────────────────┤
│              Rust Engine                     │
│  ┌─────────┬──────────┬───────────────────┐ │
│  │  Graph   │ Semantic │ Lexical           │ │
│  │  (AGE)   │(PGVector)│(pg_textsearch/FTS)│ │
│  └─────────┴──────────┴───────────────────┘ │
│              Score Fusion (RRF)             │
├─────────────────────────────────────────────┤
│              PostgreSQL 17                   │
└─────────────────────────────────────────────┘
```

## Key Design Principles

- **Rich nodes + typed edges** — not triples. Articles are documents with metadata, connected by a growing vocabulary of relationship types.
- **Graph for storage, flat list for retrieval, graph-walk for investigation** — search returns chain tips; the graph is there for provenance and exploration.
- **Dual-stream architecture** — fast algorithmic path for retrieval/indexing, slow inference path for compilation/edge inference.
- **Clean abstractions** — the graph backend (AGE today, SQL/PGQ tomorrow) is a pluggable implementation detail behind a trait boundary.
- **16GB is enough** — designed for M4 Mac Mini. No profligate memory use.

## Status

**Phase Zero** — spec complete, research done, implementation not yet started.

## Documentation

- [Phase Zero Spec](docs/phase-zero/SPEC.md) — the full specification
- [Research](docs/research/) — literature review, extension ecosystem, SING translation guide, current substrate audit

## License

TBD
