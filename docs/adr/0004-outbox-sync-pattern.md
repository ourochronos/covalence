# ADR-0004: Outbox Pattern for Graph Sync

**Status:** Partially Implemented (polling fallback only)

**Date:** 2026-03-07

**Updated:** 2026-04-08

**Spec Reference:** spec/04-graph.md, spec/03-storage.md

## Context

The petgraph sidecar needs to stay in sync with PG. LISTEN/NOTIFY has an 8KB payload limit, making it unreliable for large batch operations. WAL-based replication is operationally complex.

The motivating concrete failure (issue #177): the engine and worker are separate processes, each holding their own in-memory sidecar. When the worker commits nodes/edges (e.g. async edge synthesis, background extraction), the engine's sidecar never sees them until the engine restarts. This produced silent search/answer drift between processes.

## Decision

The original target design is the Outbox Pattern: PG triggers write change records to an `outbox_events` table on node/edge changes. NOTIFY sends an empty "ping" to wake the sidecar, which then queries the outbox for changes since its last processed sequence ID. A 5-second polling fallback ensures changes are never missed.

## Implementation Status (2026-04-08)

Only the **polling fallback** is implemented today. The outbox table, triggers, sequence-id cursor, and LISTEN/NOTIFY ping path do not exist yet.

What ships:

- `covalence-api` spawns a tokio task at startup that periodically calls `GraphEngine::reload(pool)` against PG. This is a full sidecar rebuild, not a delta — it is correct but not efficient.
- The interval is configured by `COVALENCE_GRAPH_RELOAD_INTERVAL_SECS` (default `30`). Setting it to `0` disables the task. See `engine/crates/covalence-api/src/main.rs::spawn_graph_reload_task`.
- The first tick is skipped because `AppState::new` already loaded the sidecar synchronously at startup.

This closes the immediate cross-process drift gap (#177) at the cost of periodic full reloads. The outbox + NOTIFY delta path is still the desired endgame; it should be revisited once reload cost becomes a measurable problem (large graphs, frequent commits).

## Consequences

### Positive

- No payload size limits (outbox rows can be arbitrarily large)
- Reliable: polling fallback covers missed NOTIFYs
- Auditable: outbox provides a change log
- Simple to implement and debug
- **Polling-only mode (current state) is trivially correct** — every reload is a fresh build from PG, so no cursor bookkeeping or trigger drift to worry about.

### Negative

- Additional table and triggers (not yet implemented)
- Hourly pruning job needed (delete events older than 24 hours) (not yet implemented)
- Slightly higher latency than direct NOTIFY with payload
- **Polling-only mode** rebuilds the entire sidecar on every tick, which is wasteful for large graphs. Cost scales linearly with graph size and inversely with the configured interval.

## Alternatives Considered

- **LISTEN/NOTIFY with payload:** 8KB limit makes it unreliable for batch operations
- **WAL logical replication:** Most robust but operationally complex, hard to filter
- **Simple polling:** Higher latency (5s minimum), unnecessary DB load — *this is what we shipped first as the MVP, see Implementation Status*
