# ADR-0010: Pearl's Causal Hierarchy for Edge Semantics

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/07-epistemic-model.md

## Context

Not all relationships are equal. "X correlates with Y" is weaker than "doing X causes Y" which is weaker than "had X not happened, Y would not have." The system needs to distinguish these levels of causal reasoning.

## Decision

Classify edges into three levels per Pearl's Causal Hierarchy:

- **L0 (Association):** Correlational. RELATED_TO, PART_OF, INSTANCE_OF, HAS_PROPERTY, MENTIONED_IN.
- **L1 (Intervention):** Causal/evidential. CAUSED_BY, ENABLED, ORIGINATES, CONFIRMS, CONTRADICTS, CONTENDS, SUPERSEDES, CORRECTS, APPENDED_AFTER.
- **L2 (Counterfactual):** Hypothetical. CAUSAL with explicit Pearl-level annotation.

L1+ edges carry additional metadata: causal_strength, direction_confidence, evidence_type, hidden_conf_risk.

## Consequences

### Positive

- Enables causal reasoning, not just correlation
- L1 edges support robust confidence propagation
- Edge type vocabulary is explicit and well-defined
- Search can filter by causal level (e.g., "only show causal relationships")

### Negative

- LLM extraction must classify causal level (error-prone)
- Most extracted edges will be L0 initially
- L2 (counterfactual) edges are rare and hard to validate

## Alternatives Considered

- **Untyped edges with weight only:** Loses all causal semantics
- **Binary causal/non-causal:** Too coarse, misses the intervention vs counterfactual distinction
