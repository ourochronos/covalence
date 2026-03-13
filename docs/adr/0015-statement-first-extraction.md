# ADR-0015: Statement-First Extraction Pipeline

**Status:** Accepted

**Date:** 2026-03-13

**Spec Reference:** spec/05-ingestion.md

**Supersedes aspects of:** ADR-0007 (hierarchical chunking), ADR-0009 (three-timescale consolidation)

## Context

The current ingestion pipeline follows a chunk-first approach: raw source text is structurally chunked (by headings and size constraints), chunks are embedded, then entities are extracted from chunks. This creates several compounding problems:

1. **Noise propagation.** Chunks inherit formatting artifacts, bibliography entries, boilerplate, and author blocks from the source. Extensive post-hoc quality filters (bibliography detection, boilerplate detection, author block detection, reference section detection) fight this noise but can never fully eliminate it. Each new source type introduces new noise patterns.

2. **Pronoun-dependent chunks.** Structural chunks frequently contain unresolved anaphora: "This approach improves performance by 12%." Without knowing what "this approach" refers to, the chunk is unsearchable for the actual method. Coreference resolution (fastcoref, etc.) is fragile and adds pipeline complexity.

3. **Fragile re-extraction.** When re-extracting a source (e.g., after improving the extractor), the new entity set differs from the old one in unpredictable ways — different names, descriptions, edge structures. Reconciliation requires four-tier entity resolution (exact → alias → vector → trigram) and still produces inconsistencies. Normalization is fought repeatedly.

4. **Lossy hierarchy.** RAPTOR summaries are bolted on after chunking, summarizing chunks rather than source content. Consolidation articles compile from chunks across sources, losing per-source structure. Neither produces a clean hierarchical zoom from source overview → topic → specific claim → source text.

5. **Embedding space contamination.** Vector representations of noisy chunks pollute the embedding space, degrading nearest-neighbor search quality for all queries.

The root cause: **knowledge extraction happens after structural decomposition, so the knowledge units inherit the decomposition's problems.** Inverting this — extracting knowledge first, then building structure from the extracted knowledge — eliminates the entire noise-propagation surface.

## Decision

Replace the chunk-first pipeline with a **statement-first extraction pipeline**. The core change: LLM extraction happens on the raw source text *before* any structural decomposition. The extracted knowledge (atomic statements) becomes the fundamental unit, replacing structural chunks.

### Pipeline Overview

```
Source text (stored as ground truth, not embedded)
  ↓ windowed LLM extraction
Atomic statements (self-contained, no pronouns, with source location refs)
  ↓ embed statements → cluster by similarity
Statement groups
  ↓ for each group: statements + source context window → LLM compiler
Section summaries (embedded, link to constituent statements)
  ↓ section summaries → LLM compiler
Source summary (embedded, links to sections)
```

### Layer 1: Atomic Statement Extraction

The first pass windows over the raw source text and extracts **atomic statements**. Each statement:

- Is a single, self-contained factual claim or assertion
- Contains no pronouns — all referents are explicit (prompted behavior, eliminates need for coreference resolution)
- Carries a **source location reference** (byte offset range, heading path, or paragraph index) pointing to where in the source text the statement is grounded
- Is independently meaningful — can be understood without reading surrounding statements

Example extraction from a passage about RAPTOR:

| Statement | Source Location |
|-----------|----------------|
| "RAPTOR uses k-means clustering to group semantically similar text chunks before recursive summarization." | §3.2, para 1 |
| "RAPTOR's tree structure enables retrieval at multiple levels of abstraction, from leaf chunks to root summaries." | §3.2, para 3 |
| "On the NarrativeQA benchmark, RAPTOR with GPT-4 achieves a METEOR score of 0.536, a 20% improvement over baseline RAG." | §4.1, Table 2 |

The prompt explicitly instructs the LLM to:
- Write each statement as if the reader has no context
- Replace all pronouns with their referents
- Include quantitative claims with their units and conditions
- Omit bibliographic citations, author lists, acknowledgments, and boilerplate
- Tag each statement with its source location

This eliminates the noise problem at the source: bibliography, boilerplate, author blocks, and formatting artifacts are never extracted because they contain no knowledge claims.

### Layer 2: Statement Clustering

Atomic statements are embedded and clustered by semantic similarity. Clustering produces natural topic groupings — all statements about "RAPTOR's clustering algorithm" cluster together, all statements about "evaluation results" cluster together.

The clustering algorithm (k-means, hierarchical, or HDBSCAN) operates on statement embeddings. The number of clusters is determined by the source's complexity, not by structural headings.

### Layer 3: Section Compilation

For each statement cluster, the section compiler receives:
- The clustered statements
- A **context window** from the raw source text around the referenced locations (so the compiler can verify and contextualize, not just summarize summaries)

The compiler produces a **section summary** that:
- Synthesizes the statements into a coherent narrative
- Links to its constituent statements (by ID)
- Gets its own embedding
- Is a natural retrieval unit for topic-level queries

### Layer 4: Source Summary

The source summary compiler receives the section summaries (NOT the raw source — keeping context manageable). It produces a single overview that:
- Captures the source's main contributions and conclusions
- Links to its constituent sections
- Gets its own embedding
- Serves as the retrieval unit for broad queries

### Layer 5: Cross-Source Articles (Consolidation)

The existing Article concept is preserved but simplified. Instead of compiling from raw chunks across sources, consolidation:
- Clusters **statements across all sources** by embedding similarity
- Groups cross-source statement clusters into topic articles
- Compiles using section summaries that reference those statements

This uses the same cluster → compile pattern as within-source processing. The architecture is fractal: same mechanism at source scope and global scope.

### Re-Extraction

When a source is re-extracted (improved extractor, updated content):

1. Extract new statement set from source
2. Match against existing statements (embedding cosine similarity > threshold, or content hash)
3. **Present in both:** Keep, no action needed
4. **New in re-extraction:** Add to the graph (superset growth)
5. **Missing from re-extraction:** Do not auto-delete. Instead:
   - Check the source location reference — is the claim still supported by the source text at that location?
   - If yes: keep (extractor missed it this time)
   - If no: mark for eviction

Step 5 is the critical advantage: statements have explicit provenance, so verification is mechanical. No fuzzy entity matching, no normalization heuristics — just "does paragraph 3.2 actually say this?"

### Migration

Existing sources are not deleted. They are re-processed through the new pipeline to rebuild their statement trees. The graph is reconstructed from the new extraction layer. This can be done incrementally — source by source — without downtime.

## Consequences

### Positive

- **Eliminates noise at source.** No bibliography filters, boilerplate detectors, or author block heuristics needed — noise is never extracted because it contains no knowledge claims.
- **Self-contained retrieval units.** Every statement is independently searchable without context. No more "this approach" → ???
- **Clean embedding space.** Vectors represent actual knowledge, not formatting-contaminated text.
- **Natural hierarchy with zoom.** Search matches at the right granularity: broad queries hit summaries, specific queries hit atomic statements, drill-down reaches source text.
- **Robust re-extraction.** Set operations with mechanical verification replace fragile entity reconciliation.
- **Simplified consolidation.** Cross-source articles use the same cluster → compile pattern as within-source processing. One mechanism, two scopes.
- **Provenance by construction.** Every statement points to its source location. Every section links to its statements. Every summary links to its sections. The provenance chain is structural, not inferred.
- **Reduced pipeline complexity.** Removes: chunk quality filters (6+ heuristics), RAPTOR post-hoc summarization, coreference resolution. Adds: statement extraction prompt, section compiler prompt.

### Negative

- **Higher LLM cost at ingestion.** Every source requires LLM extraction (windowed passes) rather than just structural splitting. Mitigated by: extraction is a one-time cost per source, and the current pipeline already uses LLM for entity extraction and RAPTOR.
- **Extraction quality depends on prompt design.** The statement extraction prompt is the critical component — poor prompts produce poor statements. Requires careful iteration and evaluation.
- **Latency increase at ingestion.** Windowed LLM passes are slower than structural chunking. Acceptable because ingestion is not latency-sensitive (batch operation).
- **Migration effort.** All existing sources need re-processing. Can be done incrementally but represents significant compute cost.
- **Statement deduplication across sources.** Multiple sources may state the same fact. Need cross-source dedup at the statement level (embedding similarity matching).

## Alternatives Considered

- **Keep chunk-first, improve filters.** The current approach of adding quality filters for each noise pattern. Rejected: this is whack-a-mole — each new source type introduces new noise patterns. The fundamental problem is that structural chunks don't align with knowledge boundaries.

- **Chunk-first with coreference resolution.** Add fastcoref or similar to resolve pronouns in chunks. Rejected: adds pipeline complexity, is fragile on domain-specific text, and doesn't solve the noise problem.

- **Question generation per chunk (Pike-RAG knowledge atomizing).** Generate atomic questions per chunk as retrieval indexes. Rejected: questions are retrieval aids, not knowledge representations. Statements are the actual knowledge; questions can be generated from statements if needed.

- **Hybrid: chunk for embedding, extract for graph.** Keep structural chunks for vector search, use extraction only for the graph layer. Rejected: maintains two parallel representations, doubles storage and embedding costs, and the chunk noise problem persists in vector search.
