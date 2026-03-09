# ADR-0001: Hybrid Property Graph + Provenance View

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/02-data-model.md

## Context

The system needs to represent knowledge as a graph. Three options exist: pure property graph, pure triples (RDF-style), or a hybrid. The existing valence-v2 used pure triples, which proved verbose and join-heavy.

## Decision

Use a property graph as the primary model (nodes and edges with JSONB properties). For fine-grained provenance and epistemic operations, a `provenance_triples` SQL view decomposes the property graph into (subject, predicate, object) triples on-the-fly.

## Consequences

### Positive

- Property graph is natural for petgraph, no join overhead for properties
- Familiar to most developers
- Triple decomposition via SQL view is zero-maintenance (no sync)
- Can promote to `MATERIALIZED VIEW` if performance warrants

### Negative

- Loses some composability of pure triples
- View-based triples may be slower than native triple stores for complex SPARQL-like queries

## Alternatives Considered

- **Pure triples (valence-v2 approach):** Maximally composable but verbose (node with 5 properties = 5+ triples) and join-heavy
- **Pure property graph:** Loses fine-grained per-fact provenance
