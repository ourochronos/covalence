# Design: Graph Sidecar

## Status: implemented

## Spec Sections: 04-graph.md, 06-search.md

## Architecture Overview

The graph sidecar is an in-memory petgraph `StableDiGraph<NodeMeta, EdgeMeta>` that mirrors the PostgreSQL graph data. It enables O(1) graph traversals for search without database round-trips. Protected by `Arc<RwLock>`, loaded on startup from PG, and synced via outbox events.

## Implemented Components

### Fully Implemented ✅

| Component | File | Notes |
|-----------|------|-------|
| **Core sidecar** | `graph/sidecar.rs` | `StableDiGraph` with UUID→NodeIndex lookup via HashMap |
| **PageRank** | `graph/algorithms.rs` | Standard iterative PageRank with configurable damping (default 0.85) |
| **PersonalizedPageRank** | `graph/algorithms.rs` | Seed-biased PageRank for topic-focused ranking |
| **TrustRank** | `graph/algorithms.rs` | Anti-spam trust propagation from operator-defined seed set |
| **Spreading activation** | `graph/algorithms.rs` | Query-seeded activation with hop decay for graph search |
| **Structural importance** | `graph/algorithms.rs` | Combined centrality metric for node ranking |
| **Topological confidence** | `graph/algorithms.rs` | Graph-structure-derived confidence for epistemic composite scoring |
| **Community detection** | `graph/community.rs` | Leiden-style community detection with modularity optimization |
| **K-core decomposition** | `graph/community.rs` | `compute_core_numbers()` for structural shell analysis |
| **Landmark detection** | `graph/community.rs` | Identifies high-centrality nodes as navigation landmarks |
| **Bridge discovery** | `graph/bridges.rs` | Finds structural bridges between graph components |
| **Filtered views** | `graph/filtered.rs` | Clearance-level and edge-type filtered graph views |
| **Graph traversal** | `graph/traversal.rs` | Path finding and neighborhood expansion |
| **PG sync** | `graph/sync.rs` | Full reload from PG on startup |
| **Thread safety** | `graph/sidecar.rs` | `Arc<RwLock>` (tokio) for concurrent read access |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **Incremental sync** | Full reload only | No outbox-driven incremental updates — requires full restart to pick up new ingestions |
| **TrustRank seed selection** | Algorithm exists | No mechanism to designate trusted seed nodes via API |
| **Community membership in search** | Communities computed | Not exposed to search dimension weighting |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **Outbox-driven incremental sync** | Spec 04: "Graph Sidecar Sync", outbox_events | High — currently requires restart |
| **PG LISTEN/NOTIFY** | Spec 04: "NOTIFY ping" for real-time sync | Medium — alternative to outbox polling |
| **WAL-based sync** | Spec 04: "WAL-based" replication | Low — complex, outbox simpler |
| **Node2Vec embeddings** | Spec 04: graph-aware structural embeddings | Low — requires training pipeline |
| **Spectral community detection** | Spec 04: spectral methods | Low — Leiden sufficient for now |
| **Zero-copy filtered views** | Spec 04: "zero-copy" clearance filtering | Low — current filtered views allocate |

## Key Design Decisions

### Why in-memory petgraph over PG-only graph queries
Graph traversals involve many small hops — each would be a DB round-trip. A 1,000-node graph with 2,000 edges fits in ~1MB of RAM. The sidecar enables microsecond graph operations that would take milliseconds via SQL.

### Why StableDiGraph over DiGraph
`StableDiGraph` preserves node/edge indices across removals. This is critical — the UUID→NodeIndex mapping must remain valid when nodes are deleted during consolidation.

### Why full reload over incremental sync (current state)
Simpler to implement correctly. The outbox pattern exists in the schema (`outbox_events` table) but isn't wired to the sidecar yet. Full reload on startup means the sidecar is always consistent with PG — at the cost of requiring a restart to see new data.

### Why tokio RwLock over std RwLock
Search is async and read-heavy. `tokio::sync::RwLock` allows multiple concurrent readers without blocking the async runtime. Writes (sync events) are rare and brief.

## Gaps Identified by Graph Analysis

1. **No incremental sync** — the single biggest operational gap. After ingesting 50+ sources today, we had to restart the server each time to see new graph data in search. Outbox events table exists, just needs to be consumed.

2. **TrustRank has no seed UI** — the algorithm is fully implemented but there's no way to designate which nodes are trusted seeds. Could default to operator-authored articles.

3. **Community structure unused by search** — communities are detected but don't influence search ranking. A result from a dense community (well-connected subgraph) should rank higher than one from a sparse region.

4. **Bridge nodes not boosted** — bridge discovery works but bridges aren't scored higher in search. These are exactly the "connecting" concepts that make cross-domain queries work.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| PageRank | Page & Brin 1998 | ✅ Ingested |
| Leiden communities | Traag et al. 2019 | ✅ Ingested |
| Louvain communities | Blondel et al. 2008 | ✅ Ingested |
| Spreading activation | Collins & Loftus 1975 | 🔄 Worker ingesting |
| TrustRank | Gyöngyi et al. 2004 | ✅ Ingested |
| EigenTrust | Kamvar et al. 2003 | ✅ Ingested |
| Node2Vec | Grover & Leskovec 2016 | ❌ Not ingested |
| K-core decomposition | Matula & Beck 1983 | ❌ Not ingested (textbook) |

## Next Actions

1. Wire outbox-driven incremental sync — consume `outbox_events` on a polling interval
2. Expose TrustRank seed selection via admin API
3. Feed community membership into search dimension scoring
4. Boost bridge nodes in graph search
5. Ingest Node2Vec paper for future graph embedding work
