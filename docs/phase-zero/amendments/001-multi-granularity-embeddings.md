# Spec Amendment 001: Multi-Granularity Embedding Pipeline

**Date:** 2026-03-01  
**Status:** Implemented  
**Affects:** §6.3 (Vector Embeddings), §8.1 (Fast Path), §8.2 (Slow Path), §5 (v0 Scope)  
**Author:** Jane  

---

## Problem

The v0 spec defines a single embedding per node (`node_embeddings` table, §6.3). This design has two critical flaws:

1. **Silent truncation.** OpenAI's `text-embedding-3-small` accepts 8,191 tokens (~28K chars). Sources exceeding this limit were truncated at the API client level, silently discarding content. The spec does not acknowledge this limitation or prescribe a handling strategy.

2. **No sub-document retrieval.** A 10K-char source about five distinct topics gets one embedding that is the semantic average of all five topics. A query matching one subtopic competes with the diluted signal of the other four. Valence v2 solved this with `tree_index` + `section_embeddings` — Covalence did not inherit this capability.

Both problems violate the spec's own principle: *"Covalence replaces the relational scaffolding [...] while preserving the compilation pipeline."* The tree index pipeline is part of Valence's compilation pipeline.

---

## Solution: Tree Index + Section Embeddings

### New Table: `covalence.node_sections`

```sql
CREATE TABLE covalence.node_sections (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID NOT NULL REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    tree_path       TEXT NOT NULL,       -- "0", "0.1", "0.2.3" etc.
    depth           INT NOT NULL DEFAULT 0,
    title           TEXT,
    summary         TEXT,
    start_char      INT NOT NULL,
    end_char        INT NOT NULL,
    content_hash    TEXT,                -- MD5 of slice for change detection
    embedding       halfvec(1536),
    model           TEXT DEFAULT 'text-embedding-3-small',
    created_at      TIMESTAMPTZ DEFAULT now(),
    CONSTRAINT node_sections_unique UNIQUE (node_id, tree_path)
);

CREATE INDEX node_sections_hnsw_idx ON covalence.node_sections
    USING hnsw (embedding halfvec_cosine_ops) WITH (m = 16, ef_construction = 64);
```

### Pipeline

```
Source ingested (any size)
    │
    ├─ < 700 chars ─────── Trivial tree (1 node, no LLM)
    │                        → Direct embedding (fast path)
    │
    ├─ 700–280K chars ───── Single LLM call for tree decomposition
    │                        → Section embeddings + composed node embedding
    │
    └─ > 280K chars ──────── Sliding window with tunable overlap (default 20%)
                              → Per-window LLM tree decomposition
                              → LLM merge pass (recursive for very large sources)
                              → Section embeddings + composed node embedding
```

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| No truncation at any level | Sliding windows for oversized sections | Truncation silently loses content; unacceptable |
| Overlap is tunable | Default 20%, configurable per-call | Different content types need different overlap |
| Composed embedding = mean of leaf sections | Stored in `node_embeddings` with model suffix `:composed` | Backwards compatible with existing vector search |
| Section embeddings participate in vector search | UNION ALL with node_embeddings, MIN(distance) per node | Sub-document precision without result duplication |
| Tree index stored in `nodes.metadata->'tree_index'` | JSONB, not a separate table | Tree structure is metadata; sections are the queryable artifact |
| Content hash per section | MD5 of content slice | Incremental re-embedding on content update |

### Thresholds and Constants

| Constant | Value | Rationale |
|----------|-------|-----------|
| `TRIVIAL_THRESHOLD_CHARS` | 700 | Below this, LLM decomposition adds no value |
| `SINGLE_WINDOW_MAX_CHARS` | 280,000 | ~80K tokens; fits in one LLM context window |
| `DEFAULT_WINDOW_CHARS` | 280,000 | Match single-window max for consistency |
| `DEFAULT_OVERLAP_FRACTION` | 0.20 | 20% overlap preserves context at window boundaries |
| `MAX_SECTION_EMBED_CHARS` | 24,000 | text-embedding-3-small limit is ~28K; 24K leaves margin |
| `MIN_SECTION_CHARS` | 50 | Don't create tiny fragments |

### Impact on Existing Architecture

**§6.3 (Vector Embeddings):** `node_embeddings` now stores either direct embeddings (small sources) or composed embeddings (tree-indexed sources). The `model` field distinguishes them: `text-embedding-3-small` vs `text-embedding-3-small:composed`.

**§7.1 (Three-Dimensional Retrieval):** The `VectorAdaptor` now searches both `node_embeddings` AND `node_sections` via UNION ALL, taking the best (minimum distance) match per node. This provides sub-document precision without breaking the node-level result contract.

**§8.1 (Fast Path):** Sources < 700 chars remain fully fast-path (direct embed, no LLM). The `handle_embed` worker task delegates large sources to the tree pipeline automatically.

**§8.2 (Slow Path):** Two new task types added to `slow_path_queue`:
- `tree_index` — LLM-driven tree decomposition (priority 5)
- `tree_embed` — section embedding + composition (priority 4, runs after tree_index)

**§6.1 (Schema Layout):** Add `node_sections` to the schema listing.

**§13.2 (Performance Targets):** Tree indexing a 10K source: ~15s (single LLM call). Section embedding: ~5s (17 embed calls). These are slow-path operations and do not affect fast-path ingest latency.

### Memory Estimate

At 50,000 nodes with average 10 sections each = 500,000 section embeddings.
500K × 1536 dims × 2 bytes = ~1.5 GB raw. HNSW overhead ~2.25 GB. Total: ~3.75 GB.

This exceeds the v0 spec's original estimate of 375 MB for 50K nodes. At current scale (~420 nodes, ~10 sections each = 4,200 sections) the cost is ~13 MB — negligible. The 50K-node estimate needs to be revisited if we approach that scale; options include reducing section granularity or using dimensionality reduction.

---

## Spec Sections to Update

The following sections of the main spec should be updated to reflect this amendment:

1. **§5 v0 Scope** — Add "Multi-granularity embedding pipeline" to v0 deliverables
2. **§6.1 Schema Layout** — Add `node_sections` table
3. **§6.3 Vector Embeddings** — Expand to cover section embeddings and composed vectors
4. **§7.1 Three-Dimensional Retrieval** — Note that VectorAdaptor searches both tables
5. **§8.1 Fast Path** — Note delegation to slow path for large sources
6. **§8.2 Slow Path** — Add `tree_index` and `tree_embed` operations
7. **§13.1 Functional Parity** — Add: "Sources of any size are embedded without truncation"
8. **§13.2 Performance Targets** — Add tree_index and tree_embed timing targets
