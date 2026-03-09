# 01 — Architecture

**Status:** Draft

## Overview

The system is structured as three cooperating layers with an LLM boundary at ingestion and query synthesis. The architecture is grounded in five theoretical pillars (see [Theoretical Foundations](#theoretical-foundations)) that inform every design decision.

```
┌─────────────────────────────────────────────────────┐
│                    API Layer                         │
│              (Axum HTTP + MCP)                       │
├─────────────────────────────────────────────────────┤
│                  Engine Layer                        │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │  Search   │  │  Graph   │  │    Ingestion      │  │
│  │  Service  │  │  Sidecar │  │    Pipeline       │  │
│  │          │  │ (petgraph)│  │  (LLM boundary)   │  │
│  └────┬─────┘  └────┬─────┘  └────────┬──────────┘  │
│       │              │                 │             │
│  ┌────┴──────────────┴─────────────────┴──────────┐  │
│  │           Consolidation Pipeline               │  │
│  │    (online → batch → deep / three timescales)  │  │
│  └────────────────────┬───────────────────────────┘  │
│                       │                              │
├───────────────────────┴──────────────────────────────┤
│                 Storage Layer                        │
│           PostgreSQL 17 + pgvector                   │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │  Nodes   │  │  Edges   │  │  Chunks +         │  │
│  │  + Props │  │  + Props │  │  Embeddings       │  │
│  └──────────┘  └──────────┘  └───────────────────┘  │
└─────────────────────────────────────────────────────┘
```

## Theoretical Foundations

Five frameworks converge on this architecture. These are not academic garnish — they directly dictate the system's behavior.

| Pillar | Domain | What It Provides |
|--------|--------|-----------------|
| **Free Energy Principle** (Friston) | Unified objective | Everything the system does minimizes surprise about the world it models. Gap detection, active acquisition, and forgetting all derive from a single variational objective. |
| **AGM Belief Revision** | Update logic | When evidence arrives, belief changes must be minimal, consistent, and epistemically rational. Drives contention resolution and supersession. |
| **Stigmergy** | Coordination | Multiple agents coordinate through the knowledge graph as shared medium — no direct communication required. Rich epistemic annotations (confidence, gaps, contradictions) are the stigmergic marks. |
| **Pearl's Causal Hierarchy** | Depth | Three tiers of edge semantics: L0 association ("X correlates with Y"), L1 intervention ("doing X causes Y"), L2 counterfactual ("had X not happened, Y would not have"). See [07-epistemic-model](07-epistemic-model.md). |
| **Complementary Learning Systems** | Memory architecture | Fast episodic acquisition balanced by slow semantic consolidation. Maps to the three-timescale consolidation pipeline. |

## Layer Responsibilities

### Storage Layer (PostgreSQL + pgvector)

The single source of truth for all persistent state.

- **Nodes** — Entities with properties, type, clearance level, and metadata
- **Edges** — Typed, directed relationships with properties, causal metadata, and temporal bounds
- **Chunks** — Text segments at multiple granularities with parent-child links and structural hierarchy
- **Embeddings** — Vector representations (HNSW-indexed) for chunks and optionally for nodes
- **Sources** — Provenance records tracking origin of every node, edge, and chunk
- **Full-text indexes** — tsvector columns for lexical search

See [03-storage](03-storage.md) for schema details.

### Engine Layer (Rust)

Stateless compute except for the in-memory graph sidecar.

**Graph Sidecar (petgraph)**
- Mirrors the PG edge table as a `DiGraph<Uuid, EdgeMeta>`
- Provides fast traversal, PageRank, community detection, topological confidence
- Periodic TrustRank batch computation for global confidence calibration
- Syncs from PG on startup; incremental updates via notify/listen or polling
- See [04-graph](04-graph.md)

**Search Service**
- Orchestrates parallel dimension queries (vector, lexical, temporal, graph, structural)
- Fuses results via RRF with configurable weights per query strategy
- See [06-search](06-search.md)

**Ingestion Pipeline**
- Accepts raw sources → parses → normalizes to Markdown → chunks → embeds → **analyzes embedding landscape** → targeted extraction → resolves → stores
- Embedding landscape analysis (parent-child alignment, adjacent similarity peaks/valleys) determines which chunks warrant LLM extraction
- LLM-driven extraction with structured output (entities, relationships, co-references) applied only to chunks flagged by landscape analysis
- Entity resolution via vector similarity + graph context
- See [05-ingestion](05-ingestion.md)

**Consolidation Pipeline**
- Three-timescale knowledge maturation, modeled on hippocampal-neocortical memory consolidation:

| Timescale | Scope | Operations |
|-----------|-------|-----------|
| **Online** (per-ingestion, seconds) | Parse, chunk, extract, resolve entities. Update confirms/contradicts/originates edges. Incremental confidence updates. |
| **Batch** (periodic, hours) | Group sources by topic. LLM-based compilation into articles. Bayesian confidence aggregation. Contention detection and queuing. |
| **Deep** (scheduled, daily+) | Bayesian Model Reduction (prune low-value knowledge). Cross-domain generalization discovery. Domain topology map update. TrustRank recalibration. Landmark article identification. |

The online tier handles individual source ingestion. The batch tier produces compiled articles — right-sized summaries (200–4000 tokens) that serve as optimal retrieval units. The deep tier performs structural maintenance and principled forgetting.

### Evaluation Layer (`covalence-eval`)

Layer-by-layer evaluation harness for the pipeline. Provides the `LayerEvaluator` trait with typed inputs, outputs, and metrics. Current evaluators:

- **ChunkerEval** — Evaluates chunking quality (boundary detection, token counts)
- **ExtractorEval** — Evaluates entity/relationship extraction (precision, recall)
- **SearchEval** — Evaluates search result quality (relevance, ranking)

The eval crate produces a `covalence-eval` binary for running evaluations from the command line. See [11-evaluation](11-evaluation.md) for methodology.

### API Layer (Axum + MCP)

Thin routing layer. No business logic.

- HTTP REST endpoints for CRUD, search, ingestion
- MCP tool interface for Claude/agent integration
- See [08-api](08-api.md)

## Data Flow

### Ingestion Path (Online Consolidation)
```
Raw Source → Parser → Markdown Normalizer → Hierarchical Chunker → Embedder
    → Landscape Analysis (peaks/valleys, parent-child alignment)
    → Targeted LLM Extraction (gated by extraction priority map)
    → Entity Resolver → Storage → Graph Sidecar Update
```

### Compilation Path (Batch Consolidation)
```
Sources (by topic cluster) → LLM Compilation → Article → Embed → Store
                                                  ↓
                                          Contention Detection
```

### Query Path
```
Query → Search Service → [Vector, Lexical, Temporal, Graph, Structural] → RRF Fusion → Ranked Results
                              ↑                    ↑
                           pgvector             petgraph
                            (PG)              (in-memory)
```

### Synthesis Path (optional)
```
Ranked Results → Context Assembly → LLM Synthesis → Response
```

### Deep Consolidation Path (scheduled)
```
Full Graph → TrustRank → BMR Pruning Candidates → Prune/Archive
          → Community Detection → Domain Topology Map
          → Cross-Domain Bridge Discovery → Landmark Articles
```

## Boundaries and Invariants

1. **PG is the source of truth.** The graph sidecar is a derived, rebuildable cache. If it diverges, PG wins.
2. **LLM calls are isolated to ingestion extraction, batch compilation, and optional query synthesis.** The engine never requires an LLM for search or graph operations. Within ingestion, LLM extraction is further gated by embedding landscape analysis — only chunks with sufficient novelty/misalignment are sent to the LLM.
3. **Every stored fact has a source.** No node or edge exists without a provenance link to at least one source record.
4. **The graph sidecar is eventually consistent.** Writes go to PG first; the sidecar syncs asynchronously.
5. **Confidence is multi-layered.** Source confidence, extraction confidence, and topological confidence are computed independently and composed at query time. See [07-epistemic-model](07-epistemic-model.md).
6. **Uncertainty is distinct from disbelief.** The system tracks epistemic uncertainty separately from negative belief — "unknown" ≠ "50% likely". See Subjective Logic in [07-epistemic-model](07-epistemic-model.md).

## Open Questions

- [x] Multiple subgraphs/tenants → Single-tenant for v1. Multi-tenant deferred.
- [x] Sync mechanism → Outbox Pattern + LISTEN/NOTIFY wake-up + 5s polling fallback. See [04-graph](04-graph.md).
- [x] Read-only mode → Yes, fallback PG stored procedures (graph_traverse) already specified in [03-storage](03-storage.md).
- [x] Batch consolidation trigger → Both timer AND epistemic delta threshold.
- [x] Deep consolidation process → Embedded in main engine for v1. Extract to separate process when load warrants.
