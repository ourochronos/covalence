# Design: Development Process

## Status: emergent, being formalized now

> **Updated 2026-03-10**: Massive engineering wave — 47 of 50 GitHub issues closed in a single day.
> Full local model pipeline confirmed: ReaderLM-v2 → Fastcoref → GLiNER2 → NuExtract →
> HDBSCAN (~5.5GB RAM total). Embedding provider switched to Voyage AI. Gemini 2.5 Flash via
> OpenRouter now active for extraction ($0.30/M tokens). Only 3 issues remain open: #11 (fine-tune
> RE), #35 (federation scope), #42 (extraction research tracking).

## Spec Sections: 10-lessons-learned.md, 11-evaluation.md

## Architecture Overview

Covalence's development process is itself a knowledge-driven system: the graph reviews its own design, identifies gaps, triggers targeted research, and measures improvement. This document captures the process that emerged organically during initial development and formalizes it for ongoing use.

## The Flywheel

The core development loop has four stages:

```
1. INSPECT  →  Read code, map to spec, measure what exists
2. DOCUMENT →  Write design doc with status tables (✅ 🟡 ❌)
3. GROUND   →  Identify citation gaps, spawn paper ingestion
4. MEASURE  →  Compute grounding %, coupling, isolation scores
     ↓
   (repeat — each cycle improves the next)
```

This flywheel is self-reinforcing: design docs identify which papers are missing → papers improve the graph → the improved graph produces better gap analysis → the next design doc is more precise.

## Evidence: The Flywheel Works

### March 9, 2026 — Initial documentation session (~20 minutes)

| Metric | Before | After | Δ |
|--------|--------|-------|---|
| Spec grounding (median) | ~20% | ~52% | +32pp |
| Sources | 45 | 88+ | +43 |
| Knowledge graph nodes | ~1,050 | 1,807 | +757 |
| Knowledge graph edges | ~1,796 | 3,010 | +1,214 |
| Design docs | 1 | 8 | +7 |
| Subsystems with documented status | 1 | 8 | all |

### March 10, 2026 — Engineering wave (single day)

| Metric | Before | After | Δ |
|--------|--------|-------|---|
| Open GitHub issues | 50 | 3 | -47 |
| Two-pass extraction | ❌ Blocked | ✅ Active | — |
| Local model pipeline | ❌ Untested | ✅ Confirmed | ~5.5GB RAM |
| Embedding provider | OpenAI | Voyage AI | $0.13→$0.01/M |
| Extraction provider | Gemini Flash Lite | Gemini 2.5 Flash | Better quality |
| Community detection | 🐛 6,026 empty | ✅ Correct | — |
| Byte-offset chunking | ❌ | ✅ | Incremental re-ingest |
| URL ingestion | ❌ | ✅ | Accept URLs directly |
| Source metadata | ❌ | ✅ | title/author/date |

## Process Components

### Implemented ✅

| Component | How It Works |
|-----------|-------------|
| **Three-layer traceability** | Papers (why) → Spec (what) → Design docs (how), with gap analysis at each layer |
| **Grounding analysis** | SQL query counts spec entities that also appear in paper/design sources — measures academic backing |
| **Coupling analysis** | Shared entity count between spec sections — reveals which specs are tightly/loosely connected |
| **Sink/source detection** | Graph nodes with high in-degree but zero out-degree are knowledge dead-ends needing expansion |
| **Bridge entity detection** | Entities connecting papers ↔ specs are the intellectual lineage of the system |
| **Async paper workers** | Subagents research, synthesize, and ingest papers in parallel while analysis continues |
| **Design doc template** | Status, spec sections, implemented/partial/not-implemented tables, gaps, academic foundations, next actions |
| **Immediate git commit** | Design docs committed and pushed as they're written — no batch, no delay |
| **Issue-driven development** | GitHub issues tracked per feature; closed when confirmed working in code |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **ADR format** | Design docs capture decisions | Not using formal Nygard/Zdun ADR template with Status/Consequences |
| **Regression gating** | Eval crate has regression baselines | Not wired to CI — can't block merges on quality drops |
| **DSRM alignment** | We follow Hevner's guidelines informally | Not explicitly mapped to the 6-step DSRM process |

### Not Implemented ❌

| Component | Source | Priority |
|-----------|--------|----------|
| **Automated gap detection** | #39 ✅ Closed | API endpoint implemented; not yet wired to CI check |
| **Grounding CI check** | — | Medium — fail build if grounding drops below threshold |
| **Design doc staleness detection** | — | Medium — detect when code changes invalidate a design doc |
| **Process metrics dashboard** | — | Low — track grounding %, node/edge counts, worker success rate over time |
| **Automated paper suggestions** | — | Low — graph analysis suggests papers to ingest |
| **DSRM evaluation phase** | Hevner 2004 | Medium — formal evaluation criteria for each design artifact |

## Current Model Stack (confirmed March 10)

The full local pipeline is confirmed working at ~5.5GB total RAM:

| Stage | Model | RAM | Notes |
|-------|-------|-----|-------|
| HTML → Markdown | ReaderLM-v2 (MLX) | ~1GB | High-quality structure preservation |
| PDF → Markdown | pymupdf4llm | ~0MB | No model — pure extraction, 3.4s/15 pages |
| Coreference resolution | Fastcoref 90M | ~300MB | 20KB context OK |
| Entity extraction (NER) | GLiNER2 ~500MB | ~500MB | 384-token limit (windowing needed) |
| Relationship extraction | NuExtract-1.5-tiny 0.5B | ~1GB | 4K token context |
| Embeddings | Voyage AI (cloud) | ~0MB | $0.01/M tokens, voyage-3-large |
| LLM fallback / enrichment | Gemini 2.5 Flash (OpenRouter) | ~0MB | $0.30/M tokens |

## Open Issues (as of 2026-03-10)

| Issue | Description | Status |
|-------|-------------|--------|
| #11 | Fine-tune relationship extraction | 🔴 Open |
| #35 | Federation scope decision | 🔴 Open |
| #42 | Extraction alternatives research tracking | 🔴 Open |
| All others | — | ✅ Closed 2026-03-10 |

## Key Design Decisions

### Why the graph reviews itself
Covalence is a knowledge graph engine. Using it to manage its own development means every improvement to the engine immediately improves the development process. The system and its development co-evolve.

### Why design docs over ADRs (for now)
ADRs capture individual decisions. Design docs capture the full status of a subsystem: what works, what doesn't, what's next. They're broader and more useful for gap analysis. ADRs could be extracted from design docs later — each "Key Design Decision" section is a proto-ADR.

### Why grounding percentage as the primary metric
Grounding measures how much of the spec is backed by academic literature. It's objective (entity overlap), automated (SQL query), and actionable (low grounding → ingest more papers on that topic). It's not perfect — entity matching is noisy — but it's directionally correct and computable in milliseconds.

### Why Voyage AI over OpenAI for embeddings
Voyage `voyage-3-large` is $0.01/M tokens vs OpenAI `text-embedding-3-large` at $0.13/M — 13× cheaper with comparable or better quality on technical domain retrieval. Voyage is also the provider for the `rerank-2.5` model already wired in the codebase. One provider for both embeddings and reranking simplifies key management.

### Why Gemini 2.5 Flash for extraction
At $0.30/M tokens and with strong instruction following, Gemini 2.5 Flash via OpenRouter offers better extraction quality than Gemini Flash Lite at minimal extra cost. OpenRouter provides vendor-agnostic routing — if Gemini degrades, switching to another model is a one-line config change.

### Why immediate commit
Design docs are artifacts of analysis, not proposals. They describe what IS, not what should be. No review gate needed — they're documentation of measured reality. Push immediately, iterate later.

## Process Patterns Observed

### The Citation Gap Pattern
When a design doc identifies a concept referenced in code but absent from the KB, that's a citation gap. Filling it always improves grounding, often by 5-15 percentage points.

### The Bridge Pattern
Papers that introduce concepts used across multiple spec sections have outsized impact. Prioritize bridge papers over section-specific ones.

### The Isolation Pattern
Spec sections that share few entities with other sections are either (a) genuinely independent or (b) missing connections. Design docs fix isolation by explicitly mapping connections.

### The Sink Pattern
Graph entities with many incoming edges but no outgoing edges are knowledge dead-ends. Filling these enriches the graph's explanatory power.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| Architecture Decision Records | Nygard 2011 | ✅ Ingested |
| Design Science Research | Hevner et al. 2004 | ✅ Ingested |
| DSRM 6-step process | Peffers et al. 2007 | ✅ Ingested |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Ingested |
| KG Quality Assessment | Zaveri et al. 2016 | ✅ Ingested |
| Reflective practice | Schön 1983 | ❌ Not ingested |
| Double-loop learning | Argyris & Schön 1978 | ❌ Not ingested |

## Next Actions

1. Wire grounding check to CI — warn (not block) on drops
2. Detect design doc staleness by tracking code changes vs doc timestamps
3. Extract formal ADRs from design doc "Key Design Decisions" sections
4. Resolve #42 (extraction research) — document findings from GLiNER2/NuExtract testing
5. Ingest Schön's reflective practice and Argyris & Schön's double-loop learning
6. Define DSRM evaluation criteria for Covalence as a design science artifact
