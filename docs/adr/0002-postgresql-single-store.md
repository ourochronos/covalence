# ADR-0002: PostgreSQL as Single Persistent Store

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/03-storage.md

## Context

The system needs persistent storage for graph data, embeddings, full-text search, and provenance. Options range from a single database to a polyglot persistence architecture (separate vector DB, graph DB, document store).

## Decision

Use PostgreSQL 17 with pgvector, pg_trgm, and ltree extensions as the sole persistent store. All data — graph, chunks, embeddings, provenance — lives in PG.

## Consequences

### Positive

- Single operational target (backup, monitoring, scaling)
- Transactional guarantees across all data types
- pgvector provides HNSW indexes competitive with dedicated vector DBs
- pg_trgm enables fuzzy text matching
- ltree supports hierarchical queries on chunk structure

### Negative

- pgvector may not match dedicated vector DBs at extreme scale (>100M vectors)
- No native graph query language (Cypher, SPARQL)
- Raw content storage in PG may not scale past ~100GB (plan: migrate to object storage)

## Alternatives Considered

- **Polyglot (PG + Milvus/Qdrant + Neo4j):** Better per-domain performance but massive operational complexity
- **SQLite for dev, PG for prod:** Schema drift risk, different query behaviors
