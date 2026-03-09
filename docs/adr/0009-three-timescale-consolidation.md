# ADR-0009: Three-Timescale Consolidation Pipeline

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/01-architecture.md, spec/05-ingestion.md

## Context

Knowledge acquisition and knowledge maturation operate at different timescales. Trying to do everything at ingestion time is too slow; deferring everything loses freshness. Based on Complementary Learning Systems theory (hippocampal-neocortical memory consolidation).

## Decision

Three-timescale pipeline:

- **Online (seconds):** Per-source. Parse, chunk, embed, extract, resolve, store. Incremental confidence updates.
- **Batch (hours):** Group by topic cluster. LLM-compiled articles (200–4000 tokens). Bayesian confidence aggregation. Contention detection. Triggered by timer or epistemic delta > threshold.
- **Deep (daily+):** TrustRank global recalibration. Community detection. BMR forgetting. Cross-domain bridge discovery. Domain topology map.

## Consequences

### Positive

- Fresh ingestion (seconds) without waiting for synthesis
- Articles are right-sized retrieval units (validated by LlamaIndex eval)
- Deep consolidation handles structural maintenance without blocking ingestion
- Epistemic delta threshold prevents unnecessary re-synthesis

### Negative

- Three codepaths to maintain
- Batch compilation requires LLM calls (cost)
- Deep consolidation is computationally expensive (PageRank, community detection over full graph)

## Alternatives Considered

- **Single-pass (all at ingestion):** Too slow, synthesis quality suffers without cross-source context
- **Two-timescale (online + batch):** Misses structural maintenance, forgetting, global calibration
- **Continuous streaming:** Makes entity resolution harder (iText2KG finding)
