# ADR-0003: In-Memory Graph Sidecar via petgraph

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/04-graph.md

## Context

Graph algorithms (PageRank, community detection, traversal, shortest path) are too slow to run via SQL recursive CTEs on every query. The system needs fast graph compute.

## Decision

Maintain an in-memory `DiGraph<NodeMeta, EdgeMeta>` via petgraph, embedded as a library in the engine process. Sync from PG via outbox pattern + LISTEN/NOTIFY + 5s polling fallback.

## Consequences

### Positive

- Sub-millisecond traversal and algorithm execution
- petgraph is mature, well-tested, and supports filtered views
- Outbox pattern provides reliable sync without 8KB NOTIFY payload limit
- Embedded sidecar avoids network overhead

### Negative

- Memory usage grows with graph size (~2.3GB for 10M nodes + 50M edges)
- Eventually consistent (target <1s, worst case 5s)
- Single process limits horizontal scaling (extract to separate process when needed)
- petgraph has poor dynamic update performance; mitigated by batched outbox sync

## Alternatives Considered

- **Pure SQL (recursive CTEs):** Too slow for interactive graph queries
- **Neo4j sidecar:** Operational complexity, sync overhead, license considerations
- **Separate process via gRPC:** Network overhead for v1; consider when graph ops become bottleneck
