# Design: Consolidation & Ontology

## Status: substantially complete (HDBSCAN, gravity wells, gap detection, self-loop filtering all wired)

> **Updated 2026-03-10**: HDBSCAN clustering fully wired (#36), cluster pinning/gravity wells
> implemented (#37), self-loop filtering active (#40), knowledge gap detection implemented (#39).
> Background loop still the main outstanding gap.

## Spec Sections: 04-graph.md, 05-ingestion.md, 07-epistemic-model.md

## Architecture Overview

Consolidation is the background process that refines the knowledge graph after ingestion: clustering entities into canonical groups, compiling articles from source clusters, detecting contentions, scheduling epistemic propagation, and running organic forgetting. It's the system's "thinking" phase — ingestion is fast and greedy, consolidation is slow and careful.

## Implemented Components

### Fully Implemented ✅

| Component | File | Notes |
|-----------|------|-------|
| **HDBSCAN clustering** | `consolidation/ontology.rs` | **UPDATED (#36)**: Fully wired end-to-end — density-based clustering for entity names, types, rel_types; results written back to DB |
| **Entity name clustering** | `consolidation/ontology.rs` | `build_entity_clusters()` with embedding similarity |
| **Entity type clustering** | `consolidation/ontology.rs` | `build_type_clusters()` — currently 11 types, already clean |
| **Rel type clustering** | `consolidation/ontology.rs` | `build_rel_type_clusters()` — 354 raw → 74 natural clusters |
| **Canonical columns write-back** | `consolidation/ontology.rs` | `canonical_rel_type`, `canonical_type`, `cluster_id` populated via `apply_clusters()` |
| **Gravity well model** | `consolidation/gravity.rs` | **NEW (#37)**: Three-phase (partition → HDBSCAN → attraction). Pinned clusters act as attractors — noise points are pulled into the nearest well if within distance threshold. |
| **Cluster pinning** | `consolidation/gravity.rs` | **NEW (#37)**: Operator-pinned clusters persist across re-runs; `ontology_overrides` table with manual pins respected |
| **Self-loop filtering** | `consolidation/graph_batch.rs` | **NEW (#40)**: Self-referential edges (A → A) detected and removed automatically during batch consolidation |
| **Knowledge gap detection** | `consolidation/gaps.rs` | **NEW (#39)**: Implemented as API endpoint + background analysis. Detects: low-connectivity nodes, clusters with few sources, concepts with no outbound edges, unresolved contentions. |
| **Article compilation** | `consolidation/compiler.rs` | `ConcatCompiler` (baseline) + `LlmCompiler` (LLM-summarized) |
| **Contention detection** | `consolidation/contention.rs` | Finds contradictions in graph structure |
| **Batch consolidation** | `consolidation/batch.rs` | `BatchJob` orchestration |
| **Graph batch consolidation** | `consolidation/graph_batch.rs` | Graph-level cleanup and optimization |
| **Consolidation scheduler** | `consolidation/scheduler.rs` | Delta-driven scheduling — runs when accumulated changes exceed threshold |
| **Deep consolidation** | `consolidation/deep.rs` | Expensive full-graph analysis |
| **Topic detection** | `consolidation/topic.rs` | Topic/theme clustering |
| **Summary generation** | `consolidation/summary.rs` | Community/topic summaries |
| **Label embedding cache** | `consolidation/ontology.rs` | Embeddings persisted in `ontology_label_embeddings` table — no re-embed on reruns |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **Scheduler wiring** | Scheduler logic exists | Not wired to a background task loop — must be triggered manually via admin endpoint |
| **Example entity filtering** | `apply_clusters()` exists | Min label length filter added, but example entity detection (spec examples injected as real nodes) still imperfect |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **Background consolidation loop** | Spec: periodic consolidation | Medium — scheduler exists and gap detection is wired, but no tokio background task runs the loop automatically |
| **Organic forgetting trigger** | Spec 07: BMR-based eviction on schedule | Medium — `bmr_analysis()` exists, not triggered |
| **Cluster change tracking** | — | Low — no diff/notification when clusters shift across runs |

## Key Design Decisions

### Why HDBSCAN over threshold-based clustering
User explicitly rejected magic thresholds. HDBSCAN with `min_cluster_size: 2` finds natural density clusters — no tuning needed. When tested on 354 rel_types, it found 74 natural clusters with mostly excellent merges.

### Why gravity wells (#37)
Pure HDBSCAN classifies many points as noise (cluster_id = -1). Gravity wells — pinned, canonical clusters — attract nearby noise points via distance threshold. This prevents ontology fragmentation: a concept like "embedding model" appearing 3 times in slightly different phrasings should converge to a single canonical node. Pinned clusters are protected from re-clustering: operator knowledge wins over statistics.

### Why emergent ontology over predefined schema
Covalence's "special sauce": the ontology emerges from data via embed→cluster→name. No predefined entity types, no relationship schema. This scales to arbitrary domains without manual ontology engineering. The tradeoff (per Noy & McGuinness 2001) is less precision but vastly better adaptability.

### Why delta-driven scheduling over fixed intervals
The scheduler tracks accumulated epistemic delta across ingestions. Consolidation runs when the delta exceeds a threshold — not on a timer. This means bursts of ingestion trigger consolidation, but quiet periods don't waste compute.

### Why knowledge gap detection as an API endpoint (#39)
Gap detection needs to be inspectable by developers, not just run in the background. Exposing it via API allows: manual audits before/after large ingestion batches, integration with CI (fail if gap score exceeds threshold), and developer dashboards. The background analysis is additive — it stores results, the API serves them.

### Why self-loop filtering (#40)
Self-loops (A → A) are artifacts of extraction, not meaningful knowledge. A chunk mentioning "Rust builds fast Rust code" can produce a "builds" edge from Rust→Rust. These edges distort PageRank, community detection, and spreading activation. Removing them at consolidation is safer than blocking them at ingestion (where false positives could drop real data).

## Gaps Identified

1. **No background loop** — the scheduler decides *when* to consolidate but nothing *runs* it
   automatically. Needs a tokio background task launched at server startup.

2. **Example entity pollution** — illustrative entities from spec documents ("John works at Google",
   etc.) still appear in the graph. Min label length helps but semantic detection (embedding
   similarity to known example templates) is needed.

3. **Contention detection results not surfaced** — `detect_contentions()` runs but results aren't
   exposed beyond the basic contention list endpoint.

4. **No cluster change tracking** — when clusters shift across consolidation runs, there's no diff
   or notification. Important for understanding ontology evolution over time.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| HDBSCAN | Campello et al. 2013 | ❌ Not ingested — should be |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Ingested |
| KG Quality Assessment | Zaveri et al. 2016 | ✅ Ingested |
| ACT-R forgetting | Anderson 1998 | ✅ Ingested |
| Argumentation frameworks | Dung 1995 | ✅ Ingested |

## Next Actions

1. Wire scheduler to background tokio task loop (server startup)
2. Ingest HDBSCAN paper (Campello et al. 2013)
3. Improve example entity detection with embedding-similarity filter
4. Wire organic forgetting trigger to consolidation schedule
5. Add cluster change diff/notification
