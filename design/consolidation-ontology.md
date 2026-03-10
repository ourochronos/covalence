# Design: Consolidation & Ontology

## Status: partial

## Spec Sections: 04-graph.md, 05-ingestion.md, 07-epistemic-model.md

## Architecture Overview

Consolidation is the background process that refines the knowledge graph after ingestion: clustering entities into canonical groups, compiling articles from source clusters, detecting contentions, scheduling epistemic propagation, and running organic forgetting. It's the system's "thinking" phase — ingestion is fast and greedy, consolidation is slow and careful.

## Implemented Components

### Fully Implemented ✅

| Component | File | Notes |
|-----------|------|-------|
| **HDBSCAN clustering** | `consolidation/ontology.rs` | Density-based clustering for entity names, types, rel_types (#36) |
| **Entity name clustering** | `consolidation/ontology.rs` | `build_entity_clusters()` with embedding similarity |
| **Entity type clustering** | `consolidation/ontology.rs` | `build_type_clusters()` — currently 11 types, already clean |
| **Rel type clustering** | `consolidation/ontology.rs` | `build_rel_type_clusters()` — 354 raw → 74 natural clusters |
| **Article compilation** | `consolidation/compiler.rs` | `ConcatCompiler` (baseline) + `LlmCompiler` (LLM-summarized) |
| **Contention detection** | `consolidation/contention.rs` | Finds contradictions in graph structure |
| **Batch consolidation** | `consolidation/batch.rs` | `BatchJob` orchestration |
| **Graph batch consolidation** | `consolidation/graph_batch.rs` | Graph-level cleanup and optimization |
| **Consolidation scheduler** | `consolidation/scheduler.rs` | Delta-driven scheduling — runs when accumulated changes exceed threshold |
| **Deep consolidation** | `consolidation/deep.rs` | Expensive full-graph analysis |
| **Topic detection** | `consolidation/topic.rs` | Topic/theme clustering |
| **Summary generation** | `consolidation/summary.rs` | Community/topic summaries |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **Cluster write-back** | `apply_clusters()` exists | Dry-run tested, but canonical columns not yet in schema |
| **Gravity well model** | Designed in #31 | Three-phase (partition→HDBSCAN→wells) not yet coded |
| **Label embedding cache** | Embeddings computed per run | Not persisted — re-embeds on every cluster run |
| **Scheduler wiring** | Scheduler logic exists | Not wired to a background task loop |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **Canonical columns** | #31: `canonical_rel_type`, `canonical_type`, `cluster_id` | High — needed for cluster write-back |
| **Ontology overrides table** | #31: `ontology_overrides` for manual pins | High — needed for HITL |
| **Gravity well attraction** | #31: pinned clusters attract noise points | Medium |
| **Background consolidation loop** | Spec: periodic consolidation | Medium — scheduler exists, loop doesn't |
| **Organic forgetting trigger** | Spec 07: BMR-based eviction on schedule | Medium |
| **Self-loop cleanup** | #40: filter self-referential edges | Low — easy win |
| **Example entity filtering** | #40: detect and tag illustrative entities | Low |

## Key Design Decisions

### Why HDBSCAN over threshold-based clustering
User explicitly rejected magic thresholds. HDBSCAN with `min_cluster_size: 2` finds natural density clusters — no tuning needed. When tested on 354 rel_types, it found 74 natural clusters with mostly excellent merges.

### Why emergent ontology over predefined schema
Covalence's "special sauce": the ontology emerges from data via embed→cluster→name. No predefined entity types, no relationship schema. This scales to arbitrary domains without manual ontology engineering. The tradeoff (per Noy & McGuinness 2001, now in KB) is less precision but vastly better adaptability.

### Why delta-driven scheduling over fixed intervals
The scheduler tracks accumulated epistemic delta across ingestions. Consolidation runs when the delta exceeds a threshold — not on a timer. This means bursts of ingestion trigger consolidation, but quiet periods don't waste compute.

### Why separate batch and deep consolidation
Batch is cheap: entity merging, duplicate cleanup, basic graph optimization. Deep is expensive: full epistemic propagation, community re-detection, embedding landscape analysis. Different intervals prevent deep consolidation from blocking ingestion.

## Gaps Identified by Graph Analysis

1. **No background loop** — the scheduler decides *when* to consolidate but nothing *runs* it. Needs a tokio background task.

2. **Label embeddings recomputed every time** — should cache permanently. Label text never changes; embedding is deterministic.

3. **Cluster quality varies** — entity clustering produced 211 clusters, but many were noisy (single-letter labels, example entities). Needs minimum label length filter and example detection (#40).

4. **Contention detection runs but isn't surfaced** — `detect_contentions()` exists but results aren't stored or exposed via API beyond the basic contention list.

5. **No cluster change tracking** — when clusters change across runs, there's no diff or notification. Important for understanding ontology evolution.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| HDBSCAN | Campello et al. 2013 | ❌ Not ingested — should be |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Just ingested |
| KG Quality Assessment | Zaveri et al. 2016 | ✅ Just ingested |
| ACT-R forgetting | Anderson 1998 | 🔄 Worker ingesting |
| Argumentation frameworks | Dung 1995 | 🔄 Worker ingesting |

## Next Actions

1. Add canonical columns via migration and wire `apply_clusters()` to write them
2. Implement label embedding cache (persist in `ontology_label_embeddings` table)
3. Wire scheduler to background task loop
4. Implement gravity well model (partition→HDBSCAN→attraction)
5. Add minimum label length filter to clustering
6. Ingest HDBSCAN paper
