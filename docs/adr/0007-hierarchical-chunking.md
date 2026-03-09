# ADR-0007: Hierarchical Chunking with Structural + Semantic Boundaries

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/05-ingestion.md

## Context

Traditional flat chunking loses document structure. Fixed-size chunks split mid-sentence or mid-topic. The system needs to preserve document hierarchy and detect topic boundaries.

## Decision

Use hierarchical chunking: document → section → paragraph → sentence. Primary boundaries from Markdown heading structure (structural). Secondary boundaries from embedding similarity drops between adjacent sentences (semantic). Size constraints: max 1024 tokens (complex documents), min 32 tokens (merge with neighbors).

## Consequences

### Positive

- Preserves document structure as queryable metadata
- Parent-child retrieval: search at sentence level, retrieve at paragraph/section level
- Structural hierarchy enables pre-filtering (query about "Chapter 2" filters by metadata)
- Semantic boundaries catch topic shifts within structural chunks
- 1024-token max validated by LlamaIndex eval (optimizes faithfulness + relevancy)

### Negative

- More complex than fixed-size chunking
- Semantic boundary detection requires embedding each sentence (additional API calls)
- Hierarchy depth varies by document type

## Alternatives Considered

- **Fixed-size sliding window:** Simple but loses all structure
- **Recursive splitting (LangChain-style):** Better than fixed but still size-driven, not structure-driven
- **Sentence-only:** Too fine-grained for embedding quality
