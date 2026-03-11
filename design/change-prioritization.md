# Design: Change Prioritization & Safe Development

## Status: current — reflects post-March-10 engineering wave

> **Updated 2026-03-10 (+post2)**: 47 of 50 GitHub issues closed. Additional post-wave closures:
> #43, #46, #51 (coref standalone stage), #52 (converter windowing), #57 (batch extraction
> thresholds), #58 (full stage configurability). GLiNER2 windowing ✅, Voyage reranker ✅.
> Bug #59: dotenvy ordering (fixed locally, commit pending). Bug #60: landscape gating too
> aggressive — disabled via `COVALENCE_LANDSCAPE_ENABLED=false`. Remaining open: #11, #35, #42,
> #59 (pending commit), #60 (landscape re-calibration).

## Spec Sections: 10-lessons-learned.md

## Architecture Overview

With 47 issues closed in one day, Covalence's prioritization focus has shifted from "building the pipeline" to "hardening and connecting what's built." This document reflects the current state and what's left.

## What Closed on March 10

| Issue | Description | Impact |
|-------|-------------|--------|
| #28 | URL-based ingestion | Accept URLs directly at API |
| #29 | UTF-8 safe chunking | Fixed multi-byte boundary crashes |
| #30 | Byte-offset chunks | Incremental re-ingestion, precise attribution |
| #32 | Two-pass extraction | GLiNER2 + NuExtract now active via `COVALENCE_ENTITY_EXTRACTOR=two_pass` |
| #33 | Source metadata enrichment | title/author/date now stored |
| #36 | HDBSCAN clustering wired | Ontology clustering end-to-end |
| #37 | Cluster pinning/gravity wells | Pinned clusters persist and attract |
| #39 | Knowledge gap detection | API endpoint + background analysis |
| #40 | Self-loop + example entity filtering | Graph cleanup automated |
| #43 | Landscape bypass for single-chunk sources | Single-chunk docs always get FullExtraction |
| #44 | Extraction sidecar wired | SidecarExtractor: three-stage coref→NER→rels with Rust windowing |
| #45 | Table linearization | Pure Rust, no model needed |
| #46 | Voyage env-based provider switching | `COVALENCE_EMBED_PROVIDER=voyage`; reranker auto-activated |
| #47 | Community detection fixed | Was producing 6,026 empty communities |
| #48 | Entity nodes filtered from search | Clean result sets |
| #49 | Extraction error logging | Warnings on parse failure |
| #50 | RRF score normalization | Dimension weights now meaningful |
| #4  | Layer evaluation harness | CLI verified end-to-end |
| #51 | Separate coref preprocessing from extraction | Coref is now standalone pipeline stage |
| #52 | Converter windowing for large HTML; PDF converter placeholder | HtmlConverter windowed; PDF trait wired as placeholder |
| #57 | Batch extraction token thresholds | min_extract_tokens=30, extract_batch_tokens=2000 |
| #58 | Full stage configurability | CONVERT_ENABLED, COREF_ENABLED, LANDSCAPE_ENABLED, EXTRACT_ENABLED |
| …28 more | Various bug fixes and completions | — |

## What Remains Open

| Issue | Description | Why Open |
|-------|-------------|----------|
| **#11** | Fine-tune relationship extraction | Needs labeled dataset + training run |
| **#35** | Federation scope decision | Architectural decision, not implementation |
| **#42** | Extraction alternatives research | Research tracking, not a code issue |
| **#59** | dotenvy initialized after tracing_subscriber | Bug fixed locally — commit/PR pending |
| **#60** | Landscape gating too aggressive | Disabled pending threshold re-calibration |

## Prioritization Framework

### Risk-Driven Ordering

Priority = Risk × Value, where:
- **Risk** = probability of user-facing impact × severity if it happens
- **Value** = number of downstream components unblocked + quality improvement

### Priority Tiers (Post-March-10)

#### P0: Fix Now (blocking search quality)

| Item | Risk | Value | Rationale |
|------|------|-------|-----------|
| **Rebuild Voyage vector index** | High | Unblocks entire vector search dimension | Dimension alignment broken after Voyage migration — only lexical search firing |

#### P1: Do Soon (high value, low risk)

| Item | Risk | Value | Rationale |
|------|------|-------|-----------|
| **Re-calibrate landscape (#60)** | Low | Re-enables extraction quality gating | Landscape disabled — all chunks extract at full cost regardless of content |
| **Commit dotenvy fix (#59)** | Low | Fixes tracing + env var startup reliability | Fix exists locally, just needs a commit |
| **Background consolidation loop** | Low | Enables auto-clustering and forgetting | Scheduler exists, no tokio task running it |
| **Incremental graph sync** | Medium | Eliminates restart-to-see-new-data | `outbox_events` table exists, not consumed |
| **Confidence_breakdown population** | Low | Epistemic explainability | Data computed but not stored in JSONB |

#### P2: Do When Convenient (good improvements, can wait)

| Item | Risk | Value | Rationale |
|------|------|-------|-----------|
| **Lost in the Middle reordering** | Zero | Better context assembly quality | Academic foundation in KB |
| **PII gating** | Low | Compliance for multi-user deployment | Pattern detection exists, not blocking |
| **Landscape analysis surfacing** | Low | Better ingestion quality signals | Metrics stored per chunk; disabled (#60) and not exposed via API |
| **PDF converter (complete #52)** | Low | Unblocks PDF ingestion at scale | Placeholder wired; pymupdf4llm sidecar integration pending |
| **Bridge node boosting in search** | Low | Better cross-domain results | Bridge discovery exists, not used in scoring |
| **Community → search weighting** | Low | Denser subgraphs ranked higher | Communities now correct, not in scoring |
| **Eval fixture generation** | Low | Regression testing | Harness works, no data |

#### P3: Deferred / Needs Decision

| Item | Decision Needed | Rationale |
|------|----------------|-----------|
| **Federation** (#35) | In-scope or separate service? | 10 unimplemented components, all blocked on scope |
| **Relationship extraction fine-tuning** (#11) | Labeled dataset needed first | No training data yet |
| **Auth layer** | Local-only or multi-user? | Scope determines complexity |
| **HyDE query expansion** | Value vs LLM cost? | Graph expansion works; LLM expansion is extra cost |
| **Streaming responses** | Which operations need SSE? | Nice-to-have |
| **Node2Vec embeddings** | Value vs complexity? | Requires training pipeline |

## Safe Change Practices

### The Strangler Fig Principle
Never big-bang replace. Always:
1. Build the new alongside the old
2. Route traffic incrementally
3. Verify parity
4. Remove the old

Applied: Voyage migration kept OpenAI as fallback until Voyage confirmed working. Two-pass extraction falls back to single-pass if sidecar is unavailable.

### Feature Flags for Partial Features
Components that are implemented but not activated:
- `NoopReranker` → `HttpReranker` (Voyage rerank-2.5) — config flag
- Single-pass → Two-pass extraction — sidecar availability as implicit flag
- `TrustRank: false` → `true` — config flag (needs seed set first)

### Database Safety
With a live DB:
- All migrations MUST be idempotent (`IF NOT EXISTS`)
- All migrations MUST be additive (add columns, not remove them)
- Schema changes separated from data migrations
- Backups before batch operations

### Characterization Tests
Before changing a component, capture its current behavior:
1. Run eval harness → save baseline metrics
2. Make change
3. Run eval harness → compare against baseline
4. Regression gate: fail if any metric drops > threshold

## Fitness Functions

| Fitness Function | Measures | Current Status |
|-----------------|----------|---------------|
| **Grounding %** | Academic backing of spec | ✅ Automated SQL query, run manually |
| **Embedding coverage** | % of chunks/nodes with embeddings | ✅ Can query from DB |
| **Search dimension firing** | Dimensions producing results | 🟡 Lexical firing; vector blocked on index rebuild; reranker now active with Voyage |
| **Entity extraction P/R/F1** | Extraction quality | 🟡 Eval crate works, no fixtures |
| **Search P@K, nDCG** | Search quality | 🟡 Eval crate works, no fixtures |
| **RAGAS scores** | End-to-end RAG quality | ❌ Stubs only |
| **Abstention rate** | False negative rate | ❌ Not tracked |
| **Cluster quality** | Ontology coherence | ❌ Not tracked |
| **Community count** | Sanity check on graph structure | ✅ Fixed (#47) — monitor for regression |

## The "Partial → Complete" Playbook

For each of the remaining partial components, the path to completion follows a pattern:

1. **Read the design doc** — understand what's implemented vs missing
2. **Write a characterization test** — capture current behavior
3. **Identify the smallest useful increment** — not "finish everything", but "what's the one change that unlocks the most value?"
4. **Make the change** — commit to main, push
5. **Verify fitness functions** — no regression
6. **Update the design doc** — move item from 🟡 to ✅

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| Technical debt | Kruchten et al. 2012 | ✅ Ingested |
| Strangler fig | Fowler 2004 | ✅ Ingested |
| DORA metrics | Forsgren et al. 2018 | ✅ Ingested |
| Risk-driven architecture | Fairbanks 2010 / ATAM | ✅ Ingested |
| Continuous delivery | Humble & Farley 2010 | ✅ Ingested |
| Evolutionary architecture | Ford et al. 2017 | ✅ Ingested |
| Reflective practice | Schön 1983 | ❌ Not ingested |
| Double-loop learning | Argyris & Schön 1978 | ❌ Not ingested |

## Next Actions

1. **Rebuild Voyage vector index** — highest leverage remaining item (unblocks vector search dimension)
2. **Re-calibrate landscape thresholds** (#60) — tune similarity cutoffs so valid chunks aren't gated; re-enable `COVALENCE_LANDSCAPE_ENABLED=true`
3. **Commit dotenvy fix** (#59) — one-liner move; unblocks reliable env-var startup
4. Start background consolidation loop (tokio background task)
5. Generate eval fixtures from current corpus
6. Wire grounding % as CI fitness function

> **Done since last prioritization**: GLiNER2 windowing (✅), Voyage reranker (✅), landscape
> single-chunk bypass (#43 ✅), Voyage env switching (#46 ✅), coref independent stage (#51 ✅),
> converter windowing (#52 ✅), batch extraction thresholds (#57 ✅), full stage configurability
> (#58 ✅). Two-pass extraction confirmed working: GLiNER (local) + Gemini Flash.
