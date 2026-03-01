# Covalence: Phase Zero Specification

**Project:** Covalence — Graph-Native Knowledge Substrate for AI Agent Persistent Memory  
**Document Version:** 1.0.0  
**Date:** 2026-03-01  
**Authors:** Jane (Architecture Lead), with research contributions from Literature Review, Extension Ecosystem, SING Translation, and Substrate Audit subagents  
**Status:** Approved for Phase One Planning

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Vision](#2-vision)
3. [Constraints](#3-constraints)
4. [Architecture Overview](#4-architecture-overview)
5. [v0 Scope — What We Build First](#5-v0-scope--what-we-build-first)
6. [Data Model](#6-data-model)
7. [Retrieval Architecture](#7-retrieval-architecture)
8. [Inference vs. Algorithm Boundary](#8-inference-vs-algorithm-boundary)
9. [Migration Strategy](#9-migration-strategy)
10. [API Surface (REST/OpenAPI)](#10-api-surface-restopenapi)
11. [Risk Register](#11-risk-register)
12. [Roadmap](#12-roadmap)
13. [Success Criteria](#13-success-criteria)

---

## 1. Problem Statement

### 1.1 What We Have

The current knowledge substrate — Valence v2 — is a Python MCP server backed by PostgreSQL 16. As of the audit date (2026-02-28), it manages:

| Metric | Value | Signal |
|--------|-------|--------|
| Total articles | 289 | |
| Active articles | 126 | 56% activity rate — accumulation problem |
| Total sources | 264 | |
| Unresolved contentions | 26 | 21% of active articles have open contradictions |
| Unique domain paths | 1 | No meaningful domain taxonomy |
| Articles with embeddings | 270 | 19 articles have no vector representation |

These numbers tell a story: at 289 articles and 264 sources — a modest corpus by any measure — the substrate is already exhibiting serious structural dysfunction. It is not scaling gracefully.

### 1.2 The Root Cause: A List Masquerading as a Graph

Every failure mode in Valence v2 traces to a single architectural error: **knowledge versioning is modeled as a doubly-linked list, not a graph.**

```
articles.supersedes_id  →  ONE foreign key per article
sources.supersedes_id   →  ONE foreign key per source
```

A single parent pointer cannot express:

| Real Knowledge Event | What Valence v2 Can Model | What Actually Happens |
|---|---|---|
| Article grows; one subtopic becomes its own subject (split) | Nothing | Two disconnected articles; relationship lost |
| Two overlapping articles are reconciled (merge) | Nothing | New article with no structural link to parents |
| Article A contradicts Article B | Side-table hack (contentions) | Contention row; no graph edge |
| Article A elaborates on Article B (without superseding) | Nothing | No edge at all |
| Source B corrects Source A (which had two child articles) | One pointer; one child | Second child loses the correction link |

The `article_sources` junction table exists but only bridges articles to sources; it cannot express article-to-article or source-to-source relationships. Its edge vocabulary (`originates`, `confirms`, `supersedes`, `contradicts`, `contends`) is frozen in a PostgreSQL `CHECK` constraint — adding a new relationship type requires a schema migration.

### 1.3 Cascade Failures from the Root Cause

**Duplicate accumulation.** With no concept of stable topic identity, each `article_compile` call produces a new article node. Two sessions compiling the same topic produce two active articles — structurally indistinguishable from the substrate's point of view. The 289 total / 126 active ratio (56%) at tiny scale predicts tens of thousands of zombie articles at production scale.

**False contention flood.** Contention detection uses cosine similarity (threshold 0.85). Near-duplicate articles — which are themselves a symptom of the accumulation problem — look semantically similar to each other and trigger false contradiction alerts. 26 unresolved contentions at 126 active articles means roughly one in five active articles is flagged as contradicting something. Most are false positives that require manual intervention to dismiss. This does not scale.

**Provenance opacity.** There is no mechanism to ask "what is the full provenance chain for this article?" The `supersedes_id` chain can be walked in application code one link at a time, but the substrate has no graph traversal capability. Multi-hop provenance walks require fetching the full table and doing BFS in memory. Cluster detection, influence graphs, and convergent-compilation detection are impossible at the substrate level.

**Confidence state fragmentation.** Article confidence is stored in three places simultaneously: a `confidence` JSONB blob, six decomposed float columns (`confidence_source`, `confidence_method`, etc.), and a `corroborating_sources` JSONB array. These can and do drift from each other. There is no constraint enforcing consistency between them.

**Schema rigidity under relational misuse.** The `entities` and `article_entities` tables were built for entity extraction but were never connected to any tool or retrieval path. They consume schema space and migration complexity delivering nothing. The `opt_out_federation` and `share_policy` fields on contentions are dead residue from an abandoned P2P plan. `compilation_queue.source_ids` is `text[]` (not `uuid[]`) with no FK enforcement, so `admin_forget` can silently produce stale queue entries.

### 1.4 What the Research Literature Validates

The audit findings are not isolated — they are well-predicted by the research community's consensus about why flat knowledge systems fail at agent memory.

Six recent papers (arXiv 2602.05665, 2601.03236, 2512.13564, 2410.01778, 2510.14271, 2507.03226) examined during the Covalence literature review converge on four findings directly relevant to Valence v2's failure modes:

1. **"Graph-based memory is not a special case of memory — it is the general case."** (Yang et al., 2026, arXiv:2602.05665) — Linear buffers and key-value stores are degenerate graphs. Building on a degenerate representation forecloses the capabilities that non-degenerate graphs enable.

2. **"Typed edges are the core value proposition, not an implementation detail."** (MAGMA, Jiang et al., arXiv:2601.03236) — Systems that cannot distinguish temporal from causal from semantic edges cannot route queries intelligently or reason about *why* something is true, only *what* is stored.

3. **"A smaller, higher-quality graph consistently outperforms a larger, noisier one."** (DEG-RAG, Zheng et al., arXiv:2510.14271) — Accumulation without structured consolidation degrades retrieval. Valence v2's duplicate accumulation is exactly the phenomenon DEG-RAG empirically characterizes.

4. **"Dual-stream ingestion — fast path for raw storage, slow path for structural consolidation — is the correct write architecture."** (MAGMA, validated against Covalence's existing design) — Valence v2 has this structurally (source_ingest + article_compile) but the slow path has no async queue visibility, no edge inference capability, and no consolidation discipline.

### 1.5 The Decision

Valence v2 is a well-intentioned relational solution to a graph problem. Its pipeline logic (ingest → compile → retrieve → decay) is sound and should be preserved. Its ranking formula, confidence model, and usage-trace-driven self-organisation are genuine innovations. But the data model cannot be fixed in place — the supersedes_id problem is structural, not incidental, and every workaround creates new inconsistencies.

**Covalence replaces the relational scaffolding with a typed-edge property graph while preserving the compilation pipeline, ranking formula, confidence model, usage traces, and the external API contract. The agent contract does not break; only the substrate underneath changes.**

---

## 2. Vision

### 2.1 What Covalence Is

Covalence is a **graph-native knowledge substrate for AI agent persistent memory**. It stores, organizes, retrieves, and evolves structured knowledge on behalf of an AI agent — enabling the agent to accumulate genuine understanding over time rather than maintaining a growing pile of disconnected text fragments.

The word "substrate" is intentional. Covalence is not a knowledge graph in the academic ontology sense. It is not a vector database. It is not a search engine. It is a **substrate**: a foundational layer on which all of those capabilities are built, unified under a single transactional store, a single query interface, and a single coherent data model.

### 2.2 How It Differs from the Current System

| Dimension | Valence v2 | Covalence |
|---|---|---|
| **Knowledge model** | Articles and sources as flat relational rows | Rich property nodes connected by typed edges in a property graph |
| **Versioning** | Doubly-linked list (`supersedes_id`) | Typed graph edges; splits, merges, and corrections are first-class events |
| **Edge schema** | Five fixed types in a CHECK constraint | Extensible string labels; new edge types require no migration |
| **Graph traversal** | None at the substrate level | Apache AGE / openCypher traversal; bounded BFS, neighborhood queries, provenance walks |
| **Retrieval** | FTS (pg_trgm) + vector (HNSW), simple RRF | Three-dimensional: graph traversal + semantic (PGVector HNSW) + lexical (BM25/FTS), intent-aware routing, cascaded pre-filtering |
| **Implementation** | Python MCP server | Rust engine + Go CLI; REST API; MCP via OpenClaw plugin |
| **Confidence** | Three drifting representations | Single canonical confidence struct per node |
| **Contention model** | Side-table; article-to-article only | Graph edges; detectable via traversal; source-to-article contentions possible |
| **Maintenance pipeline** | Ad-hoc mutation queue | Explicit dual-stream: fast algorithmic path + slow inference path with logged decisions |

### 2.3 What It Enables That Is Currently Impossible

**Structural provenance.** "Show me the complete lineage of this claim — every source that contributed to this article, every source that contributed to those sources' parent articles, and every article that was merged into this one." This requires multi-hop graph traversal. In Valence v2, it requires writing custom SQL for each new traversal pattern. In Covalence, it is a single AGE Cypher query.

**Topic-stable identity.** A node for "the Covalence schema" persists with a stable UUID as new information arrives. New sources update the node rather than spawning clones. The duplicate accumulation problem is structurally prevented because the graph knows what topics it already has nodes for.

**Intent-aware retrieval.** A query like "why did we decide to use AGE instead of Neo4j?" should prioritize causal edges. "What happened during the migration?" should traverse temporal edges. "What is related to the retrieval architecture?" should use semantic similarity. Covalence routes queries along the right edge dimensions based on declared or inferred intent — impossible in a system with no edge types.

**Typed contention resolution.** When source B contradicts article A, the relationship is expressed as a `contradicts` edge in the graph. Downstream articles that depend on A can be flagged automatically. Resolution creates a new `supersedes` edge — no orphan contention records, no manual bookkeeping.

**Graduation of inference to algorithm.** Every slow-path inference decision (edge type assignment, contention detection, confidence update) is logged. As patterns emerge, they can be promoted to fast-path algorithms. Covalence is designed to improve its own hot path over time by learning from the slow path.

### 2.4 What the Research Community Has Converged On

The six-paper literature review produces a clear consensus table:

| Design Decision | Research Consensus | Evidence |
|---|---|---|
| Graph + Vector dual substrate | **Very High** | Papers 1, 2, 4, 5, 6 |
| Typed edges as first-class data | **Very High** | Papers 1, 2, 3, 6 |
| Hybrid retrieval (graph + vector + text) via RRF | **High** | Papers 2, 5, 6 |
| Dual-stream ingestion (fast/slow) | **High** | Paper 2; validated by existing Covalence design |
| Intent-aware query routing | **High** | Papers 1, 2, 3 |
| Graph quality > graph size | **High** | Papers 5, 6 |
| Memory lifecycle as primary API framing | **High** | Papers 1, 3 |
| Richer edge taxonomy (causal, temporal, entity) | **High** | Papers 1, 2 |

The MAGMA architecture (arXiv:2601.03236) is the closest published analog to Covalence. It independently arrived at every major Covalence design decision: the PGVector + property graph dual storage model, the dual-stream write architecture, RRF-based anchor fusion, and intent-aware traversal. This is strong validation.

Covalence's advantage over MAGMA and every other published system: all three storage capabilities (graph, vector, lexical) live in a **single ACID-transactional PostgreSQL instance** — no cross-database synchronization overhead, no consistency windows between the graph store and the vector store.

---

## 3. Constraints

### 3.1 Target Hardware

| Dimension | Specification |
|---|---|
| Primary machine | Apple M4 Mac Mini |
| RAM | 16 GB unified memory |
| Storage | Local NVMe; assume ≤ 100 GB working set |
| Concurrency | Primary: single-agent; design for future multi-agent namespace isolation |
| OS | macOS (primary), Linux (secondary — must work in Docker) |

**Memory budget allocation (estimated):**
- PostgreSQL 17 shared_buffers: 2 GB
- PostgreSQL work_mem (per connection): 256 MB
- Rust engine process: 512 MB
- Go CLI process: 64 MB
- OS + inference workload: 13+ GB remaining

The M4's unified memory architecture means PostgreSQL and the inference workload compete for the same physical RAM. Covalence must be a good neighbor: avoid bloating `shared_buffers` above 2 GB, keep HNSW index creation within `maintenance_work_mem = 2 GB`, and avoid holding large result sets in process memory.

### 3.2 Software Stack

The following stack is fixed. No substitutions without architectural review.

| Layer | Technology | Version | Notes |
|---|---|---|---|
| Database | PostgreSQL | 17 | Core RDBMS |
| Graph extension | Apache AGE | 1.7.0 (PG17 branch) | openCypher queries |
| Vector extension | pgvector | 0.8.2 | HNSW + halfvec |
| Text search extension | pg_textsearch (Tiger Data) | 1.0.0-dev | BM25; fallback to ts_rank |
| Engine language | Rust | stable | async via tokio |
| CLI language | Go | 1.22+ | Cobra-based |
| API style | REST | OpenAPI 3.1 | No MCP in v0 |
| Embeddings | OpenAI text-embedding-3-small | — | 1536 dims; design for swappability |

### 3.3 Scope Exclusions

The following are **out of scope for v0** and must not drive architectural decisions. This list is a firewall against scope creep.

| Excluded Feature | Rationale | When It Enters |
|---|---|---|
| **MCP protocol** | REST + Go CLI provides equivalent tool access without MCP dependency | v1 planning review |
| **Plasmon (NL↔triples)** | Separate extraction system; Covalence stores knowledge, does not extract it | Never (separate system) |
| **P2P federation / multi-agent sync** | Out of deployment envelope; adds protocol complexity | v2+ |
| **Topology-derived embeddings (TopER)** | Requires PH computation; PostgreSQL has no native topology compute | v2+ research |
| **Intent auto-detection** | Lightweight classifier needed; explicit API params are v0 | v1 |
| **Canonical entity registry** | Requires deduplication pipeline; scoped to v1 | v1 |
| **Dependency-parser fast path** | spaCy integration; cost-benefit needs measurement | v1 |
| **Multi-agent namespace isolation** | Single-agent target for v0; schema must accommodate it | v1 |
| **Differential memory decay by function type** | Requires memory-type tagging system | v1 |

### 3.4 Coexistence Requirements

Covalence v0 **must coexist with Valence v2** on the same PostgreSQL instance during migration. Concretely:

- Covalence uses a dedicated schema: `covalence` (separate from Valence v2's `public`)
- Both systems share the same PostgreSQL 17 server (different connection pools)
- Covalence must not require AGE or pg_textsearch to be available in Valence v2's schema
- During coexistence, writes go only to one system at a time (no dual-write)
- The migration window is expected to be ≤ 48 hours for the current data volume

---

## 4. Architecture Overview

### 4.1 Layer Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                         AGENT INTERFACE                          │
│                                                                   │
│   ┌───────────────────────┐    ┌──────────────────────────────┐ │
│   │    OpenClaw Plugin    │    │       Go CLI (cov)           │ │
│   │  (MCP tool adapter)   │    │  cobra commands → REST calls │ │
│   └──────────┬────────────┘    └──────────────┬───────────────┘ │
└──────────────┼──────────────────────────────────┼───────────────┘
               │  HTTP/REST (OpenAPI 3.1)          │
┌──────────────▼──────────────────────────────────▼───────────────┐
│                      RUST ENGINE                                  │
│                  (covalence-engine crate)                         │
│                                                                   │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │                    REST API Layer                           │  │
│  │            (axum + tower + OpenAPI generation)             │  │
│  └──────────────────────────┬─────────────────────────────────┘  │
│                             │                                     │
│  ┌──────────────────────────▼─────────────────────────────────┐  │
│  │                  Query Engine                               │  │
│  │                                                             │  │
│  │  ┌─────────────────┐  ┌──────────────────────────────────┐ │  │
│  │  │  Query Planner  │  │       Result Fusion              │ │  │
│  │  │  (cost model +  │  │  (RRF + dimensional scores +     │ │  │
│  │  │   intent route) │  │   confidence × freshness weight) │ │  │
│  │  └────────┬────────┘  └──────────────────────────────────┘ │  │
│  │           │                                                  │  │
│  │  ┌────────▼──────────────────────────────────────────────┐  │  │
│  │  │              DimensionAdaptor Layer                    │  │  │
│  │  │                                                        │  │  │
│  │  │  ┌────────────┐  ┌────────────┐  ┌─────────────────┐  │  │  │
│  │  │  │   Vector   │  │   Lexical  │  │     Graph       │  │  │  │
│  │  │  │  Adaptor   │  │  Adaptor   │  │    Adaptor      │  │  │  │
│  │  │  │ (pgvector) │  │(pg_text/   │  │ (AGE/openCypher)│  │  │  │
│  │  │  │            │  │  ts_rank)  │  │                 │  │  │  │
│  │  │  └────────────┘  └────────────┘  └─────────────────┘  │  │  │
│  │  └────────────────────────────────────────────────────────┘  │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │              Dual-Stream Write Layer                         │  │
│  │                                                             │  │
│  │  Fast Path (algorithmic, synchronous):                      │  │
│  │    Ingest → fingerprint dedup → embedding → vector index    │  │
│  │    → temporal edge creation → queue slow-path tasks         │  │
│  │                                                             │  │
│  │  Slow Path (inference, async, background worker):           │  │
│  │    Dequeue → LLM compile → edge type inference              │  │
│  │    → contention detection → confidence update → log         │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │            Migration & Maintenance Layer                     │  │
│  │    (import, eviction, deduplication, score recompute)       │  │
│  └─────────────────────────────────────────────────────────────┘  │
└──────────────────────────────┬──────────────────────────────────┘
                               │ sqlx / deadpool-postgres
┌──────────────────────────────▼──────────────────────────────────┐
│                   PostgreSQL 17                                   │
│                                                                   │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────────────┐│
│  │ Apache AGE  │  │   pgvector   │  │   pg_textsearch (BM25)   ││
│  │  1.7.0      │  │   0.8.2      │  │   1.0.0-dev + ts_rank    ││
│  │  (graph     │  │  (halfvec,   │  │   fallback               ││
│  │   storage)  │  │   HNSW)      │  │                          ││
│  └─────────────┘  └──────────────┘  └──────────────────────────┘│
│                                                                   │
│  Schemas: covalence (Covalence tables) | public (Valence v2)     │
└──────────────────────────────────────────────────────────────────┘
```

### 4.2 The DimensionAdaptor Pattern (from SING)

The SING codebase (`postgresql-singularity`) solved the same core problem Covalence's query engine must solve: orchestrating AGE, PGVector, and pg_textsearch into a coherent scored result. Its `DimensionAdaptor` trait is the right abstraction. Covalence adopts it with modifications for the async application-layer context (SING was a PostgreSQL extension with in-process Spi calls; Covalence runs queries over a sqlx connection pool).

```rust
// covalence-engine/src/search/adaptor.rs
#[async_trait]
pub trait DimensionAdaptor: Send + Sync {
    fn name(&self) -> &'static str;
    
    /// Check if this dimension's backend extension is available.
    /// Called once at startup; failure is a hard error (except pg_textsearch,
    /// which falls back to ts_rank gracefully).
    fn check_availability(&self, pool: &PgPool) -> impl Future<Output = bool>;
    
    /// Execute this dimension's search.
    /// `candidates`: if Some, only consider these node IDs (cascade pre-filter).
    /// Returns results with raw (unnormalized) scores.
    async fn search(
        &self,
        pool: &PgPool,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> Result<Vec<DimensionResult>>;
    
    /// Normalize raw scores to [0.0, 1.0] (higher = better).
    /// Each adaptor implements its own normalization semantics:
    /// - Vector: 1.0 - cosine_distance (distance inverted to similarity)
    /// - Lexical: min-max normalize BM25 scores within the result set
    /// - Graph: normalize by max path score in the result set
    fn normalize_scores(&self, results: &mut [DimensionResult]);
    
    /// Static estimate of this dimension's selectivity [0.0, 1.0].
    /// Lower = more selective. Used by the query planner to determine
    /// cascade order (most selective runs first).
    fn estimate_selectivity(&self, query: &DimensionQuery) -> f64;
    
    /// Can this dimension run in parallel with others?
    /// Vector and lexical: yes (indexed, no mutual dependency).
    /// Graph: no (traversal must start from candidates found by prior dims).
    fn parallelizable(&self) -> bool;
}
```

**The cascade pre-filter is the critical performance mechanism.** SING's benchmarks demonstrate a 43.7× query latency degradation at 1M entities when dimensions are queried independently (no cascade). The pattern: lexical and vector dimensions run in parallel first (both have indexes, high selectivity), producing a candidate set. The graph dimension then runs a bounded traversal anchored to that candidate set — preventing O(n²) join explosions in the AGE traversal.

```
Without cascade: lexical_results ∪ vector_results ∪ graph_results
  → full-table operations, joined at application layer
  → 1,205ms at 1M entities

With cascade (lexical + vector parallel → graph on intersection):
  → graph traversal bounded to ~50–200 candidate nodes
  → estimated 30–80ms at 1M entities
```

For Covalence's target corpus (current: ~300 nodes; expected steady-state: 5K–50K nodes), the cascade pattern provides headroom for growth without architectural rework.

### 4.3 The Dual-Stream Write Architecture

Covalence formalizes what Valence v2 has informally — two distinct write paths with different latency, cost, and intelligence characteristics.

```
FAST PATH (synchronous, algorithmic, ≤ 100ms target)
─────────────────────────────────────────────────────
Source arrives via POST /sources

1. Fingerprint (SHA-256) → deduplicate
2. Parse metadata → create Source node in PG table
3. Generate embedding via OpenAI API → store as halfvec(1536)
4. Insert HNSW index entry
5. Create temporal edge to session node (if session context present)
6. Enqueue slow-path tasks:
   - article_compile (if enough sources on this topic)
   - edge_inference (causal/entity edge detection)
   - contention_check
7. Return Source node ID to caller

SLOW PATH (async, inference, seconds to minutes)
─────────────────────────────────────────────────
Background worker processes the slow-path queue:

1. LLM article compilation (selected sources → synthesized article)
2. LLM-assisted edge type inference (temporal, causal, entity edges)
3. Contention detection (embedding delta + NLI reasoning)
4. Confidence score recomputation from updated provenance
5. Log inference decision with inputs, outputs, and confidence
   → This log is the graduation training signal for v1+

The slow path produces durable side effects (new article nodes, typed edges,
contention records) but the fast path is never blocked by it.
```

This dual-stream model is validated by MAGMA ("Synaptic Ingestion" + "Structural Consolidation"), by the SAP paper's cascaded retrieval work, and by Covalence's own existing source_ingest / article_compile separation.

### 4.4 The Query Abstraction Layer (AGE Migration Hedge)

Apache AGE carries meaningful abandonment risk (see §11). All graph queries in Covalence MUST be isolated behind a `GraphRepository` abstraction. No AGE-specific syntax or agtype handling shall leak into the engine, API, or CLI layers.

```rust
// covalence-engine/src/graph/repository.rs

pub trait GraphRepository: Send + Sync {
    async fn find_neighbors(
        &self,
        node_id: Uuid,
        edge_labels: Option<&[&str]>,
        direction: TraversalDirection,
        depth: u32,
        limit: usize,
    ) -> Result<Vec<GraphNode>>;

    async fn create_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        label: &str,
        properties: serde_json::Value,
    ) -> Result<EdgeId>;

    async fn get_provenance_chain(
        &self,
        article_id: Uuid,
        max_depth: u32,
    ) -> Result<Vec<ProvenanceLink>>;

    async fn find_contradictions(
        &self,
        node_id: Uuid,
    ) -> Result<Vec<Contradiction>>;
}

// Implementation: AgeGraphRepository (v0)
// Future: SqlPgqGraphRepository (when SQL/PGQ lands in PG19+)
pub struct AgeGraphRepository {
    pool: PgPool,
    graph_name: &'static str, // "covalence"
}
```

The `AgeGraphRepository` translates these domain operations into AGE Cypher queries via sqlx. agtype is projected to native SQL types at the query boundary — no agtype propagates past the repository implementation.

---

## 5. v0 Scope — What We Build First

v0 is the minimum viable replacement for Valence v2 that is **better, not just different**. Every v0 feature must either fix a known Valence v2 failure mode or be required for functional equivalence. Nothing is included because it is interesting.

### 5.1 Node Types

| Type | Description | Key Fields |
|---|---|---|
| `Source` | Raw, immutable input material | content, type, fingerprint, reliability, metadata, embedding |
| `Article` | Compiled, mutable knowledge unit | content, title, version, confidence, epistemic_type, domain_path, embedding |
| `Session` | Conversation / task context boundary | session_id, platform, started_at, status |

**Not in v0:** `Entity` (canonical entity registry), `Agent` (multi-agent namespace), `Procedure` (structured how-to graphs)

### 5.2 Edge Types (Initial Vocabulary)

Edges are typed string labels stored in AGE. **No schema migration is required to add a new edge type** — a new label string is sufficient. The initial vocabulary covers the Valence v2 semantics plus the relationships that were previously inexpressible:

**Provenance edges (article→source, article→article)**
| Label | Semantics | Direction |
|---|---|---|
| `ORIGINATES` | Source directly contributed to article compilation | Source → Article |
| `CONFIRMS` | Source corroborates an existing article's claim | Source → Article |
| `SUPERSEDES` | This node replaces another node (directional) | New → Old |
| `CONTRADICTS` | These nodes make conflicting claims | Bidirectional |
| `EXTENDS` | This node elaborates without superseding | Child → Parent |
| `DERIVES_FROM` | Article derived from another article (e.g., post-split) | Derived → Source |
| `MERGED_FROM` | Article produced by merging these nodes | Merged → Each Parent |
| `SPLIT_INTO` | Article was divided; these are the fragments | Original → Each Fragment |

**Temporal edges**
| Label | Semantics | Direction |
|---|---|---|
| `PRECEDES` | Node A is temporally before Node B | Earlier → Later |
| `CONCURRENT_WITH` | Nodes reference overlapping time periods | Bidirectional |

**Causal / logical edges** (slow-path inferred; logged for graduation)
| Label | Semantics | Direction |
|---|---|---|
| `CAUSES` | LLM-inferred causal relationship | Cause → Effect |
| `MOTIVATED_BY` | This decision/action was motivated by this knowledge | Decision → Motivation |
| `IMPLEMENTS` | This concrete artifact implements this abstract concept | Concrete → Abstract |

**Session / entity edges**
| Label | Semantics | Direction |
|---|---|---|
| `CAPTURED_IN` | Source was captured during this session | Source → Session |
| `INVOLVES` | Node references this entity (v1 — entity nodes don't exist in v0) | Node → Entity |

**Edge properties (all edges carry):**
```json
{
  "created_at": "ISO8601 timestamp",
  "confidence": 0.0–1.0,
  "method": "algorithmic | llm_inferred | agent_explicit",
  "notes": "optional rationale string"
}
```

### 5.3 Retrieval Capabilities (v0)

| Capability | Implementation | v0 Target |
|---|---|---|
| Hybrid text+vector search | PGVector HNSW + pg_textsearch BM25 (ts_rank fallback), RRF fusion | ✅ |
| Graph neighborhood traversal | AGE Cypher, configurable depth (default 2, max 5) | ✅ |
| Provenance chain walk | AGE traversal along ORIGINATES/CONFIRMS/DERIVES_FROM | ✅ |
| Intent-aware retrieval | Explicit intent parameter in API (factual/temporal/causal/entity) | ✅ |
| Session-scoped search | WHERE session_id = $1 pre-filter applied to all adaptor SQL | ✅ |
| Confidence-weighted ranking | confidence_score multiplier in fusion after dimensional scores | ✅ |
| Freshness decay | Exponential decay function on modified_at, tunable λ | ✅ |
| Usage trace recording | Every retrieval hit appended to retrieval_events table | ✅ |
| Contention-aware search | Contention count surfaced in result metadata | ✅ |
| TopER structural similarity | Topological embedding signatures | ❌ v2+ |
| Intent auto-detection | NL classifier on query text | ❌ v1 |

### 5.4 Endpoints Required (v0)

Grouped by lifecycle stage (see §10 for full specification):

- **Sources:** `POST /sources`, `GET /sources/{id}`, `GET /sources`, `DELETE /sources/{id}`
- **Articles:** `POST /articles`, `GET /articles/{id}`, `PATCH /articles/{id}`, `DELETE /articles/{id}`, `POST /articles/{id}/split`, `POST /articles/merge`, `GET /articles/{id}/provenance`, `POST /articles/compile`
- **Edges:** `POST /edges`, `DELETE /edges/{id}`, `GET /nodes/{id}/edges`, `GET /nodes/{id}/neighborhood`
- **Search:** `POST /search` (hybrid, multi-dimensional), `GET /search/sources`
- **Sessions:** `POST /sessions`, `GET /sessions/{id}`, `POST /sessions/{id}/flush`
- **Admin:** `GET /admin/stats`, `POST /admin/maintenance`, `POST /admin/migrate`
- **Memory:** `POST /memory` (wrapper over source ingest with memory metadata), `POST /memory/search`, `PATCH /memory/{id}/forget`

### 5.5 What Makes v0 Better Than Valence v2 (The Minimum Bar)

| Problem in Valence v2 | v0 Solution | Measurement |
|---|---|---|
| Single `supersedes_id` pointer | Typed graph edges for SUPERSEDES, SPLIT_INTO, MERGED_FROM | All split/merge operations produce proper graph edges |
| Duplicate article accumulation | Topic identity check before compile; optional merge suggestion | Contention-flagged-as-duplicate rate should drop ≥ 80% |
| 26 false contention positives | Contention detection uses embedding delta + edge traversal, not naive cosine threshold | False positive rate < 10% on migration corpus |
| Fixed 5-type edge vocabulary | Extensible string labels | New edge type requires no migration |
| No graph traversal | AGE Cypher traversal behind GraphRepository | Provenance walk to 5 hops in < 500ms on migration corpus |
| Three confidence representations | Single `ConfidenceScore` struct per node | Zero drift possible (single source of truth) |
| Dead entity subsystem | Not migrated; entity graph is v1 | Cleaner schema from day 1 |
| No slow-path visibility | Slow-path queue exposed in `GET /admin/stats` | Queue depth, oldest job visible |
| session→source type mismatch | Both use UUID; Session nodes in graph | All links are FK-enforced |

---

## 6. Data Model

### 6.1 PostgreSQL Schema Layout

```
covalence.        -- Covalence schema (isolated from public/Valence v2)
  nodes           -- All nodes (Source, Article, Session) — relational store
  node_embeddings -- Vector embeddings (halfvec(1536)) — separate table for partial indexing
  edges_meta      -- Edge metadata mirror (AGE is canonical; this enables SQL queries)
  retrieval_events-- Usage trace per search hit
  slow_path_queue -- Async background work queue
  slow_path_log   -- Inference decisions log (graduation training signal)
  contentions     -- Detected contradictions (denormalized from edge graph for fast query)
  migration_log   -- Import provenance for coexistence validation

ag_catalog.       -- AGE extension tables (managed by AGE)
  [graph: covalence] -- AGE property graph for the covalence workspace
```

### 6.2 Node Table (Relational Backbone)

```sql
CREATE TABLE covalence.nodes (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    node_type       text NOT NULL CHECK (node_type IN ('source', 'article', 'session')),
    
    -- Content
    content         text,                          -- NULL for session nodes
    title           text,
    content_hash    char(64),                      -- SHA-256 for deduplication
    fingerprint     text UNIQUE,                   -- Domain-specific dedup key (sources)
    
    -- Classification
    epistemic_type  text DEFAULT 'semantic'        -- semantic | episodic | procedural
                    CHECK (epistemic_type IN ('semantic', 'episodic', 'procedural')),
    source_type     text,                          -- document|conversation|web|code|observation|tool_output|user_input
    domain_path     text[] DEFAULT '{}',
    
    -- Confidence (single canonical representation)
    confidence      real NOT NULL DEFAULT 0.5,     -- [0.0, 1.0]
    reliability     real NOT NULL DEFAULT 0.5,     -- source reliability prior
    
    -- Temporal
    created_at      timestamptz NOT NULL DEFAULT now(),
    modified_at     timestamptz NOT NULL DEFAULT now(),
    compiled_at     timestamptz,                   -- NULL for raw sources
    valid_from      timestamptz,
    valid_until     timestamptz,
    
    -- Status
    status          text NOT NULL DEFAULT 'active'
                    CHECK (status IN ('active', 'superseded', 'archived', 'disputed')),
    version         integer NOT NULL DEFAULT 1,
    pinned          boolean NOT NULL DEFAULT false,
    
    -- Provenance
    author_type     text DEFAULT 'system'
                    CHECK (author_type IN ('system', 'operator', 'agent')),
    session_id      uuid REFERENCES covalence.nodes(id) ON DELETE SET NULL,
    
    -- Scores (maintained by maintenance worker)
    usage_score     real NOT NULL DEFAULT 0.0,     -- retrieval frequency × recency
    
    -- Platform / session fields (session nodes)
    platform        text,
    platform_session_id text,
    
    -- Metadata escape hatch
    metadata        jsonb NOT NULL DEFAULT '{}',
    
    -- Full-text index (generated)
    content_tsv     tsvector GENERATED ALWAYS AS (
                        to_tsvector('english', COALESCE(title, '') || ' ' || COALESCE(content, ''))
                    ) STORED
);

-- BM25 index (pg_textsearch) — single column, English config
CREATE INDEX nodes_bm25_idx ON covalence.nodes
    USING bm25(content)
    WITH (text_config = 'english')
    WHERE content IS NOT NULL;

-- Native FTS fallback (always present)
CREATE INDEX nodes_fts_idx ON covalence.nodes
    USING gin(content_tsv)
    WHERE content IS NOT NULL;

-- Status + type for filtered retrieval
CREATE INDEX nodes_status_type_idx ON covalence.nodes (status, node_type);

-- Domain path for domain-scoped queries
CREATE INDEX nodes_domain_idx ON covalence.nodes USING gin(domain_path);

-- Session lookup
CREATE INDEX nodes_session_idx ON covalence.nodes (session_id) WHERE session_id IS NOT NULL;

-- Fingerprint (partial — sources only)
CREATE UNIQUE INDEX nodes_fingerprint_idx ON covalence.nodes (fingerprint)
    WHERE fingerprint IS NOT NULL;
```

### 6.3 Vector Embeddings (Separate Table)

Embeddings are stored in a dedicated table to enable clean partial indexing (not all nodes have embeddings) and to keep the main `nodes` table narrow for non-vector queries.

```sql
CREATE TABLE covalence.node_embeddings (
    node_id         uuid PRIMARY KEY REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    embedding       halfvec(1536) NOT NULL,         -- half-precision = 50% storage vs float32
    model           text NOT NULL DEFAULT 'text-embedding-3-small',
    embedded_at     timestamptz NOT NULL DEFAULT now()
);

-- HNSW index for approximate nearest neighbor search
-- halfvec_cosine_ops = cosine similarity (correct for text embeddings)
-- m=16 (connectivity), ef_construction=64 (build quality)
CREATE INDEX node_embeddings_hnsw_idx ON covalence.node_embeddings
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

**Memory estimate:** 1536 dims × 2 bytes (half-precision) = 3,072 bytes per embedding. At 50,000 nodes: ~150 MB for raw embeddings. HNSW index overhead: approximately 1.5× raw data = ~225 MB. Total at 50K nodes: ~375 MB — well within the 2 GB PostgreSQL budget.

### 6.4 Graph Storage in AGE

The AGE property graph stores **all relationship edges**. Node identity is anchored in the `covalence.nodes` table; AGE stores edge topology and properties.

```sql
-- Initialize Covalence property graph
SELECT create_graph('covalence');

-- AGE graph structure:
-- Vertex labels: Source, Article, Session
-- Vertices store: id (uuid as string), status (for graph-side filtering)
-- All rich properties live in covalence.nodes (SQL joins used for property access)

-- Edge labels: ORIGINATES, CONFIRMS, SUPERSEDES, CONTRADICTS, EXTENDS,
--              DERIVES_FROM, MERGED_FROM, SPLIT_INTO, PRECEDES,
--              CONCURRENT_WITH, CAPTURED_IN,
--              CAUSES, MOTIVATED_BY, IMPLEMENTS
-- Edge properties: created_at, confidence, method, notes

-- Example: Create a SUPERSEDES edge
SELECT * FROM cypher('covalence', $$
    MATCH (new:Article {id: $new_id}), (old:Article {id: $old_id})
    CREATE (new)-[:SUPERSEDES {
        created_at: $now,
        confidence: 1.0,
        method: 'agent_explicit'
    }]->(old)
$$, $params) AS (result agtype);
```

**Edge metadata mirror.** Because AGE Cypher cannot be composed with SQL WHERE clauses efficiently, a lightweight mirror table in SQL enables fast edge queries without Cypher overhead:

```sql
CREATE TABLE covalence.edges_meta (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    from_node_id    uuid NOT NULL REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    to_node_id      uuid NOT NULL REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    label           text NOT NULL,
    confidence      real NOT NULL DEFAULT 1.0,
    method          text NOT NULL DEFAULT 'algorithmic',
    created_at      timestamptz NOT NULL DEFAULT now(),
    notes           text,
    age_edge_id     bigint          -- AGE internal edge ID for sync
);

CREATE INDEX edges_meta_from_idx ON covalence.edges_meta (from_node_id, label);
CREATE INDEX edges_meta_to_idx ON covalence.edges_meta (to_node_id, label);
CREATE INDEX edges_meta_label_idx ON covalence.edges_meta (label);
```

AGE is canonical; the mirror is kept synchronous. Complex graph traversal uses AGE/Cypher. Simple edge lookups ("does this edge exist?", "list edges from this node") use the SQL mirror.

### 6.5 Slow-Path Queue and Inference Log

```sql
CREATE TABLE covalence.slow_path_queue (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    operation       text NOT NULL,  -- compile|edge_infer|contention_check|decay|recompute_score
    node_ids        uuid[] NOT NULL DEFAULT '{}',
    payload         jsonb NOT NULL DEFAULT '{}',
    priority        integer NOT NULL DEFAULT 5,
    status          text NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'processing', 'completed', 'failed')),
    attempts        integer NOT NULL DEFAULT 0,
    max_attempts    integer NOT NULL DEFAULT 3,
    created_at      timestamptz NOT NULL DEFAULT now(),
    started_at      timestamptz,
    completed_at    timestamptz,
    error           text
);

-- Graduation training signal: every inference decision is logged
CREATE TABLE covalence.slow_path_log (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    operation       text NOT NULL,
    input_snapshot  jsonb NOT NULL,     -- inputs to the inference call
    output          jsonb NOT NULL,     -- resulting decision
    confidence      real,
    model           text,               -- which LLM was used
    latency_ms      integer,
    created_at      timestamptz NOT NULL DEFAULT now()
);
```

The `slow_path_log` is the **graduation training dataset**. When patterns in inference decisions become frequent and high-confidence, they can be promoted to fast-path algorithms in v1.

### 6.6 Retrieval Events (Usage Traces)

```sql
CREATE TABLE covalence.retrieval_events (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    query_hash      char(16) NOT NULL,  -- blake3 truncated for grouping
    node_id         uuid NOT NULL REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    rank_position   integer,
    dimensional_scores jsonb,           -- {vector: 0.82, lexical: 0.61, graph: 0.44}
    composite_score real,
    intent          text,               -- factual|temporal|causal|entity
    session_id      uuid,
    retrieved_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX retrieval_events_node_idx ON covalence.retrieval_events (node_id, retrieved_at DESC);
```

---

## 7. Retrieval Architecture

### 7.1 Three-Dimensional Retrieval

Covalence retrieval operates across three orthogonal dimensions, each implemented as a `DimensionAdaptor`:

| Dimension | Adaptor | Backend | What It Finds |
|---|---|---|---|
| **Semantic** | `VectorAdaptor` | pgvector HNSW on halfvec(1536) | Conceptually similar nodes regardless of lexical form |
| **Lexical** | `LexicalAdaptor` | pg_textsearch BM25 (ts_rank fallback) | Exact or near-exact keyword matches |
| **Graph** | `GraphAdaptor` | AGE openCypher traversal | Structurally related nodes via typed edge traversal |

### 7.2 Cascade Pre-Filtering (Critical)

SING's benchmark data makes cascade pre-filtering non-negotiable:

```
Scale    | No cascade  | With cascade (est.) | Speedup
─────────────────────────────────────────────────────
10K nodes | 27.6ms     | ~5ms                | 5.5×
100K nodes| 288.5ms    | ~15ms               | 19×
1M nodes  | 1,205ms    | ~35ms               | 34×
```

The 43.7× degradation from 10K→1M without cascade demonstrates that each dimension's independent full-table scan combines multiplicatively, not additively.

**Execution order (planner-determined):**

```
Step 1 (parallel): LexicalAdaptor + VectorAdaptor
   Both have high-quality indexes (BM25 + HNSW)
   Both are parallelizable (no mutual dependency)
   Run concurrently via tokio::try_join!
   → candidate_set = union of top-N results from each

Step 2 (sequential): GraphAdaptor
   Takes candidate_set as input (WHERE id = ANY($candidates))
   Traverses only from anchor nodes in candidate_set
   → Returns graph-adjacent nodes not found in steps 1/2

Step 3: ResultFusion
   Merge results from all three dimensions
   Apply DimensionWeights normalization
   Apply confidence × freshness multipliers
   Apply usage_score boost
   Sort by composite score, return top-K
```

**Intent routing modifies edge weight selection in Step 2:**

| Intent | AGE Edge Priority | Example Query |
|---|---|---|
| `factual` | CONFIRMS, ORIGINATES (provenance) | "What is the Covalence data model?" |
| `temporal` | PRECEDES, FOLLOWS, CONCURRENT_WITH | "What happened after the AGE migration?" |
| `causal` | CAUSES, MOTIVATED_BY, IMPLEMENTS | "Why did we choose PostgreSQL over SQLite?" |
| `entity` | INVOLVES, CAPTURED_IN | "Everything about the M4 Mac Mini deployment" |
| `(none)` | All edge types equal weight | Generic recall |

In v0, intent is **explicit in the API** (`intent` parameter on `POST /search`). Auto-detection via lightweight query classifier is a v1 feature.

### 7.3 Score Normalization and Fusion

Each adaptor normalizes its raw scores to `[0.0, 1.0]` (higher = better) before fusion:

```
VectorAdaptor:   normalized = 1.0 - cosine_distance   (distance → similarity)
LexicalAdaptor:  normalized = (score - min) / (max - min)  (min-max within result set)
GraphAdaptor:    normalized = 1.0 / (1.0 + path_cost)  (shorter paths score higher)
```

Fusion via weighted mean over whichever dimensions returned results for each node:

```rust
pub struct DimensionWeights {
    pub vector:  f32,  // default: 0.50
    pub lexical: f32,  // default: 0.30
    pub graph:   f32,  // default: 0.20
}

// Normalize weights on input (user can pass {vector:2, lexical:1, graph:0})
impl DimensionWeights {
    pub fn normalize(&mut self) {
        let sum = self.vector + self.lexical + self.graph;
        if sum > 0.0 {
            self.vector /= sum;
            self.lexical /= sum;
            self.graph /= sum;
        }
    }
}

// Final score formula (preserving Valence v2's proven weighting):
let dimensional_score = weighted_mean(vector_score, lexical_score, graph_score, weights);
let freshness = (-DECAY_RATE * days_since_modified).exp();
let final_score = dimensional_score * 0.50
                + node.confidence * 0.35
                + freshness * 0.15;

// Novelty boost for new nodes (decays over 48h, max 1.5×)
let novelty = if hours_since_created < 48.0 {
    1.0 + 0.5 * (1.0 - hours_since_created / 48.0)
} else { 1.0 };

let final_score = final_score * novelty;
```

This formula is carried forward from Valence v2, where the 0.50/0.35/0.15 weights were empirically tuned. They are configurable per-query via the `weights` parameter.

### 7.4 pg_textsearch + FTS Fallback

pg_textsearch (Tiger Data BM25) is preview-quality (v1.0.0-dev). The system must function correctly when BM25 is unavailable:

```rust
pub struct LexicalAdaptor {
    bm25_available: AtomicBool,    // set at startup by check_availability()
}

impl LexicalAdaptor {
    async fn search_bm25(&self, pool: &PgPool, query: &str, limit: usize) -> Result<Vec<DimensionResult>> {
        // Uses pg_textsearch <@> operator + BM25 index
        sqlx::query!(
            "SELECT id, content <@> $1 AS bm25_score FROM covalence.nodes
             WHERE content IS NOT NULL AND status = 'active'
             ORDER BY content <@> $1 LIMIT $2",
            query, limit as i64
        ).fetch_all(pool).await
    }

    async fn search_fts_fallback(&self, pool: &PgPool, query: &str, limit: usize) -> Result<Vec<DimensionResult>> {
        // Falls back to native PostgreSQL ts_rank (always available)
        sqlx::query!(
            "SELECT id, ts_rank(content_tsv, websearch_to_tsquery('english', $1)) AS ts_score
             FROM covalence.nodes
             WHERE content_tsv @@ websearch_to_tsquery('english', $1) AND status = 'active'
             ORDER BY ts_score DESC LIMIT $2",
            query, limit as i64
        ).fetch_all(pool).await
    }
}
```

The fallback is transparent to callers. The `explain` response field indicates which backend was used.

### 7.5 Reciprocal Rank Fusion (SQL Implementation)

RRF fusion runs in a single SQL query using CTEs. This keeps all ranking computation in PostgreSQL without application-layer sort overhead:

```sql
WITH
vector_results AS (
    SELECT ne.node_id AS id,
           ROW_NUMBER() OVER (ORDER BY ne.embedding <=> $query_vec) AS rank
    FROM covalence.node_embeddings ne
    JOIN covalence.nodes n ON n.id = ne.node_id
    WHERE n.status = 'active'
      AND ($session_id IS NULL OR n.session_id = $session_id)
    ORDER BY ne.embedding <=> $query_vec
    LIMIT $candidate_limit
),
lexical_results AS (
    SELECT id,
           ROW_NUMBER() OVER (ORDER BY content <@> $query_text) AS rank
    FROM covalence.nodes
    WHERE content IS NOT NULL AND status = 'active'
      AND ($session_id IS NULL OR session_id = $session_id)
    ORDER BY content <@> $query_text
    LIMIT $candidate_limit
),
fused AS (
    SELECT
        COALESCE(v.id, l.id) AS id,
        COALESCE(1.0 / (60.0 + v.rank), 0.0) * $vector_weight +
        COALESCE(1.0 / (60.0 + l.rank), 0.0) * $lexical_weight AS rrf_score
    FROM vector_results v
    FULL OUTER JOIN lexical_results l ON v.id = l.id
)
SELECT f.id, f.rrf_score,
       n.confidence, n.modified_at, n.epistemic_type
FROM fused f
JOIN covalence.nodes n ON n.id = f.id
ORDER BY f.rrf_score DESC
LIMIT $final_limit;
```

The graph dimension result is merged at the application layer with the SQL RRF result (since graph traversal via AGE must be a separate query). The merged set is re-ranked by the `final_score` formula in §7.3.

### 7.6 Query Explain Mode

The API supports `?explain=true` on `POST /search`, returning:

```json
{
  "results": [...],
  "explain": {
    "intent": "causal",
    "execution_order": ["lexical", "vector", "graph"],
    "parallel_groups": [["lexical", "vector"], ["graph"]],
    "backends": {
      "lexical": "pg_textsearch_bm25",
      "vector": "pgvector_hnsw_halfvec",
      "graph": "apache_age_cypher"
    },
    "timing_ms": {
      "lexical": 3.2,
      "vector": 4.8,
      "graph": 18.4,
      "fusion": 0.6,
      "total": 27.0
    },
    "candidate_counts": {
      "after_lexical": 87,
      "after_vector": 41,
      "after_graph_expansion": 62,
      "final": 10
    },
    "weights_used": {
      "vector": 0.50,
      "lexical": 0.30,
      "graph": 0.20
    }
  }
}
```

This is essential for debugging retrieval quality and for future tuning of the planner cost model.

---

## 8. Inference vs. Algorithm Boundary

This section defines precisely what runs in the fast algorithmic path vs. the slow inference path, and what data is logged in v0 for future graduation.

### 8.1 Fast Path (Algorithmic) — Rules for Inclusion

A computation belongs in the fast path if and only if it satisfies ALL of:
1. Deterministic given the same inputs
2. No LLM call required
3. Completes in ≤ 100ms on target hardware
4. Does not require reading the full corpus (is bounded)

| Operation | Path | Implementation |
|---|---|---|
| Source fingerprint deduplication | Fast | SHA-256 comparison against unique index |
| Embedding generation | Fast | OpenAI API call (async, ~200ms — accept this latency in v0) |
| Vector index insertion | Fast | pgvector INSERT |
| BM25 index update | Fast | pg_textsearch index (batched on write) |
| AGE vertex creation | Fast | Cypher CREATE vertex |
| CAPTURED_IN session edge | Fast | Algorithmic: session context is always known at ingest time |
| PRECEDES / CONCURRENT_WITH temporal edges | Fast | Algorithmic: compare created_at timestamps |
| Retrieval scoring (dimensional fusion) | Fast | Pure arithmetic on retrieved scores |
| Freshness decay computation | Fast | exp(-λ × days) |
| Novelty boost computation | Fast | Linear decay from created_at |
| Usage score increment | Fast | Atomic counter update on retrieval_events insert |
| Organic forgetting (eviction) | Fast | Sort by usage_score; archive lowest-N |
| Contention lookup | Fast | SQL/AGE edge query (reads existing edges) |

### 8.2 Slow Path (Inference) — Rules for Inclusion

A computation belongs in the slow path if it requires ANY of:
- An LLM API call
- Reading and reasoning over multiple full node contents
- Probabilistic output that must be logged
- Human judgment simulation

| Operation | Path | LLM Role | Log Target |
|---|---|---|---|
| Article compilation from sources | Slow | Synthesizes article content from source texts | inputs: source_ids, source_contents; output: article_content |
| Causal edge inference (CAUSES, MOTIVATED_BY) | Slow | Determines if A caused B given their contents | inputs: node_ids, contents; output: edge_label, confidence |
| IMPLEMENTS edge inference | Slow | Determines if concrete artifact implements abstract concept | Same |
| Contention detection | Slow | NLI: do these two nodes contradict? | inputs: node_ids, contents; output: contradicts_bool, severity |
| Contention resolution | Slow | Which node should supersede? | inputs: contention_id, both node_contents; output: resolution |
| Relevance-based merge suggestion | Slow | Should these two nodes be merged? | inputs: node_ids, embeddings, contents; output: merge_bool, rationale |
| Article recompilation (post-update) | Slow | Resynthesize from updated source set | Same as compile |
| Confidence recomputation (complex) | Slow | Assess confidence given new contradicting evidence | inputs: article_id, contention_id; output: new_confidence |

### 8.3 Graduation Data Logging (v0 → v1 Pathway)

Every slow-path inference decision is written to `covalence.slow_path_log`:

```json
{
  "operation": "causal_edge_inference",
  "input_snapshot": {
    "source_id": "uuid-A",
    "target_id": "uuid-B",
    "source_content_excerpt": "...first 500 chars...",
    "target_content_excerpt": "...first 500 chars...",
    "embedding_cosine_similarity": 0.74,
    "shared_domain_path": ["covalence", "architecture"]
  },
  "output": {
    "edge_label": "CAUSES",
    "confidence": 0.81,
    "rationale": "Source A describes the problem that motivated target B's design decision"
  },
  "model": "gpt-4o",
  "latency_ms": 1420,
  "created_at": "2026-03-01T12:34:56Z"
}
```

After accumulating ~500 examples per operation type, patterns can be extracted:
- **Heuristic rules:** If cosine_similarity > 0.85 AND shared_domain_path AND source_A.created_at < source_B.created_at, the PRECEDES edge can be created algorithmically with high confidence.
- **Fine-tuned classifier:** Train a lightweight binary classifier (edge_label prediction) on the log, deploy as a fast-path computation.
- **Feature extraction rules:** If node_B's content contains "because", "therefore", "in response to" AND mentions node_A's entity names, infer MOTIVATED_BY algorithmically.

The v0 goal is **data collection, not automation**. The graduation process begins in v1.

### 8.4 What is Never Automatic

Some decisions must always involve the agent or a human reviewer:
- Contention **resolution** (which article wins a contradiction) — agent decides
- `admin_forget` (hard delete) — agent or operator only
- Article pinning — agent explicit
- Edge type correction (relabeling an inferred edge) — agent explicit

---

## 9. Migration Strategy

### 9.1 Current Data Volume

| Entity | Count | Migration Priority |
|---|---|---|
| Sources | 264 | Critical |
| Active articles | 126 | Critical |
| All articles (incl. archived/superseded) | 289 | High (for provenance chain reconstruction) |
| `article_sources` provenance links | ~500+ (estimated) | Critical |
| Open contentions | 26 | Medium |
| Memories (sources with metadata.memory=true) | ~40 | High |
| Sessions | Unknown (active only) | Low |
| Usage traces | Unknown | Low (reset acceptable) |

Total migration runtime estimate: < 30 minutes for data extract/transform/load; embedding regeneration adds 264 × ~200ms ≈ 53 minutes for sources. Use existing embeddings from Valence v2 where model matches (text-embedding-3-small, 1536 dims) to avoid regeneration cost.

### 9.2 Supersession Chain Reconstruction

The core migration challenge. Valence v2 uses doubly-linked list pointers; Covalence uses typed graph edges.

**Algorithm:**

```python
# Migration step: walk all supersedes_id chains, emit graph edges

# 1. Build chain maps from articles
for article in valence_articles:
    if article.supersedes_id:
        emit_edge(
            from_node=article.id,          # newer article
            to_node=article.supersedes_id, # older article it replaced
            label="SUPERSEDES",
            properties={"created_at": article.created_at, "method": "migration_reconstructed"}
        )
    if article.source_id:  # legacy single-source link
        emit_edge(
            from_node=article.source_id,
            to_node=article.id,
            label="ORIGINATES",
            properties={"method": "migration_reconstructed"}
        )

# 2. Migrate article_sources (the real graph edges)
for link in valence_article_sources:
    emit_edge(
        from_node=link.source_id,
        to_node=link.article_id,
        label=RELATIONSHIP_MAP[link.relationship],  # originates→ORIGINATES, etc.
        properties={"created_at": link.added_at, "notes": link.notes}
    )

# 3. Contentions → CONTRADICTS edges
for contention in valence_contentions WHERE status != 'resolved':
    emit_edge(
        from_node=contention.article_id,
        to_node=contention.related_article_id,
        label="CONTRADICTS",
        properties={"severity": contention.severity, "description": contention.description}
    )

RELATIONSHIP_MAP = {
    "originates":  "ORIGINATES",
    "confirms":    "CONFIRMS",
    "supersedes":  "SUPERSEDES",
    "contradicts": "CONTRADICTS",
    "contends":    "EXTENDS",   # closest semantic match
}
```

### 9.3 Confidence Canonicalization

Valence v2 stores confidence in three representations (JSONB blob, six float columns, denormalized corroborating_sources). The migration script recomputes confidence from first principles:

```python
def compute_confidence(article, sources):
    if not sources:
        return 0.5  # default prior

    avg_reliability = mean(s.reliability for s in sources)
    n = len(sources)
    source_bonus = min(0.15, math.log(1 + n - 1) * 0.1) if n > 1 else 0
    return min(0.95, avg_reliability + source_bonus)
```

This produces a single canonical `confidence` float per migrated node.

### 9.4 Coexistence Architecture

During migration:

```
PostgreSQL 17 instance
├── public schema          → Valence v2 (unchanged, read-write until cutover)
├── covalence schema       → Covalence (write-enabled after migration import)
└── ag_catalog             → AGE property graph

Connection pools:
├── Valence v2 pool:   public.articles, public.sources, public.article_sources...
└── Covalence pool:    covalence.nodes, covalence.edges_meta... + AGE graph 'covalence'
```

No cross-schema writes. No triggers that bridge the two systems. If AGE is not yet installed on the Valence v2 server, Covalence migration is blocked until AGE is installed — this is a deployment coordination requirement, not a code problem.

### 9.5 Migration Steps

```
Step 0: Install extensions
  → Install AGE 1.7.0 on PostgreSQL 17 instance
  → Install pg_textsearch 1.0.0-dev on PostgreSQL 17 instance
  → Add both to shared_preload_libraries; restart PostgreSQL
  → Verify: SELECT create_graph('covalence'); (dry-run; rollback)

Step 1: Schema creation
  → Run Covalence Sqlx migrations to create covalence.* tables
  → Run SELECT create_graph('covalence'); (persistent)

Step 2: Dry-run
  → cov migrate --dry-run --source-dsn $VALENCE_DSN --target-dsn $COVALENCE_DSN
  → Output: node counts, edge counts, detected chain anomalies
  → Review any circular supersession chains or orphaned sources

Step 3: Migration execute
  → cov migrate --execute --source-dsn $VALENCE_DSN --target-dsn $COVALENCE_DSN
  → Imports all nodes, recomputes confidence, reconstructs graph edges
  → Reuses existing embeddings where model matches (skip re-embedding)
  → Logs every imported entity to covalence.migration_log

Step 4: Validation
  → cov migrate --validate
  → Checks: node count parity, zero orphaned edges, all active articles have provenance
  → Generates validation report

Step 5: Cutover
  → Agent switches REST endpoint from Valence v2 MCP to Covalence REST + Go CLI
  → Valence v2 MCP server enters read-only mode (configuration flag)
  → Covalence is live

Step 6: Post-cutover observation (30 days)
  → Valence v2 PostgreSQL tables preserved in public schema
  → Agent can query historical data via direct SQL if needed
  → After 30 days: archive or drop public schema
```

### 9.6 Cutover Criteria

Migration is considered complete and cutover is safe when:
- [ ] `cov migrate --validate` reports zero errors
- [ ] All 264 sources imported (node_type='source' count matches)
- [ ] All 289 articles imported (status preserved: active/archived/superseded)
- [ ] Every article with article_sources links has corresponding edges in AGE
- [ ] At least 95% of existing embeddings reused (text-embedding-3-small 1536-dim match)
- [ ] `GET /admin/stats` reports 0 pending slow-path queue items (or < 10, all non-critical)
- [ ] Hybrid search returns expected top-3 results for 10 known good queries (manual QA)
- [ ] p99 search latency ≤ 500ms on the full migrated corpus (automated benchmark)

---

## 10. API Surface (REST/OpenAPI)

### 10.1 Design Principles

- **REST with OpenAPI 3.1.** Generated from Rust types via `utoipa` or `aide`.
- **Plural resource nouns.** `/sources`, `/articles`, `/edges`, `/sessions`
- **Consistent envelope.** All responses: `{"data": ..., "meta": {...}}` or `{"error": {"code": ..., "message": ...}}`
- **UUID identifiers.** All resource IDs are UUIDs.
- **Pagination.** All list endpoints support `?limit=` and `?cursor=` (keyset pagination).
- **No MCP in v0.** The Go CLI wraps REST calls; OpenClaw plugin calls the CLI.

### 10.2 Mapping Valence v2 MCP Tools → REST Endpoints

| Valence v2 Tool | Covalence REST | Notes |
|---|---|---|
| `source_ingest` | `POST /sources` | Same semantics; fingerprint dedup preserved |
| `source_get` | `GET /sources/{id}` | |
| `source_search` | `GET /sources?q=` | |
| `source_list` | `GET /sources?type=` | |
| `knowledge_search` | `POST /search` | Upgraded: 3-dimensional, intent-aware |
| `article_get` | `GET /articles/{id}` | |
| `article_create` | `POST /articles` | |
| `article_compile` | `POST /articles/compile` | Queues slow-path compile; returns job ID |
| `article_update` | `PATCH /articles/{id}` | |
| `article_split` | `POST /articles/{id}/split` | Returns two new article IDs + SPLIT_INTO edges |
| `article_merge` | `POST /articles/merge` | Body: {article_id_a, article_id_b}; returns merged ID + MERGED_FROM edges |
| `article_search` | `POST /search` with `node_type=article` | Unified search endpoint |
| `provenance_trace` | `GET /articles/{id}/provenance?claim=` | TF-IDF claim attribution |
| `provenance_get` | `GET /articles/{id}/provenance` | |
| `provenance_link` | `POST /edges` | Generic edge creation |
| `contention_list` | `GET /contentions` | |
| `contention_resolve` | `POST /contentions/{id}/resolve` | |
| `contention_detect` | `POST /contentions/detect` | |
| `admin_forget` | `DELETE /sources/{id}` or `DELETE /articles/{id}` | |
| `admin_stats` | `GET /admin/stats` | Extended: includes slow-path queue depth |
| `admin_maintenance` | `POST /admin/maintenance` | |
| `memory_store` | `POST /memory` | Wrapper: POST /sources with memory metadata |
| `memory_recall` | `POST /memory/search` | Wrapper: POST /search with memory filter |
| `memory_status` | `GET /memory/status` | |
| `memory_forget` | `PATCH /memory/{id}/forget` | Soft delete (sets metadata.forgotten) |
| `session_start` | `POST /sessions` | |
| `session_append` | `POST /sessions/{id}/messages` | |
| `session_flush` | `POST /sessions/{id}/flush` | |
| `session_finalize` | `POST /sessions/{id}/finalize` | |
| `session_list` | `GET /sessions` | |
| `session_get` | `GET /sessions/{id}` | |
| `session_compile` | `POST /sessions/{id}/compile` | |

### 10.3 New Endpoints (No Valence v2 Equivalent)

| Endpoint | Description |
|---|---|
| `GET /nodes/{id}/neighborhood` | Graph traversal: BFS from node, configurable depth + edge label filter |
| `GET /nodes/{id}/edges` | List all edges from/to a node with label and confidence |
| `POST /edges` | Create a typed edge between any two nodes |
| `DELETE /edges/{id}` | Remove a specific edge |
| `GET /admin/queue` | Inspect slow-path queue (depth, oldest job, by operation type) |
| `GET /admin/queue/{job_id}` | Status of specific slow-path job |
| `POST /admin/migrate` | Trigger migration from Valence v2 (dry-run or execute mode) |
| `GET /admin/migrate/status` | Migration validation report |
| `POST /search` with `?explain=true` | Full retrieval explanation (timing, candidate counts, backends) |
| `GET /articles/{id}/versions` | Full version history of an article |
| `POST /articles/compile` (async) | Returns 202 Accepted + job ID; poll `GET /admin/queue/{job_id}` |

### 10.4 Key Request/Response Schemas

**`POST /search` request:**
```json
{
  "query": "why did we choose PostgreSQL over SQLite?",
  "intent": "causal",
  "limit": 10,
  "node_types": ["article"],
  "weights": {"vector": 0.5, "lexical": 0.3, "graph": 0.2},
  "temporal_preset": "default",
  "session_id": null,
  "include_sources": false,
  "explain": false
}
```

**`POST /search` response item:**
```json
{
  "id": "uuid",
  "node_type": "article",
  "title": "Why PostgreSQL over SQLite for Covalence",
  "content": "...",
  "confidence": 0.84,
  "epistemic_type": "semantic",
  "domain_path": ["covalence", "architecture", "storage"],
  "scores": {
    "composite": 0.79,
    "dimensional": {"vector": 0.88, "lexical": 0.61, "graph": 0.72},
    "freshness": 0.92,
    "usage_score": 1.34
  },
  "contention_count": 0,
  "modified_at": "2026-02-15T10:23:00Z"
}
```

**`POST /edges` request:**
```json
{
  "from_node_id": "uuid-A",
  "to_node_id": "uuid-B",
  "label": "CAUSES",
  "confidence": 0.72,
  "method": "llm_inferred",
  "notes": "A's decision about storage format caused B's migration complexity"
}
```

**`GET /nodes/{id}/neighborhood` request params:**
```
?depth=2          (default: 2, max: 5)
&direction=both   (outbound | inbound | both)
&labels=SUPERSEDES,CONFIRMS  (optional edge label filter)
&limit=50         (max nodes returned)
```

---

## 11. Risk Register

### 11.1 AGE Project Abandonment

**Risk:** Apache AGE enters dormancy or is formally retired, leaving Covalence dependent on an unmaintained C extension in a production PostgreSQL instance.

**Evidence:** GitHub discussion #2150 ("What's the Status of Apache AGE?") documents explicit community concern. Apache board minutes characterize the project as "low activity." The PG17 branch had no release for an extended period before 1.6.0 in mid-2025.

**Severity:** High — AGE is a C extension; a crash in AGE crashes the PostgreSQL backend process.

**Likelihood:** Medium — AGE 1.7.0 was released February 2026 with PG17 + PG18 support. The project is slow but not dead.

**Mitigation (already incorporated in architecture):**
1. `GraphRepository` trait abstracts ALL Cypher queries — no AGE syntax leaks past the repository implementation
2. `edges_meta` SQL mirror table makes basic edge queries executable without AGE
3. Schema is simple (vertex labels = node types; edge labels = typed strings) — directly mappable to SQL/PGQ syntax
4. Monitor pgsql-hackers for SQL/PGQ commitfest status (expected PG19 or PG20)
5. If AGE becomes a problem before SQL/PGQ ships: `edges_meta` + recursive CTEs can serve as a fallback graph layer

**Trigger for migration:** AGE fails to release for PG18 support within 90 days of PG18 GA, OR a critical security vulnerability is disclosed with no patch activity within 30 days.

### 11.2 pg_textsearch Preview Quality

**Risk:** pg_textsearch 1.0.0-dev has bugs that cause incorrect search results, index corruption, or PostgreSQL crashes in production use.

**Evidence:** Explicitly labeled "preview / early access." Not yet GA. The extension requires `shared_preload_libraries` — loading it cannot be done dynamically, and a broken version requires a PostgreSQL restart to remove.

**Severity:** Medium — search quality degrades but the system remains operational (FTS fallback).

**Likelihood:** High — preview software has bugs. High probability of encountering at least one within 6 months.

**Mitigation (already incorporated in architecture):**
1. `LexicalAdaptor` has a feature flag: `BM25_ENABLED` environment variable
2. `check_availability()` runs at startup — if BM25 fails a smoke test query, it switches to FTS fallback transparently
3. `ts_rank` FTS fallback is always present and maintained as a tested code path
4. pg_textsearch is PostgreSQL-licensed (no commercial licensing risk even if we drop it)
5. Watch Tiger Data repo for GA announcement; upgrade immediately when released

**Acceptance test:** On every startup, `LexicalAdaptor` runs `SELECT 1 FROM nodes LIMIT 0 WHERE content <@> 'test'` — if it fails, BM25 is disabled, FTS activated, and a health check warning is emitted.

### 11.3 AGE Deep Traversal Performance

**Risk:** Graph traversal queries with depth > 2 degrade unacceptably because AGE translates each hop into a SQL join — no index-free adjacency.

**Evidence:** Community consensus is that AGE performs well for depth-1 and depth-2. Depth-3+ traversal requires three-table joins — effectively a cartesian product scan.

**Severity:** High — if provenance walks or contention detection require deep traversal, query latency could be 5–30 seconds on a 50K-node graph.

**Likelihood:** High for depth ≥ 3, guaranteed.

**Mitigation:**
1. Default traversal depth: 2. Maximum exposed via API: 5. Warn in documentation that depth > 3 is expensive.
2. `edges_meta` SQL mirror enables `WITH RECURSIVE` CTEs as an alternative to Cypher for fixed-pattern traversals (provenance chain walking is one such fixed pattern)
3. Design materialized path pattern: for the most common traversal (provenance chain for an article), precompute and cache the chain as a JSON array on the article node on compile
4. Benchmark: add graph traversal depth benchmark to CI. If depth-3 exceeds 500ms on a 10K-node corpus, switch that query to `WITH RECURSIVE`

### 11.4 Scope Creep

**Risk:** Feature creep during v0 implementation extends the timeline beyond acceptable bounds.

**Specific vectors:**
- "Let's just add entity nodes while we're building the graph schema"
- "Intent auto-detection is so small, let's include it"
- "The dependency parser fast path is only a week of work"

**Severity:** High — scope creep on a new substrate is how migrations fail.

**Mitigation:**
1. §3.3 (Scope Exclusions) is a **hard firewall**, not a suggestion. Adding any excluded feature to v0 requires a written design decision signed by Jane.
2. Maintain a "parking lot" document for valid ideas that arrive during v0 implementation — they go in the parking lot, not in the sprint.
3. v0 is done when §13 Success Criteria are met — not when all features the team thought of are built.

### 11.5 Migration Divergence

**Risk:** During the coexistence window, the agent makes decisions in Valence v2 that are structurally impossible to represent in Covalence (e.g., a new contention resolution that relies on an undocumented field), causing migration to produce an inconsistent state.

**Likelihood:** Low — the migration script is a read-transform-write operation on a well-understood schema.

**Mitigation:**
1. Migration dry-run (`cov migrate --dry-run`) runs against the live Valence v2 schema and reports any unmappable constructs before cutover
2. Post-migration validation (`cov migrate --validate`) verifies every active article has a reconstructed provenance graph in Covalence
3. Valence v2 remains live read-only for 30 days post-cutover as an audit reference
4. The cutover window is expected to be a planned maintenance event, not a continuous dual-write period — this eliminates the concurrent-write divergence class of problems

### 11.6 OpenAI Embedding API Dependency

**Risk:** OpenAI API is unavailable at source ingest time, causing sources to be stored without embeddings and degrading semantic search recall.

**Severity:** Medium — search degrades but the system remains operational (FTS fallback).

**Likelihood:** Medium — API outages happen.

**Mitigation:**
1. Failed embedding jobs are enqueued in `slow_path_queue` with operation=`embed`
2. Maintenance worker retries embedding on the next cycle (exponential backoff, max 3 attempts)
3. `GET /admin/stats` reports "nodes_without_embeddings" count as a health signal
4. Architecture allows swapping the embedder behind a trait — Ollama or a bundled ONNX model can serve as fallback in v1

---

## 12. Roadmap

### 12.1 v0 — Foundation (This Spec)

**Theme:** Replace Valence v2 without breaking the agent contract. Better graph model, better retrieval, same API surface.

**Key deliverables:**
- PostgreSQL 17 + AGE + PGVector + pg_textsearch stack, Dockerized
- Rust engine with DimensionAdaptor layer, dual-stream writes, REST API
- Go CLI wrapping all REST endpoints
- Three-dimensional hybrid retrieval with RRF and cascade pre-filtering
- Typed graph edges with initial vocabulary (12 labels)
- Slow-path queue with inference decision logging
- Migration tooling (dry-run + execute + validate)
- Full Valence v2 API parity + new graph endpoints

**Out:** Intent auto-detection, entity nodes, canonical entity registry, Plasmon, P2P, MCP, TopER, dependency-parser fast path.

### 12.2 v1 — Intelligence Layer

**Theme:** Make the slow path smarter and graduate its first patterns to algorithms.

**Key deliverables:**
- **Intent auto-detection:** Lightweight query classifier (embedding + keyword heuristics) automatically assigns intent without explicit API parameter
- **Canonical entity registry:** Entity node type, deduplication pipeline, `INVOLVES` edge population
- **Graduated edge algorithms:** Promote highest-confidence slow-path patterns to fast-path algorithms based on `slow_path_log` data
- **Memory function tagging:** Tag nodes as `factual | experiential | working` with differential decay rates
- **Dependency-parser fast path:** spaCy-based structural skeleton extraction before LLM compilation for large-batch ingestion
- **Multi-agent namespace isolation:** Workspace scoping for shared vs. private memory partitions
- **Structured context packaging:** `POST /search` returns `{nodes, relationships, entities}` instead of flat node list
- **MCP adapter:** Expose the REST API through an MCP server layer (thin adapter over the Go CLI)

**Graduation targets (based on v0 slow_path_log):**
- `PRECEDES` edge: algorithmic if cosine_similarity > 0.7 AND temporal ordering is clear from timestamps
- `EXTENDS` edge: algorithmic if content overlap > 60% AND no semantic contradiction
- Merge suggestion: algorithmic if two active articles share domain_path AND embedding cosine > 0.90

### 12.3 v2 — Advanced Retrieval

**Theme:** Add the retrieval dimensions that require deep graph maturity.

**Key deliverables:**
- **Topology-derived embeddings (TopER):** Subgraph structural signatures as a fourth retrieval dimension. Enables structural similarity search independent of semantic content.
- **Topology monitoring:** TopER applied to full graph periodically; alerts on rapid topological change in a domain (signals knowledge restructuring)
- **Adaptive beam search traversal:** MAGMA-style heuristic beam search for graph traversal, replacing the current static-depth BFS
- **Differential memory decay:** Epistemic-type-specific decay rates for `factual` vs. `experiential` vs. `working` nodes
- **SQL/PGQ migration:** If SQL/PGQ is available in PG19+, migrate `AgeGraphRepository` to `SqlPgqGraphRepository` — same interface, standard query language
- **Sparse vector support:** Add SPLADE/BGE-M3 sparse vectors to `VectorAdaptor` for hybrid dense+sparse retrieval

### 12.4 Algorithm Graduation Timeline

```
v0 SLOW PATH                          v1 FAST PATH               v2 FAST PATH
─────────────────────────────────     ─────────────────────────  ─────────────────
PRECEDES inference            →       Temporal ordering rule
EXTENDS inference             →       Overlap + no-contradiction rule
Merge suggestion              →       High-sim same-domain rule
CAUSES inference                     remains slow (complex)
MOTIVATED_BY inference               remains slow (complex)
Contention detection          →                                  NLI-based classifier
Edge confidence scoring       →       Bayesian update rule
Intent detection              →       Keyword + embedding classifier
```

---

## 13. Success Criteria

### 13.1 Functional Parity Checklist

v0 is not done until all of the following are green:

**Sources:**
- [ ] `POST /sources` ingests with SHA-256 fingerprint deduplication
- [ ] Sources with identical fingerprints return the existing source ID (idempotent)
- [ ] `GET /sources/{id}` returns full source with metadata
- [ ] Embedding is generated and stored for every new source (within one maintenance cycle if API was unavailable at ingest)

**Articles:**
- [ ] `POST /articles/compile` accepts source_ids and returns a compiled article via slow path (202 + job ID)
- [ ] Compiled articles have provenance links (ORIGINATES edges) to all source nodes
- [ ] `POST /articles/{id}/split` produces two child articles with SPLIT_INTO edges to the original
- [ ] `POST /articles/merge` produces one merged article with MERGED_FROM edges to both parents
- [ ] Article `confidence` is a single float; no JSONB/float column drift possible
- [ ] `GET /articles/{id}/versions` returns full version history

**Edges:**
- [ ] `POST /edges` creates a typed edge with any string label without requiring schema migration
- [ ] `GET /nodes/{id}/neighborhood` returns BFS traversal at specified depth with edge label filtering
- [ ] `GET /articles/{id}/provenance` returns full provenance chain via graph traversal

**Search:**
- [ ] `POST /search` executes all three retrieval dimensions in the correct cascade order
- [ ] `POST /search?explain=true` returns timing breakdown and candidate counts
- [ ] Intent parameter changes edge weight selection in graph traversal
- [ ] Session scoping works (results filtered to session context when session_id provided)
- [ ] BM25 backend auto-degrades to ts_rank FTS when pg_textsearch fails startup check
- [ ] Retrieval events are recorded for every search hit

**Contentions:**
- [ ] `GET /contentions` lists open contentions
- [ ] `POST /contentions/detect` runs contention detection across the active article corpus
- [ ] Contention detection false positive rate < 10% on the migrated corpus (measured on manual-labeled ground truth from the 26 known Valence v2 contentions)

**Admin:**
- [ ] `GET /admin/stats` reports node counts, contention counts, slow-path queue depth, embedding coverage
- [ ] `POST /admin/maintenance` runs score recomputation, eviction, and queue processing
- [ ] Organic eviction archives lowest-usage-score non-pinned articles when over capacity limit

**Memory:**
- [ ] `POST /memory` creates a source node with `memory=true` in metadata
- [ ] `POST /memory/search` returns only memory-tagged nodes
- [ ] `PATCH /memory/{id}/forget` soft-deletes (sets `metadata.forgotten`; node preserved for audit)

**Migration:**
- [ ] `cov migrate --dry-run` reports correct counts without writing to Covalence
- [ ] `cov migrate --execute` imports all 264 sources and 289 articles with confidence recomputation
- [ ] `cov migrate --validate` passes with zero errors after a complete execute run
- [ ] All `supersedes_id` chains are converted to SUPERSEDES edges in AGE
- [ ] All `article_sources` links are converted to typed edges in AGE

### 13.2 Performance Targets

All targets measured on the Apple M4 Mac Mini (16GB RAM) with the full migrated corpus (~300 nodes):

| Operation | p50 Target | p99 Target | Measurement Method |
|---|---|---|---|
| `POST /sources` (with embedding) | 250ms | 500ms | wrk benchmark, 30s run |
| `POST /sources` (embedding unavailable → queued) | 20ms | 50ms | wrk benchmark |
| `POST /search` (all 3 dims, no explain) | 50ms | 200ms | wrk benchmark, 30s run |
| `POST /search` (vector+lexical only, no graph) | 20ms | 80ms | wrk benchmark |
| `GET /nodes/{id}/neighborhood` (depth=2, 50K corpus est.) | 100ms | 500ms | Artillery scenario |
| `POST /articles/compile` (queue time, not LLM time) | 10ms | 50ms | queue latency metric |
| `cov migrate --execute` (full corpus) | — | 90min | manual timing |

### 13.3 Graph Correctness Validation

After `cov migrate --validate`:
- Zero orphaned edges (every edge's from_node_id and to_node_id exists in `covalence.nodes`)
- Every source that appeared in `article_sources` has at least one edge in AGE
- Every active article has at least one ORIGINATES edge to a source
- SUPERSEDES edge count ≥ (number of articles with non-null supersedes_id in Valence v2)
- The AGE graph and `edges_meta` SQL mirror agree: `SELECT count(*) FROM edges_meta` matches `SELECT count(*) FROM cypher('covalence', $$ MATCH ()-[e]->() RETURN count(e) $$) AS (c agtype)`

### 13.4 Regression Test Suite

Before cutover, the following tests must pass:

1. **Known-good queries:** 10 manually constructed queries with known expected top-3 results (derived from Valence v2 production usage). Covalence must return the same articles in top-5 for at least 8/10.

2. **Provenance integrity:** For 5 randomly selected active articles, `GET /articles/{id}/provenance` must return at least one source for each.

3. **Contention audit:** The 26 open contentions from Valence v2 must all exist as CONTRADICTS edges in Covalence, or be explicitly marked as false positives with a rationale in the migration log.

4. **Idempotency:** Running `POST /sources` twice with the same content returns the same source ID both times.

5. **Cascade correctness:** `POST /articles/{id}/split` on an article with existing ORIGINATES edges produces two new articles that each inherit a subset of those provenance edges.

### 13.5 Definition of Done for v0 Cutover

All of the following must be true simultaneously:
1. Functional parity checklist (§13.1): 100% green
2. Performance targets (§13.2): All p99 targets met
3. Graph correctness validation (§13.3): Zero validation errors
4. Regression test suite (§13.4): 8/10 known-good queries pass, all other tests pass
5. `GET /admin/stats` shows: `slow_path_queue_depth < 10`, `nodes_without_embeddings = 0`
6. Valence v2 MCP server is confirmed operational in read-only mode (fallback available)
7. Jane has reviewed and signed off on the validation report

---

## Appendix A: Extension Installation Reference

```dockerfile
# Custom Docker image: PG17 + AGE + pgvector + pg_textsearch
FROM postgres:17

RUN apt-get update && apt-get install -y \
    build-essential libreadline-dev zlib1g-dev \
    flex bison git curl pkg-config libssl-dev

# Apache AGE 1.7.0 (PG17 branch)
RUN git clone --branch release/PG17/1.7.0 \
    https://github.com/apache/age.git /age && \
    cd /age && make install && rm -rf /age

# pgvector 0.8.2
RUN git clone --branch v0.8.2 \
    https://github.com/pgvector/pgvector.git /pgvector && \
    cd /pgvector && make install && rm -rf /pgvector

# pg_textsearch (Tiger Data)
# Install from source or binary package when GA releases
# For now: build from main branch
RUN git clone https://github.com/timescale/pg_textsearch.git /pg_textsearch && \
    cd /pg_textsearch && make install && rm -rf /pg_textsearch

COPY postgresql.conf /etc/postgresql/postgresql.conf
```

```ini
# postgresql.conf additions
shared_preload_libraries = 'age,pg_textsearch'
shared_buffers = 2GB
work_mem = 256MB
maintenance_work_mem = 2GB   # Required for HNSW index builds
max_connections = 20         # Single-agent; keep low for M4 RAM headroom
```

```sql
-- One-time initialization
CREATE EXTENSION age;
CREATE EXTENSION vector;
CREATE EXTENSION pg_textsearch;
LOAD 'age';
SET search_path = ag_catalog, "$user", public;
SELECT create_graph('covalence');
```

---

## Appendix B: Crate Selection Guidance

| Component | Selected Crate | Rationale |
|---|---|---|
| HTTP framework | `axum` | Async, tower-compatible, actively maintained |
| Database client | `sqlx` | Async, compile-time checked queries, PostgreSQL native |
| Connection pool | `deadpool-postgres` or sqlx's built-in | Either works; sqlx pool preferred for consistency |
| OpenAPI generation | `utoipa` | Generates from Rust types; axum integration |
| Async runtime | `tokio` | Standard; required by sqlx and axum |
| UUID | `uuid` crate with v4 feature | Standard |
| JSON | `serde_json` | Standard |
| Hashing | `blake3` | Fast; for query cache keys |
| In-process cache | `moka` | Async-aware LRU; for hot query caching |
| Error handling | `anyhow` + `thiserror` | Standard Rust error handling pattern |
| Tracing | `tracing` + `tracing-subscriber` | Structured logging + OpenTelemetry spans |
| Config | `config` crate | Multi-source configuration |

---

## Appendix C: Key Design Decisions Summary

| Decision | Choice | Date | Rationale |
|---|---|---|---|
| Implementation language | Rust (engine) + Go (CLI) | Pre-spec | Performance, safety, ecosystem |
| Storage backend | PostgreSQL 17 | Pre-spec | ACID, extension ecosystem, existing operational knowledge |
| Graph extension | Apache AGE 1.7.0 | Pre-spec | Only viable property graph option on PG17 today |
| Vector extension | pgvector 0.8.2 | Pre-spec | Production-ready, permissive license, halfvec support |
| Text search | pg_textsearch (BM25) + ts_rank fallback | Pre-spec | PostgreSQL license (vs AGPLv3 of ParadeDB) |
| API protocol | REST (OpenAPI 3.1) | Pre-spec | No MCP in v0 |
| Embeddings | OpenAI text-embedding-3-small | Pre-spec | Quality; design for swappability in v1 |
| Edge model | Typed string labels in AGE | Pre-spec | Extensible; no migration tax for new types |
| Knowledge model | Rich nodes + typed edges | Pre-spec | Graph > list; validated by 6 research papers |
| Write architecture | Dual-stream (fast/slow) | Pre-spec | MAGMA-validated; matches existing Valence pattern |
| AGE abstraction | GraphRepository trait | This spec | AGE abandonment hedge; SQL/PGQ migration path |
| Cascade pre-filtering | Lexical+Vector parallel → Graph on candidates | This spec | SING benchmark: 43× degradation without cascade |
| Confidence representation | Single canonical float per node | This spec | Eliminates three-representation drift in Valence v2 |
| Coexistence | Separate `covalence` schema, same PG instance | This spec | Zero-downtime migration |

---

*End of Covalence Phase Zero Specification.*  
*Version 1.0.0 — 2026-03-01*  
*For Phase One planning, consult §5 (v0 Scope), §6 (Data Model), §10 (API Surface), and §13 (Success Criteria).*  
*Questions and amendments: file in the Covalence project workspace under phase-zero/amendments/.*
