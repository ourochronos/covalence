# ADR-0021: Simplify Pipeline to Statements and Chunks

**Status:** Proposed
**Date:** 2026-03-20
**Deciders:** Chris Jacobs, Claude Opus

## Context

The current ingestion pipeline for prose sources has a complex hierarchical summary chain:

```
chunks → statements → HAC clustering → sections → section compilation (LLM)
→ source summary compilation (LLM) → source embedding
```

This chain:
- Requires multiple LLM calls per source (section compilation + source compilation)
- Is the most fragile part of the pipeline (compose_source_summary jobs were 24% of all dead jobs in Session 41)
- Produces summaries that are lossy compressions of the raw text
- Creates staleness — summaries don't update when sources change
- Adds 3 pipeline stages (HAC, section compile, source compile) that each can fail

Meanwhile, the `/ask` endpoint already has an LLM that can synthesize answers from raw chunks and statements at query time.

## Decision

Remove the hierarchical summary pipeline. Keep statements and chunks as the primary knowledge representations. Let `/ask` synthesize on demand from raw content.

### What Stays

| Component | Why |
|-----------|-----|
| **Chunks** | Primary retrieval units. Embedded, searchable, provenance-linked. The foundation of the search pipeline. |
| **Statements** | Atomic, self-contained knowledge claims extracted from chunks. Eliminate noise, enable precise retrieval. |
| **Code entity summaries** | Semantic bridge between code syntax and prose queries. `fn resolve_tier5()` needs a natural language description to be findable. These are per-entity, not hierarchical. |
| **Node embeddings** | Entity-level search. |
| **Chunk embeddings** | Document-level search. |
| **Statement embeddings** | Claim-level search. |

### What Goes

| Component | Why remove |
|-----------|-----------|
| **Sections** (HAC clustered groups) | Intermediate artifact with no direct search value. Statements are already self-contained. |
| **Section compilation** (LLM) | Lossy compression of statement groups. /ask can read raw statements directly. |
| **Source summary compilation** (LLM) | Pre-computed "what is this about" that /ask can determine on demand. |
| **ComposeSourceSummary job** | No longer needed — removes the most fragile pipeline stage. |

### Source-Level Embeddings

**Open question:** What's the best strategy for embedding large sources without LLM-compiled summaries?

**Decision: MaxSim — no pre-computed source embeddings.**

Source relevance is determined at query time by the maximum similarity of any chunk or statement in the source to the query vector:

```sql
SELECT s.id, s.title, MIN(st.embedding <=> $1) as best_distance
FROM statements st
JOIN sources s ON s.id = st.source_id
GROUP BY s.id, s.title
ORDER BY best_distance
LIMIT $2
```

The source is as relevant as its best-matching content. No pre-computation, no LLM cost, no staleness, query-dependent (multi-topic sources surface the right facet).

If MaxSim proves insufficient, density-weighted variants can be explored (e.g., `max_sim * (1 + α * ln(count))`), but start simple.

### The /ask Upgrade

Instead of relying on pre-computed summaries, `/ask` becomes the synthesis layer:

1. Search returns relevant chunks/statements (existing behavior)
2. For high-relevance hits, fetch surrounding raw source context (new)
3. Feed the LLM: retrieved passages + raw context + question
4. The LLM reads actual text, not compressed summaries

This is standard RAG — retrieval + synthesis. The pre-compilation step was an optimization that cost more than it saved.

## Consequences

### Positive
- **Simpler pipeline** — 3 fewer stages (HAC, section compile, source compile)
- **Fewer LLM calls** — no compilation step per source, only per entity (code summaries)
- **Fewer failure modes** — ComposeSourceSummary was the most fragile job kind
- **Fresher data** — no stale summaries to invalidate
- **Faster ingestion** — skip the slowest pipeline stages
- **Cheaper** — significant reduction in LLM API costs

### Negative
- **Slightly slower /ask** — may need to read more raw text at query time
- **Loss of source-level "about" description** — previously available via summary field
- **Search quality unknown** — source embeddings from summaries may have been better than alternatives. Needs measurement.

### Risks
- Search regression: if source-level embeddings degrade, global search dimension suffers. Run search regression before and after.
- /ask quality: if the LLM can't synthesize well from raw chunks + statements, we may need a lighter compilation step (not full hierarchical, but some summarization)

## Migration Path

1. **Measure** — run search regression with current system as baseline
2. **Remove ComposeSourceSummary** — stop generating new source summaries
3. **Remove section compilation** — stop HAC clustering and section LLM calls
4. **Keep existing summaries** — don't delete, just stop generating new ones (epistemic principle: old observations aren't garbage)
5. **Upgrade /ask** — fetch raw source context for high-relevance results
6. **Research source embedding strategies** — test the options empirically
7. **Measure again** — compare search quality before/after

## Open Questions

1. Best source embedding strategy without LLM summaries?
2. Should existing sections/summaries be kept for search, or excluded?
3. Does RAPTOR (recursive summarization) still have value if hierarchical compilation is removed?
4. For code sources, entity summaries stay — should the file-level composition also stay? (It uses entity summaries, not the statement pipeline)
