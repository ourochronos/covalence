# ADR-0004: Outbox Pattern for Graph Sync

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/04-graph.md, spec/03-storage.md

## Context

The petgraph sidecar needs to stay in sync with PG. LISTEN/NOTIFY has an 8KB payload limit, making it unreliable for large batch operations. WAL-based replication is operationally complex.

## Decision

Use the Outbox Pattern: PG triggers write change records to an `outbox_events` table on node/edge changes. NOTIFY sends an empty "ping" to wake the sidecar, which then queries the outbox for changes since its last processed sequence ID. A 5-second polling fallback ensures changes are never missed.

## Consequences

### Positive

- No payload size limits (outbox rows can be arbitrarily large)
- Reliable: polling fallback covers missed NOTIFYs
- Auditable: outbox provides a change log
- Simple to implement and debug

### Negative

- Additional table and triggers
- Hourly pruning job needed (delete events older than 24 hours)
- Slightly higher latency than direct NOTIFY with payload

## Alternatives Considered

- **LISTEN/NOTIFY with payload:** 8KB limit makes it unreliable for batch operations
- **WAL logical replication:** Most robust but operationally complex, hard to filter
- **Simple polling:** Higher latency (5s minimum), unnecessary DB load
