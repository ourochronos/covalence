# Design: Change Prioritization & Safe Development

## Status: emergent, formalizing

## Spec Sections: 10-lessons-learned.md

## Architecture Overview

With 76 implemented components, 25 partial, and 52 not-yet, Covalence needs a systematic way to decide what to work on next. This document establishes a risk-driven prioritization framework, safe change practices for a live system, and fitness functions to prevent regression.

## Prioritization Framework

### Risk-Driven Ordering

Priority = Risk × Value, where:
- **Risk** = probability of user-facing impact × severity if it happens
- **Value** = number of downstream components unblocked + grounding improvement

### Priority Tiers

#### P0: High Risk, High Value (do next)
These are blocking other work or actively degrading quality:

| Item | Risk | Value | Rationale |
|------|------|-------|-----------|
| **Incremental graph sync** | High | Unblocks real-time search | Currently requires server restart to see new data |
| **GLiNER sidecar deploy** | Low risk | Unblocks two-pass extraction, reduces LLM cost 50-70% | Code exists, just needs sidecar |
| **Source title/author in create API** | Zero risk | Improves all future ingestion metadata | Trivial change, high impact |
| **Voyage as default embedder** | Medium | Better embedding quality across all operations | Provider exists, needs config change |
| **Entity noise filtering** (#40) | Low risk | Reduces 200+ garbage entities, improves clustering | Min label length + stoplist |

#### P1: Medium Risk, High Value (do soon)
These improve quality significantly but don't block other work:

| Item | Risk | Value | Rationale |
|------|------|-------|-----------|
| **Background consolidation loop** | Medium | Enables organic forgetting, auto-clustering | Scheduler exists, loop doesn't |
| **Confidence_breakdown population** | Low | Enables epistemic explainability | Data computed but not stored |
| **Search feedback → learning** | Low | Closes the feedback loop | Endpoint exists, feedback unused |
| **Eval fixtures generation** | Low | Enables regression testing | Eval crate exists, no data |
| **Wire Voyage reranker** | Low | HttpReranker exists, just needs activation | NoopReranker costs quality |
| **Label embedding cache** | Low | Makes clustering 10x faster on reruns | Currently re-embeds every time |

#### P2: Low Risk, Medium Value (do when convenient)
Good improvements that can wait:

| Item | Risk | Value | Rationale |
|------|------|-------|-----------|
| **Lost in the Middle reordering** | Zero | Better context assembly | Academic foundation in KB |
| **PII gating** | Low | Compliance for multi-user | Pattern detection exists |
| **Landscape analysis surfacing** | Low | Better ingestion quality signals | Metrics computed, not exposed |
| **Bridge node boosting in search** | Low | Better cross-domain results | Bridge discovery exists |
| **Community → search weighting** | Low | Denser subgraphs ranked higher | Communities detected, not used |

#### P3: Deferred / Needs Decision
These require architectural decisions or are explicitly future-scoped:

| Item | Decision Needed | Rationale |
|------|----------------|-----------|
| **Federation** (#35) | In-scope or separate service? | 10 unimplemented components, all blocked on scope |
| **Auth layer** | Local-only or multi-user? | Scope determines complexity |
| **GraphQL** | Needed given MCP? | May be redundant |
| **Streaming responses** | Which operations need SSE? | Nice-to-have |
| **Node2Vec embeddings** | Value vs complexity? | Requires training pipeline |

## Safe Change Practices

### The Strangler Fig Principle
Never big-bang replace. Always:
1. Build the new alongside the old
2. Route traffic incrementally
3. Verify parity
4. Remove the old

Applied: v1→v2 migration runs v1 data through v2 ingestion. v1 continues operating. When v2 has all v1 data + new features, v1 can retire.

### Feature Flags for Partial Features
Components that are implemented but not activated:
- `NoopReranker` → `HttpReranker` (Voyage rerank-2.5) — config flag
- Single-pass → Two-pass extraction — GLiNER sidecar availability as implicit flag
- `TrustRank: false` → `true` — config flag (needs seed set first)

These are effectively feature flags already. Formalize them.

### Database Safety
With a live DB:
- All migrations MUST be idempotent (`IF NOT EXISTS`, `IF NOT EXISTS`)
- All migrations MUST be additive (add columns, not remove them)
- Schema changes separated from data migrations
- Backups before batch operations (v1 ingestion)

### Characterization Tests
Before changing a component, capture its current behavior:
1. Run eval harness → save baseline metrics
2. Make change
3. Run eval harness → compare against baseline
4. Regression gate: fail if any metric drops > threshold

This is the eval crate's regression module — it just needs fixture data.

## Fitness Functions

Automated checks that architecture properties are preserved:

| Fitness Function | Measures | Current Status |
|-----------------|----------|---------------|
| **Grounding %** | Academic backing of spec | ✅ Automated SQL query, run manually |
| **Embedding coverage** | % of chunks/nodes with embeddings | ✅ Can query from DB |
| **Search dimension firing** | All 5 dimensions produce results | ✅ Verified manually |
| **Entity extraction P/R/F1** | Extraction quality | 🟡 Eval crate exists, no fixtures |
| **Search P@K, nDCG** | Search quality | 🟡 Eval crate exists, no fixtures |
| **RAGAS scores** | End-to-end RAG quality | ❌ Stubs only |
| **Abstention rate** | False negative rate | ❌ Not tracked |
| **Cluster quality** | Ontology coherence | ❌ Not tracked |

Goal: run fitness functions in CI on every commit. Start with the ones that work today (grounding, embedding coverage, dimension firing), add more as eval fixtures are created.

## The "Partial → Complete" Playbook

For each of the 25 partial components, the path to completion follows a pattern:

1. **Read the design doc** — understand what's implemented vs missing
2. **Write a characterization test** — capture current behavior
3. **Identify the smallest useful increment** — not "finish everything", but "what's the one change that unlocks the most value?"
4. **Make the change** — commit to main, push
5. **Verify fitness functions** — no regression
6. **Update the design doc** — move item from 🟡 to ✅

This is single-loop learning applied to each component. The design doc update triggers the flywheel (double-loop) — new connections in the graph may reveal further improvements.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| Technical debt | Kruchten et al. 2012 | 🔄 Worker ingesting |
| Strangler fig | Fowler 2004 | 🔄 Worker ingesting |
| DORA metrics | Forsgren et al. 2018 | 🔄 Worker ingesting |
| Risk-driven architecture | Fairbanks 2010 / ATAM | 🔄 Worker ingesting |
| Continuous delivery | Humble & Farley 2010 | 🔄 Worker ingesting |
| Evolutionary architecture | Ford et al. 2017 | 🔄 Worker ingesting |
| Reflective practice | Schön 1983 | 🔄 Worker ingesting |
| Double-loop learning | Argyris & Schön 1978 | 🔄 Worker ingesting |

## Next Actions

1. Start P0 items in order: entity noise filtering → source title/author → Voyage default → GLiNER sidecar → incremental sync
2. Generate eval fixtures from current corpus (characterization tests)
3. Wire grounding % as CI fitness function
4. Formalize feature flags for partially-active components
5. Backup DB before v1 batch ingestion
