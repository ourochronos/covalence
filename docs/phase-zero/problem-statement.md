# Covalence — Phase Zero Problem Statement

**Status:** Draft  
**Date:** 2026-02-28  
**Author:** Architecture Review Agent  
**Project:** Covalence — Rust-based Knowledge Substrate for AI Agent Persistent Memory

---

## 0. Executive Summary

The current knowledge substrate (hereafter "Valence v2") is a PostgreSQL-backed system that manages articles (rich documents with confidence scores, provenance, and temporal validity) and sources (raw input material). It was designed to answer a simple question: *what does the agent know, and how sure is it?* It does that adequately for a flat collection of independent facts, but it fails as soon as knowledge grows, branches, conflicts, or converges. The structural primitives are too weak to represent how knowledge actually evolves.

Covalence replaces Valence v2 with an embedded, graph-native knowledge substrate written in Rust. It must handle rich knowledge nodes, typed edges that grow without schema migrations, full-text and semantic search, and an MCP tool interface — all within the resource envelope of a commodity developer machine.

---

## 1. Problem Statement — What Is Broken and Why

### 1.1 The Linear Versioning Trap

Valence v2 articles carry a single `supersedes_id` foreign key. This models knowledge as a singly-linked list: each article optionally points to the one it replaced. The implicit assumption is that knowledge revision is always one-to-one and strictly backward. Reality is messier:

| Real Event | What Valence v2 Can Model | What Actually Happens |
|---|---|---|
| Article grows, one subtopic becomes its own subject | Nothing — no split primitive | Two disconnected articles, no structural link |
| Two articles on overlapping topics are reconciled | Nothing — no merge primitive | New article with no pointer to either parent |
| Article A contradicts Article B | Nothing — contention is a side-table hack | Contention record with no graph semantics |
| Article A is related to, but doesn't supersede, Article B | Nothing | No edge at all |

The result is a **knowledge graph that believes it is a list**. Any operation that requires traversal — "give me everything related to topic X", "show me the full provenance chain for this claim" — either silently misses nodes or requires an application-layer workaround.

### 1.2 Compilation Creates Clones Instead of Versions

The `article_compile` operation ingests sources and writes a *new* article. There is no mechanism to say "recompile this existing article given new evidence." The expected workflow — ingest a source, update the relevant article — instead produces a second article covering the same ground. Over time, the article table accumulates **duplicate and near-duplicate documents** covering the same domain. Confidence scores and usage scores then fragment across clones rather than concentrating on the canonical node.

This is not a bug in the current implementation; it is a consequence of the data model. The data model has no identity-stable versioned node concept.

### 1.3 Edge Types Are Hard-Coded and Inextensible

Provenance relationships (`originates`, `confirms`, `supersedes`, `contradicts`, `contends`) are stored as an enum column in `article_sources`. Adding a new relationship type requires:

1. A PostgreSQL `ALTER TYPE` migration.
2. Application code changes.
3. A deployment window.

This creates a strong disincentive to model nuanced relationships. The agent ends up forcing novel edge semantics into the nearest existing enum value — `contends` does a lot of heavy lifting — rather than expressing them accurately. A graph substrate should let edge types grow organically: new typed edges should require no schema change, only a string.

### 1.4 No Structural Graph Traversal

There is no mechanism to ask: "starting from article X, what is the full neighbourhood of related articles within two hops?" The application layer can chase `supersedes_id` links, but that is the only navigable axis. Any richer traversal requires either a full table scan or a bespoke SQL query written for each new traversal pattern. This makes it impossible to build aggregate reasoning, cluster detection, or provenance walking at the substrate level.

### 1.5 External Database Server Dependency

Valence v2 requires a running PostgreSQL server. This introduces:

- **Operational complexity** — the agent runtime depends on a separately managed daemon.
- **Portability friction** — deploying to a new machine or a different runtime context means provisioning PostgreSQL first.
- **Resource overhead** — PostgreSQL is designed for multi-client concurrency; a single embedded agent application does not need this.
- **Migration difficulty** — any transition to a new substrate requires both systems to be online simultaneously, coordinating two connection pools.

For an AI agent's *private* memory system, the right deployment model is a library, not a client-server protocol.

### 1.6 Full-Text Search Is Available; Semantic Search Is Not

Valence v2 uses PostgreSQL full-text search (`tsvector`/`tsquery`), which works acceptably for keyword retrieval. There is no semantic or vector similarity search. This means retrieval is brittle to paraphrasing: "what do I know about car propulsion?" will miss articles indexed under "internal combustion engine" or "electric motor." For an AI agent whose knowledge is expressed in natural language, this is a significant recall gap.

---

## 2. Constraints

### 2.1 Hardware Envelope

| Dimension | Target |
|---|---|
| Primary deployment machine | Apple M4 Mac Mini, 16 GB RAM |
| Storage | Local NVMe (no size constraint specified; assume ≤ 50 GB working set) |
| Concurrency model | Single agent process; no multi-client requirement |
| Embedded | Yes — no external server process |

The hardware constraint is generous for an embedded knowledge substrate. The M4 has hardware-accelerated matrix operations usable for vector search. The design should exploit this but not depend on it (graceful degradation to CPU-only is acceptable).

### 2.2 Language and Runtime

- **Implementation language:** Rust (stable toolchain)
- **No runtime dependencies** beyond what can be compiled into the binary or loaded as a platform library (e.g., `libonnxruntime` for embeddings, if used)
- Crate ecosystem is fair game; no restriction on open-source dependencies

### 2.3 Interface Compatibility

- Covalence **must** expose an MCP (Model Context Protocol) tool interface. The agent interacts exclusively via tool calls; there is no direct query API consumed by the agent's inference layer.
- During migration, Covalence must **coexist** with the running Valence v2 system. It may not require taking Valence v2 offline as a precondition of operation.
- Migration tooling (reading from Valence v2's PostgreSQL schema and importing into Covalence) is in scope for Phase Zero planning but may be delivered in a subsequent phase.

### 2.4 Explicit Scope Exclusions

The following are **out of scope** for Covalence and must not drive architectural decisions:

| Excluded Feature | Rationale |
|---|---|
| NL-to-triples transduction (Plasmon) | Separate system; Covalence stores knowledge, does not extract it |
| P2P federation or multi-agent sync | Out of deployment envelope; adds protocol complexity |
| Network protocol / HTTP server | Agent access is in-process or local IPC via MCP; no networked client |
| Multi-tenant access control | Single-agent substrate; authentication/authorisation is not a concern |
| SPARQL or RDF compatibility | Semantic web standards impose schema rigidity; Covalence uses typed string edges |

---

## 3. Success Criteria — What "Done" Looks Like for v0

A v0 release is considered complete when all of the following are true:

### 3.1 Core Data Model

- [ ] **Identity-stable versioned nodes.** A node retains its UUID across content revisions. Retrieving a node by ID always returns the current version; prior versions are accessible via a version history API.
- [ ] **Typed, schema-free edges.** Any two nodes can be connected by an edge with an arbitrary string type (`supersedes`, `contradicts`, `related_to`, `derived_from`, `supports`, or any future type). No migration is needed to introduce a new edge type.
- [ ] **Rich node metadata.** Nodes carry at minimum: UUID, created/updated timestamps, content (markdown body), confidence score (float, 0–1), temporal validity range (optional), author type (`system | operator | agent`), and an arbitrary JSON metadata blob.
- [ ] **Source nodes.** Raw input material (documents, observations, web pages, conversations) is stored as a distinct node type, not conflated with compiled knowledge articles.

### 3.2 Graph Operations

- [ ] **Neighbourhood traversal.** Given a node ID and a depth limit, return all nodes within that many hops, optionally filtered by edge type(s).
- [ ] **Typed edge creation and deletion** without requiring any schema migration.
- [ ] **Contention detection.** When a new source contradicts an existing article, a `contradicts` edge is automatically created and surfaced in the contention list.
- [ ] **Split and merge as first-class operations.** Split: one node becomes two, both connected to the original by `split_from` edges. Merge: two nodes become one, with `merged_from` edges to both parents.

### 3.3 Search

- [ ] **Full-text search** returning ranked results, equivalent in quality to current Valence v2 behaviour.
- [ ] **Vector/semantic search** (nearest-neighbour by embedding similarity). Acceptable implementations include: `sqlite-vec`, `usearch`, `hnswlib` via FFI, or a Rust-native HNSW. Embeddings may be generated by a local model (e.g., `nomic-embed-text` via Ollama, or a bundled ONNX model).
- [ ] **Hybrid retrieval** combining BM25 and vector scores with a configurable fusion weight.

### 3.4 Persistence

- [ ] **Embedded storage** — the entire substrate runs as a library linked into the agent process (or a minimal local IPC daemon), with no external server.
- [ ] **Durability** — data survives process restart. Writes are crash-safe (at minimum, WAL-based durability).
- [ ] **Acceptable write latency** — p99 single-node write ≤ 50 ms on the target hardware.
- [ ] **Acceptable search latency** — p99 full-text or vector search over a 10,000-node corpus ≤ 200 ms.

### 3.5 MCP Tool Interface

The following MCP tools must be available and functionally equivalent to (or a superset of) the current Valence v2 tool set:

| Tool Category | Required Tools |
|---|---|
| Node management | `node_create`, `node_get`, `node_update`, `node_delete`, `node_history` |
| Edge management | `edge_create`, `edge_delete`, `edge_list` |
| Search | `knowledge_search` (hybrid), `source_search` |
| Graph traversal | `node_neighbourhood`, `provenance_trace` |
| Contention | `contention_list`, `contention_resolve` |
| Admin | `admin_stats`, `admin_maintenance` |
| Memory wrappers | `memory_store`, `memory_recall`, `memory_status`, `memory_forget` |

### 3.6 Coexistence and Migration

- [ ] Covalence can run alongside Valence v2 without port conflicts, file lock conflicts, or schema interference.
- [ ] A migration dry-run mode exists: it reads all articles and sources from Valence v2's PostgreSQL schema and reports what would be imported, without writing to Covalence.
- [ ] A migration execute mode performs the actual import, preserving UUIDs, timestamps, confidence scores, and provenance edges.

---

## 4. Key Design Questions

These questions must be resolved before implementation begins. They are not blocked on each other and can be investigated in parallel.

### 4.1 Storage Backend Selection

**Question:** Which embedded storage engine should back the graph?

**Options under consideration:**

| Option | Pros | Cons |
|---|---|---|
| **SQLite** (via `rusqlite`) | Mature, well-understood, excellent Rust support, WAL mode is solid | Relational; graph queries require recursive CTEs; no native vector search (though `sqlite-vec` exists) |
| **redb** | Pure Rust, MVCC, fast writes, no unsafe C | Key-value only; graph structure is fully application-managed; no FTS |
| **Sled** | Pure Rust, embedded | Less mature, no FTS, no vector search |
| **RocksDB** (via `rust-rocksdb`) | Battle-tested LSM, excellent write throughput | C++ dependency, complex tuning, no FTS/vector |
| **DuckDB** (via `duckdb-rs`) | Columnar analytics, FTS extension, HNSW vector index | Primarily OLAP; write amplification for small frequent updates may be high |

**Recommendation direction:** SQLite with `sqlite-vec` for vector search and FTS5 for full-text is the path of least surprise, but the graph traversal story needs verification. A spike comparing SQLite recursive CTE performance against an in-memory adjacency index for neighbourhood queries is warranted.

### 4.2 Embedding Model and Infrastructure

**Question:** How are text embeddings generated, and where does the model live?

**Sub-questions:**
- Bundled ONNX model (e.g., `all-MiniLM-L6-v2` via `ort`) vs. external Ollama endpoint vs. optional/pluggable?
- What is the embedding dimensionality? (affects storage size and search latency)
- Is embedding generation synchronous on write, or deferred/async?
- What happens if the embedding service is unavailable? (graceful degradation to FTS-only)

**Recommendation direction:** Make embedding pluggable behind a trait (`Embedder`), with two implementations: a no-op (FTS-only mode) and an Ollama HTTP client. Bundled ONNX is a stretch goal — useful for fully offline operation but adds binary size and build complexity.

### 4.3 Graph Representation

**Question:** How are edges stored and traversed?

**Options:**
- **Edge table in SQLite** — straightforward, portable, but graph traversal requires recursive SQL or application-level BFS/DFS.
- **In-memory adjacency index** — fast traversal, rebuilt on startup from the edge table. Acceptable for 10k–100k nodes; needs bounding analysis for larger corpora.
- **Hybrid** — persistent edge table + in-memory index for hot traversal.

**Key unknowns:** expected graph density (average degree per node), expected corpus size at steady state, and whether multi-hop traversal is needed at query time or only for maintenance operations.

### 4.4 MCP Server Architecture

**Question:** Does Covalence run as an in-process library or as a local daemon?

**In-process library:**
- Simplest deployment — link the crate, call functions.
- MCP tool calls are function calls; no IPC.
- Problem: if multiple agent processes ever need access (e.g., a subagent), they'd need separate instances or a locking scheme.

**Local daemon (stdio or Unix socket MCP server):**
- Clean separation between storage and agent runtime.
- Supports multiple agent processes sharing one substrate.
- Adds operational complexity (daemon lifecycle, restart on crash).

**Recommendation direction:** Start with in-process for v0 simplicity. Design the public API so that wrapping it in a daemon later is a thin layer, not a refactor.

### 4.5 Confidence Score Semantics

**Question:** How does confidence evolve over time, and should Covalence enforce decay?

The current system supports confidence scores but applies decay only via the maintenance queue. Design questions:
- Is confidence an intrinsic property of the node's content, or a function of provenance and time?
- Should Covalence compute confidence automatically from edge topology (e.g., number of confirming sources) or treat it as an opaque agent-supplied float?
- Does temporal validity (`valid_from`, `valid_until`) automatically suppress or reduce confidence after expiry?

This question has significant implications for the maintenance machinery and the `admin_maintenance` tool's responsibilities.

### 4.6 Migration Strategy and Cutover

**Question:** What is the migration path from Valence v2 to Covalence, and how is the cutover managed?

**Options:**
- **Big-bang:** Run migration, validate, switch all MCP tool bindings atomically.
- **Shadow mode:** Covalence receives all writes alongside Valence v2; reads are compared for divergence; cutover is gradual.
- **Dual-read:** Agent reads from both; writes go only to Covalence; Valence v2 is frozen.

**Recommendation direction:** Shadow mode is safest but doubles write latency during transition. Big-bang with a validated migration script and a fallback rollback procedure is likely sufficient given the single-agent, single-machine context.

---

## 5. Risks and Mitigations

### 5.1 Graph Query Performance at Scale

**Risk:** Neighbourhood traversal over a dense graph with 50k+ nodes becomes slow, causing p99 search latency to exceed targets.

**Likelihood:** Medium — knowledge graphs for a single agent are unlikely to grow this large quickly, but it is reachable over months of operation.

**Mitigation:**
- Spike graph traversal performance early (Phase One).
- Cap traversal depth at the MCP layer (default max 3 hops).
- Maintain an in-memory adjacency index for hot paths; persist to SQLite for durability.
- Add a breadth-first traversal with early termination on node count.

### 5.2 Embedding Model Unavailability

**Risk:** The embedding service (Ollama or ONNX) is unavailable at write time, causing nodes to be stored without vectors and degrading semantic search recall.

**Likelihood:** High — Ollama is a separate process that may not always be running.

**Mitigation:**
- Deferred embedding queue: write node without vector, enqueue for embedding, retry on next maintenance cycle.
- FTS remains fully functional regardless.
- Surface a health indicator in `admin_stats` showing how many nodes lack embeddings.

### 5.3 UUID Collision / Identity Instability During Migration

**Risk:** Migrating UUIDs from Valence v2 may introduce conflicts (e.g., if Covalence has already created nodes during coexistence).

**Likelihood:** Low — UUIDs are 128-bit; accidental collision is astronomically unlikely. Intentional UUID reuse during migration is the real risk if the agent creates nodes before migration runs.

**Mitigation:**
- Reserve a UUID namespace prefix for migrated nodes.
- Migration dry-run mode validates all UUIDs against the Covalence index before committing.
- Document that migration should run before Covalence is used for production writes.

### 5.4 Schema Creep in "Schema-Free" Edges

**Risk:** Edge types proliferate without discipline, becoming effectively meaningless (50 slightly different spellings of "related_to").

**Likelihood:** Medium — this is a known failure mode of schema-free systems.

**Mitigation:**
- Maintain a recommended edge type vocabulary in documentation; the substrate does not enforce it.
- Surface edge type frequency in `admin_stats` so anomalous proliferation is visible.
- The agent's tool-calling layer can normalise edge types before passing to Covalence.

### 5.5 Covalence and Valence v2 Write Divergence During Coexistence

**Risk:** During the migration window, some writes go to Valence v2 (via existing MCP tools) and some to Covalence (via new MCP tools), creating two partially-overlapping knowledge bases with no synchronisation.

**Likelihood:** High — this is inherent to any dual-system transition.

**Mitigation:**
- Define a hard cutover date: before cutover, all writes go to Valence v2 only; Covalence is read-only (migration target). After cutover, all writes go to Covalence.
- Use migration tooling to import the Valence v2 state immediately before cutover.
- Keep Valence v2 running read-only post-cutover for 30 days as an audit reference.

### 5.6 Rust Crate Ecosystem Maturity

**Risk:** Key crates (embedded graph storage, vector search, MCP server) are immature, have breaking API changes, or are abandoned.

**Likelihood:** Medium — the Rust embedded DB ecosystem is active but younger than the equivalent Python/Go ecosystem.

**Mitigation:**
- Prefer crates with >1.0 releases or clear stability guarantees for core storage (SQLite via `rusqlite` fits this).
- Isolate third-party crate surface behind internal traits — swapping the storage backend or vector index should not ripple through the whole codebase.
- Evaluate MCP server crates; if none are sufficiently mature, implement the JSON-RPC/stdio layer directly (it is not complex).

---

## 6. Out of Scope — Explicit Non-Goals (reiterated)

To keep the Phase Zero scope honest, the following are documented as non-goals and should be declined if proposed during Phase One planning:

- **Plasmon / NL-to-triples extraction** — knowledge enters Covalence as pre-processed nodes created by the agent; Covalence does not parse natural language into structured facts.
- **P2P replication or federation** — Covalence is a private, single-agent substrate.
- **Network-accessible API** — no HTTP server, no gRPC. Access is in-process or via local MCP stdio transport.
- **SPARQL / RDF / Linked Data** — these standards impose ontology constraints that conflict with the organic edge-type growth requirement.
- **Multi-tenant authentication** — single agent, no auth layer needed.

---

## 7. Recommended Next Steps

1. **Storage backend spike** — benchmark SQLite (FTS5 + sqlite-vec + recursive CTEs for graph) against a pure in-memory graph with redb persistence for the adjacency structure. Produce p50/p99 latency numbers for write, FTS search, vector search, and 2-hop neighbourhood traversal at 1k, 10k, and 50k nodes.

2. **MCP tool interface specification** — write the full MCP tool schema for Covalence v0 tools (JSON Schema for inputs/outputs). This is the contract the agent depends on and should be stable before implementation begins.

3. **Embedding strategy decision** — run Ollama with `nomic-embed-text` and measure embedding latency on M4 Mac Mini. If p99 ≤ 100 ms per node, synchronous embedding on write is acceptable. Otherwise, design the deferred embedding queue.

4. **Migration script prototype** — write a read-only migration dry-run that connects to Valence v2's PostgreSQL and dumps node/edge counts, validates UUIDs, and reports structural anomalies (orphaned sources, circular supersedes chains, missing provenance).

5. **Phase One scope document** — once design questions 4.1–4.4 are resolved, write the Phase One implementation plan with milestones, crate selections, and acceptance tests.

---

*End of Phase Zero Problem Statement. This document should be reviewed and approved before Phase One planning begins.*
