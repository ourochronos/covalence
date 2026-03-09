# ADR-0005: Subjective Logic for Confidence Representation

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/07-epistemic-model.md

## Context

A simple float confidence score is ambiguous — 0.5 could mean "equal evidence for and against" or "no evidence at all." The system needs to distinguish between uncertainty (ignorance) and disbelief (negative evidence).

## Decision

Represent confidence as Subjective Logic opinion tuples: ω = (b, d, u, a) where b=belief, d=disbelief, u=uncertainty, a=base_rate, with b+d+u=1. Stored as JSONB `confidence_breakdown`. A `projected_probability` (b + a*u) provides backward compatibility with systems expecting a single float.

## Consequences

### Positive

- Distinguishes "unknown" from "50% likely"
- Mathematically sound fusion operators (cumulative, averaging)
- Enables principled decisions about when to seek more evidence
- Projected probability provides simple float for basic use cases

### Negative

- More complex than a single float
- Requires understanding of Subjective Logic for maintenance
- JSONB storage is less query-efficient than native columns (acceptable for v1)

## Alternatives Considered

- **Simple float [0,1]:** Ambiguous, loses information about evidence quality
- **Confidence interval [low, high]:** Better than float but doesn't model base rates or separate uncertainty from disbelief
- **Full Dempster-Shafer mass functions:** Too complex for per-entity storage, used only during fusion
