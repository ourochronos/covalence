# Design: Development Process

## Status: emergent, being formalized now

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

Measured over a single session (2026-03-09, ~20 minutes):

| Metric | Before | After | Δ |
|--------|--------|-------|---|
| Spec grounding (median) | ~20% | ~52% | +32pp |
| Sources | 45 | 88+ | +43 |
| Knowledge graph nodes | ~1,050 | 1,807 | +757 |
| Knowledge graph edges | ~1,796 | 3,010 | +1,214 |
| Design docs | 1 | 8 | +7 |
| Subsystems with documented status | 1 | 8 | all |

The worst-grounded spec section (08-api at 14.4%) improved to 49.3%. The most isolated section (11-evaluation at 21.9%) jumped to 65.6%.

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

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **ADR format** | Design docs capture decisions | Not using formal Nygard/Zdun ADR template with Status/Consequences |
| **Regression gating** | Eval crate has regression baselines | Not wired to CI — can't block merges on quality drops |
| **DSRM alignment** | We follow Hevner's guidelines informally | Not explicitly mapped to the 6-step DSRM process |

### Not Implemented ❌

| Component | Source | Priority |
|-----------|--------|----------|
| **Automated gap detection** | #39 | High — currently manual SQL; should be API endpoint |
| **Grounding CI check** | — | Medium — fail build if grounding drops below threshold |
| **Design doc staleness detection** | — | Medium — detect when code changes invalidate a design doc |
| **Process metrics dashboard** | — | Low — track grounding %, node/edge counts, worker success rate over time |
| **Automated paper suggestions** | — | Low — graph analysis suggests papers to ingest |
| **DSRM evaluation phase** | Hevner 2004 | Medium — formal evaluation criteria for each design artifact |

## Key Design Decisions

### Why the graph reviews itself
Covalence is a knowledge graph engine. Using it to manage its own development means every improvement to the engine immediately improves the development process. The system and its development co-evolve.

### Why design docs over ADRs (for now)
ADRs capture individual decisions. Design docs capture the full status of a subsystem: what works, what doesn't, what's next. They're broader and more useful for gap analysis. ADRs could be extracted from design docs later — each "Key Design Decision" section is a proto-ADR.

### Why grounding percentage as the primary metric
Grounding measures how much of the spec is backed by academic literature. It's objective (entity overlap), automated (SQL query), and actionable (low grounding → ingest more papers on that topic). It's not perfect — entity matching is noisy — but it's directionally correct and computable in milliseconds.

### Why async paper workers
Paper research, synthesis, and ingestion takes 5-7 minutes per batch. Running workers in parallel while continuing analysis means no idle time. The flywheel spins continuously.

### Why immediate commit
Design docs are artifacts of analysis, not proposals. They describe what IS, not what should be. No review gate needed — they're documentation of measured reality. Push immediately, iterate later.

## Process Patterns Observed

### The Citation Gap Pattern
When a design doc identifies a concept referenced in code but absent from the KB, that's a citation gap. Filling it always improves grounding, often by 5-15 percentage points. Example: ingesting RAGAS + Zaveri pushed evaluation grounding from 31% to 66%.

### The Bridge Pattern
Papers that introduce concepts used across multiple spec sections have outsized impact. "Attention Is All You Need" bridges search, ingestion, and evaluation. Prioritize bridge papers over section-specific ones.

### The Isolation Pattern
Spec sections that share few entities with other sections are either (a) genuinely independent or (b) missing connections. 11-evaluation was isolated because it lacked references to the specific systems it should evaluate. Design docs fix isolation by explicitly mapping connections.

### The Sink Pattern
Graph entities with many incoming edges but no outgoing edges are knowledge dead-ends. Filling these (e.g., "Subjective Logic Opinions" went from 11in/0out to 14in/3out after ingesting the paper) enriches the graph's explanatory power.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| Architecture Decision Records | Nygard 2011 | ✅ Just ingested |
| Design Science Research | Hevner et al. 2004 | ✅ Just ingested |
| DSRM 6-step process | Peffers et al. 2007 | ✅ Just ingested |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Just ingested |
| KG Quality Assessment | Zaveri et al. 2016 | ✅ Just ingested |
| Reflective practice | Schön 1983 | ❌ Not ingested |
| Double-loop learning | Argyris & Schön 1978 | ❌ Not ingested |

## Next Actions

1. Automate grounding analysis as API endpoint (#39)
2. Add grounding check to CI — warn (not block) on drops
3. Detect design doc staleness by tracking code changes vs doc timestamps
4. Extract formal ADRs from design doc "Key Design Decisions" sections
5. Ingest Schön's reflective practice and Argyris & Schön's double-loop learning — the theoretical foundation for a system that improves its own process
6. Define DSRM evaluation criteria for Covalence as a design science artifact
