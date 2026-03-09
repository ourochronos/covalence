# ADR-0012: Louvain for Community Detection

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/04-graph.md

## Context

Community detection identifies clusters of related nodes, used for dynamic ontology discovery, hierarchical summarization, query scoping, and article compilation boundaries.

## Decision

Use the Louvain algorithm for community detection. Computed during deep consolidation (daily+) and on-demand when triggered by high epistemic delta. Communities are ephemeral — stored in sidecar memory, not in PG.

## Consequences

### Positive

- Industry standard with excellent performance
- Produces multi-level hierarchy naturally (maps to domain topology)
- Good Rust implementations available
- Ephemeral storage keeps PG schema simple

### Negative

- Non-deterministic (different runs may produce slightly different communities)
- Resolution limit: may miss small communities within large ones
- Full recomputation needed (no incremental Louvain)

## Alternatives Considered

- **Label Propagation:** Faster but less stable, no hierarchy
- **Leiden:** Theoretically better than Louvain (guaranteed connected communities) but less mature in Rust
- **Spectral clustering:** More principled but O(n³) for eigendecomposition
