# ADR-0020: Domain-Agnostic Core with Configurable Ontology

**Status:** Accepted
**Date:** 2026-03-20
**Deciders:** Chris Jacobs, Claude Opus

## Context

Covalence was built as a GraphRAG engine for code + research. The entity types (function, struct, concept), relationship types (CALLS, USES_TYPE, IMPLEMENTS_INTENT), domain classifications (code, spec, design, research), and analysis bridges are hardcoded throughout the codebase.

This prevents Covalence from being applied to other domains — mathematics (theorems, proofs, lemmas), legal (statutes, precedents, interpretations), medical (conditions, treatments, evidence), or any other knowledge domain — without modifying the engine source code.

The core insight: Covalence's value is in the **pipeline** (ingest → extract → resolve → connect → search) and the **epistemic model** (confidence, provenance, temporal validity). These are domain-agnostic. The domain-specific parts should be configurable.

## Decision

Separate the engine into a domain-agnostic core and domain-configurable layers.

### Core (domain-agnostic, stays in Rust)

These are primitives about **how knowledge works**, not what it looks like:

| Capability | Why core |
|-----------|----------|
| Epistemic model (SL, DS, DF-QuAD, BMR) | Uncertainty math is universal |
| Orthogonal graph views | Perspective slicing is universal; which edges belong to which view is configurable |
| Pipeline mechanics (stages, fan-out, retry) | The flow is universal; what happens in each stage is pluggable |
| Entity resolution (5-tier cascade) | Dedup works for any entity type |
| Search fusion (RRF, SkewRoute, dimensions) | Retrieval math is domain-agnostic |
| Provenance (source tracking, offset projection) | Every fact needs a source |
| Temporal validity (valid_from, supersession, invalidation) | Knowledge changes over time |
| Community detection | Graph topology is domain-agnostic |
| Embedding management (Matryoshka, truncation) | Vector math |
| Consolidation (3-timescale) | The cadence is universal |
| Infrastructure (queue, config, cache, API) | Plumbing |

### Configurable (domain-specific, stored in DB)

These are about **what knowledge looks like** in a given domain:

| Capability | Storage | Current hardcoding |
|-----------|---------|-------------------|
| Entity types | `ontology_entity_types` table | Hardcoded in noise_filter, alignment, bootstrap |
| Relationship types | `ontology_rel_types` table | Hardcoded in search/graph dimensions, analysis |
| Entity classes | `ontology_classes` table | Hardcoded as code/domain/actor/analysis |
| Domain classification | `ontology_domains` table | Hardcoded as code/spec/design/research/external |
| View → edge type mappings | `ontology_view_edges` table | Hardcoded in CAUSAL_REL_TYPES, ENTITY_REL_TYPES |
| Analysis bridge types | `ontology_bridges` table | Hardcoded as PART_OF_COMPONENT, IMPLEMENTS_INTENT |
| Noise filter patterns | `ontology_noise_patterns` table | Hardcoded in noise_filter.rs (57 tests) |
| Extraction prompts | `engine/prompts/` + DB | Partially file-based, partially hardcoded |
| Component definitions | `ontology_components` table | Hardcoded 9 components in analysis/constants.rs |
| Normalization profiles | source_profile registry | Hardcoded ArXiv, code, etc. |

### Parsers as Sidecars

Content parsing moves to an extraction sidecar contract:

```
POST /extract
Content-Type: application/json
{
  "content": "...",
  "content_type": "rust",  // or "latex", "lean4", "prose"
  "ontology": { ... }      // what entity/rel types to look for
}

Response:
{
  "entities": [{"name": "...", "type": "...", "confidence": ...}],
  "relationships": [{"source": "...", "target": "...", "type": "..."}]
}
```

AST parsing (tree-sitter) becomes a sidecar rather than embedded. New parsers (LaTeX, Lean4, legal document structure) can be added without recompiling the engine.

## Consequences

### Positive
- Covalence becomes applicable to any knowledge domain
- New domains don't require Rust changes
- Community can contribute domain configs without touching core
- The Covalence-for-code use case becomes one config among many

### Negative
- Indirection: configurable ontology is harder to debug than hardcoded
- Performance: DB lookups for entity types vs compile-time constants
- Migration: existing hardcoded logic needs systematic replacement
- Testing: need to test with multiple ontology configs, not just one

### Risks
- Over-generalization: making everything configurable can make nothing work well
- Config complexity: users need to understand ontology design to use Covalence
- Default experience: out-of-the-box must still work for the common case (code + research)

## Migration Path

### Phase 1: Ontology Tables
Create `ontology_*` tables. Seed with current hardcoded values (code + research ontology). Existing behavior preserved.

### Phase 2: Replace Hardcoded References
Systematically replace every hardcoded entity type, relationship type, and domain reference with ontology lookups. The current behavior becomes the "default" ontology.

### Phase 3: AST Sidecar
Extract tree-sitter parsing into a sidecar. Define the extraction contract. Current embedded parsing becomes the default sidecar.

### Phase 4: Multi-Project
Support multiple ontologies per project. A math project uses math entity types; a code project uses code entity types. Both share the same Covalence instance.

## Alternatives Considered

1. **Fork per domain** — create covalence-math, covalence-legal. Rejected: code duplication, divergent evolution.
2. **Plugin system with Rust traits** — domain logic as trait implementations. Rejected: requires recompilation, not community-friendly.
3. **Keep hardcoded, document conventions** — let users modify source. Rejected: doesn't scale, error-prone.
