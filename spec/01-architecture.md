# 01 — Architecture

**Status:** Implemented

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

- **Nodes** — Entities with properties, type, clearance level, and metadata. Includes code entities (`code_function`, `code_struct`, `code_trait`, `code_module`, `code_impl`) and `component` bridge nodes.
- **Edges** — Typed, directed relationships with properties, causal metadata, and temporal bounds. Includes structural code edges (`CALLS`, `USES_TYPE`, `IMPLEMENTS`, `CONTAINS`, `DEPENDS_ON`) and cross-domain bridge edges (`IMPLEMENTS_INTENT`, `PART_OF_COMPONENT`, `THEORETICAL_BASIS`).
- **Statements** — Atomic, self-contained knowledge claims extracted from source text. The primary retrieval unit.
- **Sections** — Compiled summaries of semantically clustered statements within a source.
- **Chunks** — Text segments at multiple granularities (legacy pipeline, retained for backward compatibility)
- **Components** — Bridge nodes linking spec topics to code entities to research concepts
- **Embeddings** — Vector representations (HNSW-indexed) for statements, sections, chunks, and nodes
- **Sources** — Provenance records tracking origin of every node, edge, statement, and chunk
- **Full-text indexes** — tsvector columns for lexical search

See [03-storage](03-storage.md) for schema details.

### Engine Layer (Rust)

Stateless compute except for the in-memory graph sidecar.

**Graph Sidecar (petgraph)**
- Mirrors the PG edge table as a `DiGraph<Uuid, EdgeMeta>`
- Provides fast traversal, PageRank, community detection, topological confidence
- Periodic TrustRank batch computation for global confidence calibration
- Cross-domain analysis: erosion detection, coverage analysis, blast-radius simulation (see [04-graph](04-graph.md#cross-domain-analysis))
- Syncs from PG on startup; incremental updates via notify/listen or polling
- See [04-graph](04-graph.md)

**Search Service**
- Orchestrates parallel dimension queries (vector, lexical, temporal, graph, structural)
- Fuses results via RRF with configurable weights per query strategy
- See [06-search](06-search.md)

**Ingestion Pipeline**
- Two ingestion paths, both producing statements, sections, nodes, and edges:
  - **Prose path** (default): raw source → normalize to Markdown → windowed LLM statement extraction → embed → cluster → compile sections → compile source summary → entity extraction from statements. See [05-ingestion](05-ingestion.md) and [ADR-0015](../docs/adr/0015-statement-first-extraction.md).
  - **Code path** (`source_type = "code"`): raw source → Tree-sitter AST parse → chunk by AST boundary → LLM semantic summary → embed summary → statement extraction on summaries → structural edge extraction (CALLS, USES_TYPE, etc.) → Component linking. See [12-code-ingestion](12-code-ingestion.md).
- Legacy chunk pipeline (landscape analysis, chunk-level extraction) retained for backward compatibility
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
- **StatementEval** — Evaluates statement extraction quality (self-containment, coref resolution)
- **CrossDomainEval** — Evaluates coverage, drift, and gap detection accuracy

The eval crate produces a `covalence-eval` binary for running evaluations from the command line. See [11-evaluation](11-evaluation.md) for methodology.

### API Layer (Axum + MCP)

Thin routing layer. No business logic.

- HTTP REST endpoints for CRUD, search, ingestion
- MCP tool interface for Claude/agent integration
- See [08-api](08-api.md)

## Data Flow

### Prose Ingestion Path (Online Consolidation)
```
Raw Source → Parser → Markdown Normalizer
    → Windowed LLM Statement Extraction (coref resolution)
    → Embed Statements → HAC Clustering → Compile Sections
    → Compile Source Summary → Entity Extraction from Statements
    → Entity Resolver → Storage → Graph Sidecar Update
```

### Code Ingestion Path (Online Consolidation)
```
Source (source_type = "code") → Tree-sitter AST Parse
    → Chunk by AST Boundary (function, struct, module)
    → LLM Semantic Summary per Chunk → Embed Summary
    → Statement Extraction on Summaries
    → Structural Edge Extraction (CALLS, USES_TYPE, IMPLEMENTS, CONTAINS)
    → Component Linking (PART_OF_COMPONENT, IMPLEMENTS_INTENT)
    → Storage → Graph Sidecar Update
```

### Compilation Path (Batch Consolidation)
```
Statements + Sections (by topic cluster) → LLM Compilation → Article → Embed → Store
                                                                ↓
                                                        Contention Detection
```

### Query Path
```
Query → Search Service → [Vector, Lexical, Temporal, Graph, Structural, Global] → CC/RRF Fusion → Ranked Results
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
          → Cross-Domain Analysis (erosion, coverage, whitespace)
```

## Boundaries and Invariants

1. **PG is the source of truth.** The graph sidecar is a derived, rebuildable cache. If it diverges, PG wins.
2. **LLM calls are isolated to ingestion, compilation, and optional synthesis.** The engine never requires an LLM for search or graph operations. LLM calls occur during: statement extraction (prose), semantic summary generation (code), batch article compilation, and optional query synthesis.
3. **Every stored fact has provenance.** No node, edge, or statement exists without a provenance link to at least one source record. Statements trace to source byte offsets; entities trace to the statements they were extracted from.
4. **The graph sidecar is eventually consistent.** Writes go to PG first; the sidecar syncs asynchronously.
5. **Confidence is multi-layered.** Source confidence, extraction confidence, and topological confidence are computed independently and composed at query time. See [07-epistemic-model](07-epistemic-model.md).
6. **Uncertainty is distinct from disbelief.** The system tracks epistemic uncertainty separately from negative belief — "unknown" ≠ "50% likely". See Subjective Logic in [07-epistemic-model](07-epistemic-model.md).
7. **Statements are the primary retrieval unit.** Self-contained, coreference-resolved atomic claims. Chunks are retained for backward compatibility but statements are the default extraction and search target.
8. **Code and prose share a vector space.** Code entities are embedded via their semantic summaries (natural language descriptions of business logic), not raw syntax. This enables cross-domain search without explicit query routing.

## Open Questions

- [x] Multiple subgraphs/tenants → Single-tenant for v1. Multi-tenant deferred.
- [x] Sync mechanism → Outbox Pattern + LISTEN/NOTIFY wake-up + 5s polling fallback. See [04-graph](04-graph.md).
- [x] Read-only mode → Yes, fallback PG stored procedures (graph_traverse) already specified in [03-storage](03-storage.md).
- [x] Batch consolidation trigger → Both timer AND epistemic delta threshold.
- [x] Deep consolidation process → Embedded in main engine for v1. Extract to separate process when load warrants.
