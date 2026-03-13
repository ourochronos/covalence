# ADR-0017: Cross-Domain Analysis Capabilities

**Status:** Accepted

**Date:** 2026-03-12

**Spec Reference:** spec/13-cross-domain-analysis.md

## Context

With ADR-0015 (statement-first extraction) and ADR-0016 (AST-aware code ingestion + Component bridge layer), Covalence's graph contains three semantic domains: research (academic papers), spec (design docs and specifications), and code (source code with semantic summaries). These domains are connected via Component bridge nodes with typed edges (IMPLEMENTS_INTENT, PART_OF_COMPONENT, THEORETICAL_BASIS).

The graph now has the structural prerequisites to answer questions that cross domain boundaries. The question is: what analysis capabilities should we build, and how should they be exposed?

## Decision

Build six cross-domain analysis capabilities, exposed as API endpoints under `/api/v1/analysis/`:

### 1. Research-to-Execution Verification (`/verify-implementation`)

Trace from an academic concept through spec topics and components to the code that implements it. Compare research statements against code semantic summaries to identify alignment and divergence.

**Why this matters:** Confirms that code faithfully implements the theoretical approach it claims to use, or surfaces specific deviations for review.

### 2. Architecture Erosion Detection (`/erosion`)

Measure semantic drift between Component descriptions (derived from specs) and their code entities' semantic summaries. When code evolves away from its spec, the cosine distance increases.

**Why this matters:** Makes invisible tech debt mathematically visible. Developers modify code in ways that gradually diverge from design intent — this makes that drift measurable and alertable.

**Metric:** `drift(component) = 1 - mean(cosine(component.embedding, code_node.embedding))` for all code nodes linked via PART_OF_COMPONENT.

### 3. Whitespace Roadmap (`/whitespace`)

Find dense research clusters with no corresponding Component or Spec Topic links. These are areas of theory we've studied but haven't designed or built — the unbridged voids.

**Why this matters:** The graph maps what exists. By finding research clusters with no bridge edges, we map what doesn't exist — producing a data-driven roadmap of missing capabilities.

### 4. Blast-Radius Simulation (`/blast-radius`)

Given a code entity, traverse the graph via CALLS, USES_TYPE, PART_OF_COMPONENT, IMPLEMENTS_INTENT, and THEORETICAL_BASIS edges to compute the full impact of modifying that entity.

**Why this matters:** Traditional impact analysis follows import chains. Graph-based blast radius follows semantic chains — revealing that changing a function affects not just its callers, but potentially invalidates a spec topic and contradicts a research foundation.

### 5. Dialectical Critique (`/critique`)

Given a design proposal, search the graph for competing approaches, contradicting claims, and conflicting implementations. Synthesize a structured counter-argument citing specific research statements and code realities.

**Why this matters:** The system acts as an adversarial design partner. Because it tracks claims from multiple research paradigms, it can steelman arguments against a proposed approach using the user's own ingested knowledge base.

### 6. Coverage Analysis (`/coverage`)

Find orphan code (code entities with no path to any Spec Topic) and unimplemented specs (Spec Topic nodes with no IMPLEMENTS_INTENT edges).

**Why this matters:** Simple graph traversal reveals structural voids. Orphan code is undocumented tech debt or rogue features. Unimplemented specs are design commitments that haven't been built.

## Consequences

### Positive

- **Self-awareness.** The system can reason about its own implementation quality, design fidelity, and knowledge gaps.
- **Principled prioritization.** Whitespace roadmap and coverage analysis produce data-driven work items instead of ad-hoc gap identification.
- **Confidence in changes.** Blast-radius simulation enables confident refactoring — you know what you'll break before you break it.
- **Continuous design integrity.** Erosion detection runs automatically on re-ingestion, surfacing drift as it happens.
- **Better design decisions.** Dialectical critique uses the system's own research knowledge to challenge proposals.

### Negative

- **Compute cost.** Erosion detection and whitespace analysis require embedding comparisons across potentially large node sets.
- **Quality depends on graph quality.** Analysis is only as good as the bridge edges. If Component linking is incomplete, coverage analysis will report false gaps.
- **LLM cost for critique.** The dialectical capability requires LLM synthesis to produce counter-arguments (graph traversal alone can find competing claims, but synthesis requires generation).
- **API surface area.** Six new endpoints to maintain, test, and document.

## Alternatives Considered

### 1. Expose only as CLI commands, not API endpoints
The analysis capabilities could be CLI-only tools that query the graph directly. Rejected because the analysis results should be cacheable, shareable, and accessible from the dashboard — all of which require API endpoints.

### 2. Batch-only analysis (run during consolidation)
Run analysis passes during deep consolidation and store results as Articles. This would reduce API complexity but makes the analysis stale — drift detection is most valuable when it runs on the latest ingested code. Rejected for staleness.

### 3. Build only coverage analysis (simplest capability)
Coverage analysis is pure graph traversal — no embeddings, no LLM. Could ship this first and defer the rest. This is actually the implementation approach (see spec/13: Phase 4), but the architectural decision should encompass all six capabilities to ensure the data model and graph algorithms support them all.
