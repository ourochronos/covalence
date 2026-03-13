# GraphRAG Specification

A hybrid knowledge base combining PostgreSQL/pgvector persistence with a Rust petgraph compute layer. Sources are ingested through an **embedding-first pipeline** that uses hierarchical chunking and embedding landscape analysis to determine what warrants LLM extraction — rather than extracting everything blindly.

## Design Principles

1. **Hybrid by default** — Vector embeddings for semantic search, graph for structural reasoning, fused at query time via RRF.
2. **Source fidelity** — Every fact traces back to a source with full provenance metadata. Nothing exists without attribution.
3. **Hierarchical chunking** — Documents are decomposed at multiple granularities (document → section → paragraph → sentence). Parent-child relationships enable precision retrieval with context injection.
4. **Dynamic ontology** — Entity types and relationship types emerge from data, not from a rigid predefined schema. Vector similarity drives entity resolution; community detection drives taxonomy.
5. **Composable search** — Vector, lexical, temporal, graph traversal, and structural searches compose via Reciprocal Rank Fusion with tunable weights.
6. **Embedding-first, LLM-targeted** — Embedding landscape analysis (parent-child alignment, peaks/valleys in adjacent similarity) determines which chunks warrant LLM extraction. Embeddings are cheap; LLM calls are expensive. The topology decides, not a blanket policy.
7. **Metadata is first-class** — Source type, author, date, confidence, provenance chain, and extraction method are all queryable dimensions.
8. **Uncertainty is information** — The system tracks epistemic uncertainty separately from belief using Subjective Logic. "Unknown" ≠ "50% likely."
9. **Three-timescale consolidation** — Online (per-ingestion), batch (periodic compilation), deep (structural maintenance + forgetting). Modeled on biological memory consolidation.

## Tech Stack

| Layer | Technology | Role |
|-------|-----------|------|
| Persistence | PostgreSQL 17 + pgvector | Durable storage, vector indexes (HNSW), full-text search (tsvector) |
| Graph compute | Rust + petgraph | In-memory directed graph, algorithms (PageRank, community detection, BFS/DFS), topological confidence |
| Embeddings | Voyage voyage-context-3 (v1), BGE-M3 (local v2) | Contextualized chunk embeddings — each chunk captures full document context. 2048d full, Matryoshka to 512d. First 200M tokens free. Outperforms OpenAI by 14%, Jina late chunking by 24%. |
| Extraction | LLM-driven pipeline (targeted) | Entity/relationship extraction, co-reference resolution — gated by embedding landscape analysis |
| API | Rust (Axum) HTTP + MCP | Query interface, ingestion endpoints, graph operations |

## Spec Documents

| Document | Scope |
|----------|-------|
| [01-architecture](01-architecture.md) | System layers, boundaries, data flow, component responsibilities |
| [02-data-model](02-data-model.md) | Node/edge/chunk schemas, hybrid property graph + provenance view, metadata model |
| [03-storage](03-storage.md) | PostgreSQL schema, pgvector indexes, migrations strategy |
| [04-graph](04-graph.md) | Rust petgraph sidecar, sync with PG, algorithms, topological confidence |
| [05-ingestion](05-ingestion.md) | Source handling, parsing, hierarchical chunking, **embedding landscape analysis**, targeted LLM extraction, entity resolution |
| [06-search](06-search.md) | Vector, lexical, temporal, graph, structural search; RRF fusion; query strategies |
| [07-epistemic-model](07-epistemic-model.md) | Confidence, provenance, contradiction detection, supersession, temporal validity |
| [08-api](08-api.md) | HTTP endpoints, MCP tools, client interface |
| [09-federation](09-federation.md) | Clearance levels, egress filtering, dual synthesis, ZK edges, trust tiers |
| [10-lessons-learned](10-lessons-learned.md) | Hard-won lessons: blanket extraction failure, embedding topology as signal, modifications vs rewrites, SING pattern, memory API |
| [11-evaluation](11-evaluation.md) | Evaluation methodology: RAGAS metrics, retrieval/generation/graph quality, regression gates, eval dataset construction |
| [12-code-ingestion](12-code-ingestion.md) | AST-aware code ingestion: Tree-sitter chunking, semantic summary wrapper, structural edge extraction, component linking |
| [13-cross-domain-analysis](13-cross-domain-analysis.md) | Cross-domain analysis: erosion detection, coverage analysis, blast radius, whitespace roadmap, dialectical critique |

**Reading paths:**
- **Implementing ingestion?** → 02 (data model) → 05 (pipeline) → 03 (storage schema)
- **Implementing search?** → 06 (search) → 04 (graph algorithms) → 07 (confidence integration)
- **Understanding the architecture?** → 01 → README → 10 (lessons)
- **Setting up evaluation?** → 11 → 06 (search metrics) → 08 (trace API)
- **Adding code ingestion?** → 12 (code pipeline) → 02 (data model: code entities) → 13 (analysis capabilities)
- **Understanding cross-domain analysis?** → 13 → 04 (graph algorithms) → 06 (cross-domain search)

## Prior Art (Internal)

This project distills lessons from several predecessors:

- **covalence** — Production Rust/PG/pgvector/petgraph engine. Source of the DimensionAdaptor pattern, search cascade, topological confidence, and causal edge semantics.
- **valence** — Python predecessor. Battle-tested RRF (K=60), temporal weight presets, article compilation pipeline, session ingestion.
- **valence-v2** — Triple-store + topology-derived embeddings (spectral, Node2Vec). FusionConfig five-signal scorer. Budget-bounded query operations.
- **valence-engine** — Design vision. Three-layer architecture (triples → sources → summaries). Self-closing loops. Budget-bounded ops.
- **bob / learner** — Multi-dimensional memory (5D indexing), experience → pattern → codification lifecycle.

## Research References

Key papers and techniques that influenced this design:

| Reference | Year | Relevance |
|-----------|------|-----------|
| Anthropic Contextual Retrieval | Sep 2024 | 50-100 token prefix per chunk reduces retrieval failure by 67%. Superseded by voyage-context-3 contextualized embeddings (6.76% better, no LLM cost). |
| RAPTOR (Sarthi et al., ICLR) | Jan 2024 | Recursive embed-cluster-summarize tree. Our hierarchical chunking + multi-level vector search is a superset. |
| Late Chunking (Günther et al., Jina) | Sep 2024 | Embed full document, then chunk token embeddings. Now available via API: Voyage context-3 (best quality), Jina v3 `late_chunking: true`. |
| Voyage voyage-context-3 | Jul 2025 | **v1 default embedding.** Contextualized chunk embeddings via API. +14.24% vs OpenAI, +23.66% vs Jina late chunking. 2048d, Matryoshka to 512d. First 200M tokens free. |
| LightRAG (Guo et al., EMNLP) | Oct 2024 | Dual-level (entity + relationship) graph retrieval. Validates semantic graph over raw text for retrieval. |
| HippoRAG 2 (Gutiérrez et al.) | Feb 2026 | Hippocampus-inspired retrieval with PersonalizedPageRank. Informed our PPR-based graph search dimension. |
| EA-GraphRAG (Zhang et al., PolyU) | Feb 2026 | GraphRAG underperforms vanilla RAG on simple queries by 13.4%. Validates our RRF fusion approach over explicit routing. |
| Max-Min Semantic Chunking | 2024 | Embedding-first, chunking-second. Adjacent sentence similarity for boundary detection. Informed our valley detection. |
| Matryoshka Representation Learning (Kusupati et al., NeurIPS) | 2022 | Nested embeddings at any prefix truncation. voyage-context-3 binary 512d outperforms OpenAI float 3072d. Enables multi-resolution storage. |
| Core-based Hierarchies (Hossain et al.) | Mar 2026 | k-core decomposition as deterministic O(\|E\|) replacement for Leiden. Proves modularity optimization has exponentially many near-optimal partitions on sparse KGs. Adopted for community detection in Stage 4 (graph). |
| Tree-KG (Niu et al., ACL 2025) | Jul 2025 | Expandable KG construction from structured domain texts. Tree-like graph from textbook structures + iterative expansion with predefined operators. F1 0.81 on Text-Annotated dataset. Validates our graduated extraction approach. |
| iText2KG (Lairgi et al.) | Sep 2024 | Incremental, topic-independent KG construction. Four modules: Document Distiller, iEntities Extractor, iRelation Extractor, Graph Integrator. Key insight: resolve entities incrementally during extraction (not batch post-processing). Validates our in-prompt entity resolution. |
| UnKGCP (Zhu et al., EMNLP 2025) | Oct 2025 | Conformal prediction for uncertain KG embeddings. Generates prediction intervals with statistical guarantees. v2 consideration for confidence calibration. |
| Graphiti / Zep (Rasmussen et al.) | Jan 2025 | Bi-temporal knowledge graph for agent memory. Edge invalidation (not deletion) for contradictions. Informed our bi-temporal edge model (`valid_from`, `invalid_at`, `invalidated_by`). |
| StepChain GraphRAG (Yuan et al.) | Oct 2025 | Multi-hop QA via BFS reasoning flow with query decomposition. Validates exposing fine-grained MCP primitives for iterative retrieval. |
| RAGAS (Es et al.) | Sep 2023 | Reference-free RAG evaluation: faithfulness, answer relevancy, context precision/recall. No ground truth annotations needed. Core eval framework. |
| Adaptive-RAG (Jeong et al.) | Mar 2024 | Query complexity classifier routes to no-retrieval / single-step / multi-step. Our SkewRoute-inspired score distribution analysis achieves similar routing without a trained classifier. |
| SkewRoute (Wang et al.) | May 2025 | Training-free query routing via retrieval score distribution skewness. 50% fewer large LLM calls at near-baseline accuracy. Adopted for adaptive strategy selection. |
| AutoSchemaKG (Bai et al.) | May 2025 | Autonomous KG construction with dynamic schema induction. 92% alignment with human schemas. Informed our open entity_type approach. |
| STAR-RAG (Zhu et al.) | Oct 2025 | Temporal GraphRAG: time-aligned rule graph + seeded PPR. +9.1% accuracy, -97% tokens. Informed point-in-time temporal search. |
| GraphRAG Privacy Risks (Zhang et al.) | Aug 2025 | Graph RAG leaks more structured entity/relationship data than plain RAG. Informed PII handling in data model. |
| MemOS | Jul 2025 | Memory OS for AI: hierarchical graph, task-concept-fact paths, conflict detection, dedup, versioning, forgetting policies. Informed organic forgetting lifecycle. |
| Entity Resolution at Scale (Shereshevsky) | 2025 | Three-phase: blocking via embedding similarity, pairwise cross-encoder, transitive closure. Our embedding linkage (7.1) naturally provides blocking. |
| Subjective Logic (Jøsang) | 2016 | Epistemic uncertainty model. Core of our confidence propagation (spec 07). |
| Microsoft GraphRAG (Edge et al.) | 2024 | Community detection for thematic clustering. Informed our Louvain community detection in spec 04. |

## Status

This spec is a living document. Each sub-document has a status field:
- **Draft** — Initial structure, open questions remain
- **Review** — Content complete, awaiting validation
- **Settled** — Agreed upon, implementation-ready
