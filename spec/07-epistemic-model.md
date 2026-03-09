# 07 — Epistemic Model

**Status:** Draft

## Overview

The epistemic model governs how the system reasons about the trustworthiness, currency, and consistency of its knowledge. It synthesizes five theoretical frameworks applied to knowledge management, drawing from research across ~20 domains captured in Covalence's knowledge base.

The core thesis: confidence propagation requires a **hybrid multi-stage approach** — no single algorithm handles all edge types and scenarios. Different theoretical frameworks address different aspects of epistemic reasoning.

## Confidence

### Representation: Subjective Logic Opinions

Confidence is not a simple float. Following Jøsang's Subjective Logic, each belief is represented as an opinion tuple:

```
ω = (b, d, u, a)
```

Where:
- `b` = belief (degree of positive evidence)
- `d` = disbelief (degree of negative evidence)
- `u` = uncertainty (degree of ignorance)
- `a` = base rate (prior probability absent evidence)
- Constraint: `b + d + u = 1`

**Why this matters:** A simple confidence score of 0.5 is ambiguous — it could mean "equal evidence for and against" (`b=0.4, d=0.4, u=0.2`) or "no evidence at all" (`b=0, d=0, u=1.0, a=0.5`). Subjective Logic distinguishes these cases, which is critical for deciding when to seek more evidence vs. when to accept ambiguity.

**Storage:** The opinion tuple is stored as a JSONB `confidence_breakdown` column:

```json
{
  "belief": 0.7,
  "disbelief": 0.1,
  "uncertainty": 0.2,
  "base_rate": 0.5,
  "projected_probability": 0.8
}
```

The `projected_probability` (`b + a * u`) provides backward compatibility with systems that expect a single confidence float.

### Source Confidence

Every source carries a trust prior, updated via the Beta-binomial model:

```
trust(source) = Beta(α, β)
```

- α increments on confirmations, β increments on contradictions
- Initial priors by source type:

| Source Type | Initial α | Initial β | Prior Mean |
|------------|----------|----------|------------|
| Document (published) | 4 | 1 | 0.80 |
| Tool output | 3.5 | 1.5 | 0.70 |
| Web page | 3 | 2 | 0.60 |
| Conversation | 2.5 | 2.5 | 0.50 |
| Observation | 2 | 3 | 0.40 |

These priors are updated as the system accumulates evidence about source reliability.

### Extraction Confidence

How certain we are about a specific extracted fact:

```
extraction.confidence ∈ [0.0, 1.0]
```

Set by the LLM during extraction. Low confidence extractions can be flagged for review.

### Topological Confidence

Computed dynamically from graph structure (not stored):

```
topo_confidence(node) = α * normalized_pagerank(node) + β * path_diversity(node)
```

- α = 0.6, β = 0.4
- Nodes mentioned by many sources via diverse paths have higher topological confidence
- See [04-graph](04-graph.md) for details

### Composite Confidence

For query results, confidence is composed from multiple signals. The projected probability from the opinion tuple serves as the primary score, modified by topological confidence:

```
composite = projected_probability(opinion) * (1 + γ * (topo_confidence - 0.5))
```

Where γ controls how much graph topology influences the final score (default: 0.4).

## Confidence Propagation: Hybrid Multi-Stage Pipeline

Confidence propagation uses different algorithms for different scenarios, applied in stages:

### Stage 1: Local Fusion — Dempster-Shafer Theory

When multiple sources contribute to the same claim, combine their evidence:

```
m_combined(A) = (1 / (1 - K)) * Σ m₁(B) × m₂(C)  for all B ∩ C = A
```

Where K is the conflict mass (degree of disagreement between sources).

- Handles source conflicts via conflict mass normalization
- Produces confidence + uncertainty interval
- Applied during entity resolution when multiple extractions refer to the same node

### Stage 2: Confirmation Boost — Subjective Logic Cumulative Fusion

`CONFIRMS` edges apply cumulative fusion — independent confirmations reduce uncertainty:

```
ω_fused = ω_existing ⊕ ω_confirming
```

- Multiple independent confirmations compound, asymptotically approaching certainty
- Each confirmation reduces `u` (uncertainty) while increasing `b` (belief)
- The fusion operator preserves commutativity and associativity

### Stage 3: Contradiction Handling — DF-QuAD

`CONTRADICTS` and `CONTENDS` edges use the Discontinuity-Free Quantitative Argumentation Debate framework:

```
confidence(A) *= (1 - attack_strength)
attack_strength = confidence(B) × edge_weight
```

Where B is the contradicting claim.

- **Gradual degradation** — avoids "confidence cliffs" where a single contradiction zeroes out a well-established claim
- Fixed-point iteration resolves circular attacks (mutual contradictions)
- `CONTRADICTS` uses full attack weight; `CONTENDS` uses 0.3× attack weight

### Stage 4: Supersession / Correction Decay

Temporal edges apply direct confidence modification:

| Edge Type | Formula | Effect |
|-----------|---------|--------|
| `SUPERSEDES` | `conf(old) *= (1 - conf(new) × weight)` | Proportional reduction. Full supersession (weight=1, conf=1) approaches zero. |
| `CORRECTS` | `conf(old) = 0` | Immediate zeroing. Explicit retraction. |
| `APPENDED_AFTER` | No change to existing | Additive only — prior claims unmodified. |

### Stage 5: Global Calibration — TrustRank (Batch)

Periodic batch computation (deep consolidation tier) for network-wide calibration:

- Compute eigenvector of the trust matrix over the full graph
- Captures network effects: many weak sources supporting a claim may be more trustworthy than one strong source (diversity of evidence)
- Handles cycles naturally via matrix convergence with damping
- Seed set: manually verified high-confidence nodes
- See [04-graph](04-graph.md) for algorithm details

### Convergence Guard: Preventing Epistemic Oscillation

The five frameworks have different mathematical axioms. Applying them sequentially without convergence control can cause belief oscillation — e.g., a `CONTRADICTS` edge (Stage 3) lowers confidence, then TrustRank (Stage 5) boosts it back due to structural connectivity.

**Solution:** Two-phase propagation with damping:

1. **Local Evidence Aggregation (Stages 1–2)** — Atomic, per-transaction. Must reach steady state before structural updates.
2. **Structural Belief Revision (Stages 3–5)** — Run after local aggregation converges. Use fixed-point iteration with damping:

```rust
fn compute_epistemic_closure(graph: &Graph, seeds: &[Uuid], epsilon: f64) -> HashMap<Uuid, SubjectiveOpinion> {
    let mut opinions = current_opinions(graph, seeds);
    loop {
        let new_opinions = apply_stages_3_4_5(&graph, &opinions);
        let max_delta = opinions.iter()
            .map(|(id, old)| (old.projected() - new_opinions[id].projected()).abs())
            .max();
        opinions = new_opinions;
        if max_delta < epsilon { break; }
    }
    opinions
}
```

All epistemic updates are computed in-memory first, then flushed to PG in a single batch transaction. Never update confidence scores sequentially during traversal.

### Update Strategy

| Trigger | Scope | Stages Applied | Phase |
|---------|-------|---------------|-------|
| Single source ingested | Local neighborhood | 1 (fusion), 2 (confirmation) | Local Aggregation |
| Ingestion with conflicts | Affected subgraph | 3 (contradiction), 4 (supersession) | Structural Revision (converged) |
| Batch compilation | Topic cluster | 1–4, plus confidence aggregation | Both phases |
| Deep consolidation (scheduled) | Full graph | 5 (TrustRank global recalibration) | Global Calibration |
| Critical change (high epistemic delta) | Affected subgraph | 1–4 with priority queue | Both phases, immediate |

## Provenance

Every fact in the system traces back to its origins:

```
Node/Edge → Extraction → Chunk → Source
```

**Provenance chain queries:**
- "Where did this fact come from?" → Follow extraction → chunk → source
- "What did this source contribute?" → Follow source → chunks → extractions → nodes/edges
- "How many independent sources support this?" → Count distinct sources for a node's extractions
- "Why does this claim have low confidence?" → Inspect `confidence_breakdown` and trace contributing edges

**Explainability:** The `confidence_breakdown` JSONB column tracks the contribution from each source and edge type, enabling explanations like "Based on 3 confirming sources (highest trust: 0.85), with 1 contention (trust: 0.60)."

## Edge Semantics

Typed edges carry epistemic meaning at multiple levels. Edge types are organized by Pearl's Causal Hierarchy.

### Edge Type Vocabulary

**L0 — Associational** (correlational, no causal claim):
- `RELATED_TO` — Topical proximity, bridge concept
- `PART_OF` — Compositional relationship
- `INSTANCE_OF` — Type membership
- `HAS_PROPERTY` — Attribute relationship
- `MENTIONED_IN` — Chunk ↔ Node linkage

**L1 — Interventional** (causal, evidential):
- `CAUSED_BY` — Direct causal relationship
- `ENABLED` — Necessary but not sufficient condition
- `ORIGINATES` — Source is the generative cause of a claim
- `CONFIRMS` — Independent evidence increases confidence
- `CONTRADICTS` — Evidence is inconsistent with claim
- `CONTENDS` — Disputable challenge, partial disagreement
- `SUPERSEDES` — Newer information replaces older
- `CORRECTS` — Explicit retraction (strongest epistemic signal)
- `APPENDED_AFTER` — Temporal sequence in append-only sources

**L2 — Counterfactual** (hypothetical reasoning):
- `CAUSAL` — Explicit causal claim with Pearl-level annotation

### Edge Metadata

All edges carry temporal metadata:

```
edge.properties.valid_from: Timestamp     -- when the relationship became true
edge.properties.valid_until: Timestamp?   -- when it ceased (null = still valid)
edge.properties.recorded_at: Timestamp    -- when the system learned of it
```

Causal edges (L1+) carry additional metadata:

```
edge.properties.causal_level: "association" | "intervention" | "counterfactual"
edge.properties.causal_strength: Float    -- [0,1] probability of genuine causation
edge.properties.direction_confidence: Float -- [0,1] confidence in edge direction
edge.properties.evidence_type: String     -- "structural_prior" | "experimental" | "observational" | "llm_extracted" | "temporal"
edge.properties.hidden_conf_risk: String  -- "low" | "medium" | "high" | "unknown"
```

### Edge Confidence Impact

| Edge Type | Confidence Impact on Target | Mechanism |
|-----------|---------------------------|-----------|
| `CONFIRMS` | Uncertainty reduced, belief increased | Subjective Logic cumulative fusion |
| `CONTENDS` | Attack at 0.3× weight | DF-QuAD gradual degradation |
| `CONTRADICTS` | Attack at 1.0× weight | DF-QuAD gradual degradation |
| `SUPERSEDES` | `conf *= (1 - new_conf × weight)` | Proportional temporal decay |
| `CORRECTS` | `conf = 0` | Immediate zeroing |

### Contradiction Detection

When a new extraction potentially conflicts with existing knowledge:

1. **Detection** — Vector similarity flags semantically similar nodes/edges; LLM checks for contradiction vs correction vs contention
2. **Classification** — Determine edge type:
   - Source explicitly retracts → `CORRECTS`
   - Direct factual conflict → `CONTRADICTS`
   - Partial or interpretive disagreement → `CONTENDS`
   - Newer version of same source → `SUPERSEDES`
3. **Recording** — Create the appropriate epistemic edge
4. **Entrenchment evaluation** — Compare epistemic entrenchment of existing vs new claim:
   ```
   Entrench(claim) = w_t × trust(sources) + w_s × structural_importance + w_c × corroboration_count + w_r × recency
   ```
5. **Resolution** — Based on AGM belief revision:
   - If `trust(new_source) < threshold`: Register contention, don't revise. Queue for review.
   - If `Entrench(existing) > Entrench(new)`: Accept challenge, mark as contested, gather corroboration.
   - If `Entrench(new) > Entrench(existing)`: Perform Levi Identity revision — contract old, expand with new. Propagate via TMS.
   - If approximately equal: Accept both perspectives, annotate with conflicting views, escalate for review.

### TMS Cascade

On source retraction, all claims whose support sets contained only the retracted source are marked stale. Downstream claims connected via `ORIGINATES` or `CONFIRMS` edges are transitively re-evaluated. This implements dependency-directed backtracking — the precise mechanism of ATMS assumption retraction.

## Temporal Validity

Facts may have temporal bounds via edge metadata:

```
edge.properties.valid_from: Timestamp
edge.properties.valid_until: Timestamp
```

Example: "Tim Cook is CEO of Apple" has `valid_from: 2011-08-24` and no `valid_until` (still valid).

Bitemporal queries are supported:
- **Valid time** — When the relationship was true in the world
- **Transaction time** — When the system recorded it (`recorded_at`)

This enables:
- Point-in-time queries ("Who was CEO in 2010?")
- Change detection ("What changed between Q1 and Q2?")
- Staleness detection (facts with very old `last_seen` and no recent corroboration)
- Version history ("Show me how this documentation evolved")
- Audit trail ("When did the system first learn about this?")

## Epistemic Delta

The epistemic delta measures how much a knowledge cluster has shifted due to new or updated information. See [05-ingestion](05-ingestion.md#epistemic-delta-threshold) for the ingestion trigger.

```
epistemic_delta(cluster) = Σ |confidence_change(claim)| for all affected claims
```

This metric is also useful for:
- **Alerting** — Notify users when a topic they follow has shifted significantly
- **Prioritizing review** — Surface the most epistemically unstable areas of the knowledge base
- **Audit trail** — Track how the knowledge base's beliefs evolve over time
- **Consolidation trigger** — Large deltas trigger batch re-compilation of affected articles

## Forgetting as Bayesian Model Reduction

Forgetting is not a failure — it is a mathematically principled optimization that increases model evidence by reducing complexity. Based on Friston & Zeidman (2018):

```
Retain_score = log p(observations | full_model) - log p(observations | model_without_artifact)
```

An artifact (node, edge, article) can be safely forgotten when `Retain_score < ε` — when the posterior belief about it is indistinguishable from its prior, meaning no learning occurred.

**Three-tier eviction priority:**
1. **Prune first** — Artifacts where posterior ≈ prior (no information gained)
2. **Prune second** — Low structural importance × low trust × low recency
3. **Archive, do not prune** — High structural importance (high EWC weight) OR high corroboration, regardless of recency

**Generalization discovery:** When BMR finds that `log p(observations | simple_rule) > log p(observations | many_specific_facts)`, the specific facts can be archived in favor of the general rule. This is how the system develops wisdom — replacing episodic specifics with semantic generalizations.

Decay mechanisms at query time (non-destructive):
1. **Access-based** — Facts never retrieved gradually lose prominence
2. **Age-based** — Facts from old sources with no recent corroboration decay
3. **Supersession-based** — Superseded facts ranked below replacements
4. **Correction-based** — Corrected facts fully suppressed (confidence = 0)

## Implementation Phases

The epistemic model is the highest-complexity subsystem. Implement in phases to manage risk:

**Phase 1 (Core):** Subjective Logic opinion tuples + projected probability. Source trust via Beta-binomial. Confidence stored but propagation is simple (extraction confidence × source reliability).

**Phase 2 (Confirmation/Contradiction):** Subjective Logic cumulative fusion for `CONFIRMS` edges. DF-QuAD for `CONTRADICTS`/`CONTENDS`. Convergence guard with fixed-point iteration.

**Phase 3 (Global):** TrustRank batch computation during deep consolidation. Dempster-Shafer for multi-source fusion during entity resolution.

**Phase 4 (Advanced):** Bayesian Model Reduction for forgetting. BMR `Retain_score` computation should use a **sampled approximation** — evaluate against a random subset of recent observations (last 1000 queries touching the artifact) rather than the full observation set, to keep computation tractable on large graphs.

### Organic Forgetting Lifecycle

Forgetting is not optional — without it, the graph grows unbounded. Concrete schedule:

**Continuous (per-query):**
- Track access: every search hit increments `access_count` and updates `last_accessed_at` on chunks/nodes/articles
- ACT-R base level `B_i = ln(Σ t_k^{-0.5})` computed from access timestamps

**Periodic (daily batch, configurable):**
1. Compute `B_i` for all nodes. Flag nodes where `B_i < threshold` as stale candidates.
2. Check stale candidates against structural importance (EWC weight from `04-graph`). High-EWC nodes are exempt.
3. Check stale candidates for recent corroboration (any `CONFIRMS` edge created in last 30 days). Corroborated nodes are exempt.
4. Remaining candidates: check if invalidated (bi-temporal `invalid_at IS NOT NULL`). Invalidated + stale → archive.
5. Run BMR retain_score for candidates with `B_i` in bottom 10%. Archive where `Retain_score < ε`.
6. Regenerate community summaries for any community that lost > 20% of its nodes.

**Capacity-triggered (when node count exceeds budget):**
- Apply three-tier eviction priority (above) until under budget
- Budget default: 100K active nodes. Configurable per deployment.
- Archived nodes are soft-deleted (remain queryable with `include_archived: true` flag) but excluded from default search, community detection, and summary generation.

**Metrics to monitor:**
- `archive_rate`: nodes archived per day (alert if > 5% of total)
- `stale_ratio`: nodes with `B_i < threshold` / total nodes (target: < 20%)
- `graph_density`: edges / nodes (alert if declining, may indicate over-pruning)

## Open Questions

- [x] How do we handle confidence propagation through the graph? → Hybrid multi-stage pipeline: Dempster-Shafer → Subjective Logic → DF-QuAD → Temporal Decay → TrustRank
- [x] Contradiction detection → Automatic during online consolidation. Vector similarity flags candidates, LLM classifies. Review queue for ambiguous cases.
- [x] Trust network → Yes via TrustRank batch computation + Beta-binomial per-source tracking. Already specified.
- [x] Staleness threshold → Multi-signal: ACT-R base level `B_i = ln(Σ t_k^{-0.5})` combining access count, recency, spacing. Flag as stale when `B_i < threshold` AND no recent corroboration AND low structural importance. See Covalence: consolidation synthesis.
- [x] Opinion vs factual → Provenance-as-metadata. Tag claims as `factual`, `opinion`, or `contested` during extraction. Use Subjective Logic uncertainty for classification confidence. Track asserter + source type per claim. See: Stuttgart ISWC 2024 (eSPARQL), Subjective Knowledge Graphs research.
- [x] Partial corrections → Yes via `CONTENDS` edge (0.3× attack weight) rather than `CORRECTS` (full zeroing). Correcting source specifies the specific claim being corrected + replacement. AGM theory supports partial belief contraction.
- [x] BMR threshold ε → Starting point: `keep_score = w1*structural_importance + w2*actr_base_level + w3*schema_position + w4*accommodation_count - w5*contradiction_age`. Prune when `keep_score < 0.1`. Requires empirical tuning against actual graph. Use sampled approximation (last 1000 queries touching artifact).
- [x] How do we handle cycles in confidence propagation? → Damping + fixed-point iteration with convergence guard (see Convergence Guard section)
