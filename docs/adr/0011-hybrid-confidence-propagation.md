# ADR-0011: Hybrid Multi-Stage Confidence Propagation

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/07-epistemic-model.md

## Context

No single algorithm handles all aspects of confidence propagation. Dempster-Shafer handles multi-source fusion, Subjective Logic handles confirmation, DF-QuAD handles contradiction, and TrustRank handles global calibration. Using only one framework leaves gaps.

## Decision

Five-stage hybrid pipeline:

1. **Local Fusion (Dempster-Shafer):** Multi-source evidence combination during entity resolution.
2. **Confirmation (Subjective Logic cumulative fusion):** CONFIRMS edges reduce uncertainty, increase belief.
3. **Contradiction (DF-QuAD):** CONTRADICTS/CONTENDS edges with gradual degradation, fixed-point for cycles.
4. **Supersession/Correction:** SUPERSEDES proportional decay, CORRECTS immediate zeroing.
5. **Global Calibration (TrustRank):** Eigenvector-based batch computation during deep consolidation.

Two-phase execution: local evidence aggregation (stages 1-2) reaches steady state before structural belief revision (stages 3-5). Convergence guard with damping prevents oscillation.

## Consequences

### Positive

- Each algorithm handles what it's best at
- Convergence guard prevents epistemic oscillation between frameworks
- Phased implementation possible (stages 1-2 first, add 3-5 later)
- All epistemic updates computed in-memory, flushed in batch transaction

### Negative

- Most complex subsystem — five algorithms to implement and test
- Ordering matters (must run stages sequentially)
- Convergence tuning (epsilon, damping) requires empirical testing

## Alternatives Considered

- **Single algorithm (e.g., just Bayesian):** Doesn't handle contradictions or global network effects
- **PageRank-only trust:** Misses local evidence quality, confirmation semantics
