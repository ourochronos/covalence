# Design: Epistemic Model

## Status: implemented (core), partial (propagation wiring)

> **Updated 2026-03-10**: No major code changes in this subsystem during the March 10 engineering
> wave. All 5 epistemic stages remain as documented on March 9. The main outstanding gaps
> (TrustRank seed set, confidence_breakdown population, forgetting trigger) are unchanged.

## Spec Sections: 07-epistemic-model.md, 04-graph.md

## Architecture Overview

The epistemic model provides mathematically grounded confidence scoring for all knowledge claims. It implements a 5-stage pipeline: Dempster-Shafer evidence fusion → Subjective Logic cumulative fusion → DF-QuAD contradiction handling → supersession/correction decay → TrustRank calibration. The system explicitly tracks belief, disbelief, and uncertainty rather than collapsing to a single probability.

## Implemented Components

### Fully Implemented ✅

| Component | File | Notes |
|-----------|------|-------|
| **Subjective Logic opinions** | `types/opinion.rs` | Full opinion quadruple (b, d, u, a) with projected probability |
| **Dempster-Shafer combination** | `epistemic/fusion.rs` | Standard DS rule with conflict detection (K=1 → total conflict → None) |
| **Cumulative fusion** | `epistemic/fusion.rs` | Jøsang's formula, commutative+associative, handles dogmatic case |
| **DF-QuAD contradiction** | `epistemic/contradiction.rs` | Attack strength computation, circular attack resolution |
| **Epistemic convergence** | `epistemic/convergence.rs` | Fixed-point iteration with epsilon guard, max iterations |
| **Supersession/correction decay** | `epistemic/decay.rs` | `apply_supersedes()`, `apply_corrects()`, `apply_append()` |
| **Epistemic delta** | `epistemic/delta.rs` | Change detection between propagation rounds |
| **Organic forgetting** | `epistemic/forgetting.rs` | ACT-R base-level activation, BMR (Bayesian Memory Reconsolidation) weights |
| **Composite confidence** | `epistemic/confidence.rs` | `projected_probability * (1 + gamma * (topo_confidence - 0.5))` |
| **Bayesian aggregation** | `epistemic/confidence.rs` | Beta distribution conjugate updating for multi-source confidence |
| **5-stage propagation pipeline** | `epistemic/propagation.rs` | Full pipeline orchestration with convergence guard |
| **Invalidation** | `epistemic/invalidation.rs` | Edge/claim invalidation logic |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **TrustRank calibration** | Stage 5 exists but `apply_trust_rank: false` by default | Needs seed set definition and iterative propagation wiring |
| **Topological confidence** | Used in composite_confidence but computed locally | No graph-wide iterative propagation (EigenTrust-style) |
| **Confidence breakdown on edges** | Schema has `confidence_breakdown JSONB` | Not populated during ingestion — always NULL |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **Iterative trust propagation** | Spec 07: "Two-phase propagation with damping" | High — core to epistemic integrity at scale |
| **Belief oscillation detection** | Spec 07: "belief oscillation" | Medium — guard against feedback loops |
| **Network-wide calibration** | Spec 07: "network-wide calibration" | Medium — global consistency check |
| **Epistemic delta triggers** | Spec 07: significant delta → re-propagation | Medium — currently manual |
| **Federated trust discounting** | Spec 09: discount opinions by federation trust level | Low — blocked on federation (#35) |

## Key Design Decisions

### Why Subjective Logic over Bayesian
Bayesian probability collapses to P(x) = 0.76. Subjective Logic preserves the epistemic state:
- ω = (0.76, 0.24, 0.0, 0.5) → certain, well-evidenced
- ω = (0.0, 0.0, 1.0, 0.76) → completely uncertain, relying on base rate
Same projected probability, vastly different epistemic meaning. This distinction is critical for abstention — a system should decline to answer when uncertainty is high, even if the projected probability looks reasonable.

### Why DF-QuAD for contradictions
Discontinuity-Free Quantitative Argumentation Debate handles circular attacks gracefully via fixed-point iteration. Real knowledge graphs have cycles — paper A contradicts paper B which supersedes paper C which confirms paper A. DF-QuAD converges to stable opinions under these conditions.

### Why ACT-R for forgetting
The forgetting curve follows Anderson's ACT-R base-level activation: B_i = ln(Σ t_j^(-d)). Entities accessed recently and frequently have higher activation. This gives biologically-inspired decay that naturally handles "important but old" vs "trivial but recent."

### Why 5 stages instead of a single formula
Each stage handles a qualitatively different epistemic operation:
1. DS fusion: independent evidence combination
2. Cumulative fusion: confirmation boost (same claim, multiple sources)
3. DF-QuAD: contradiction/attack handling
4. Decay: temporal supersession
5. TrustRank: source reliability calibration

Collapsing these into one formula would lose the ability to diagnose *why* a confidence score is what it is. The staged pipeline provides explainability.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| Dempster-Shafer theory | Shafer 1976 | ✅ Ingested |
| Subjective Logic | Jøsang 2016 | ✅ Ingested |
| DF-QuAD | Rago et al. 2016 | ❌ Not ingested — should be |
| ACT-R memory model | Anderson 1998 | ❌ Not ingested — should be |
| TrustRank | Gyöngyi et al. 2004 | ✅ Ingested |
| EigenTrust | Kamvar et al. 2003 | ✅ Ingested |
| Pearl's Causal Hierarchy | Pearl 2000 | ✅ Ingested |
| Beta-binomial conjugate | Standard Bayesian | ❌ No paper — textbook material |

## Gaps Identified

1. **confidence_breakdown JSONB is always NULL** — the schema supports it but ingestion never
   populates it. The 5-stage pipeline's intermediate results are lost.

2. **TrustRank disabled by default** — `apply_trust_rank: false`. No seed set defined. Needs a
   way to designate "trusted" sources (operator-authored articles?).

3. **DF-QuAD paper not in KB** — the implementation cites it but the paper hasn't been ingested.
   The graph can't trace the academic foundation of the contradiction system.

4. **Organic forgetting is computed but not triggered** — `bmr_analysis()` and
   `eviction_decision()` exist but aren't wired to a scheduled job.

5. **No connection between epistemic model and evaluation spec** — how do you evaluate epistemic
   correctness without referencing epistemic concepts? Calibration metric needed.

## Next Actions

1. Populate `confidence_breakdown` JSONB during ingestion — store per-stage opinions
2. Define TrustRank seed set criteria and enable stage 5
3. Ingest DF-QuAD paper (Rago et al. 2016) and ACT-R memory model (Anderson 1998)
4. Wire forgetting to a consolidation schedule
5. Add epistemic-specific metrics to 11-evaluation.md
