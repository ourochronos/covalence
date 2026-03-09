# ADR-0006: Reciprocal Rank Fusion for Search

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/06-search.md

## Context

Search combines five dimensions (vector, lexical, temporal, graph, structural) that produce scores on different scales. A fusion method is needed that doesn't require score normalization.

## Decision

Use Reciprocal Rank Fusion (RRF) with K=60 and configurable per-dimension weights. Each dimension produces a ranked list independently; RRF merges them using: `RRF_score(d) = Σ weight_i / (K + rank_i(d))`.

## Consequences

### Positive

- No score normalization needed across dimensions
- Simple to implement and debug
- Proven effective (used successfully in existing valence)
- Per-strategy weight profiles enable different query types

### Negative

- Rank-based fusion loses magnitude information (a much-better #1 result looks the same as a barely-better #1)
- K=60 is a magic constant (well-established but not theoretically derived)
- Adding new dimensions requires rebalancing weights

## Alternatives Considered

- **Score normalization + weighted sum:** Requires comparable score scales, fragile
- **Learning-to-rank:** Better in theory, needs labeled training data we don't have
- **CombMNZ:** More complex, marginal improvement over RRF in practice
