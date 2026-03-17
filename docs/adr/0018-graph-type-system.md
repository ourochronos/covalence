# ADR-0018: Graph Type System — Labels, Layers, and Enforcement

**Status:** Accepted

**Date:** 2026-03-16

**Spec Reference:** spec/02-data-model.md, spec/12-code-ingestion.md, spec/13-cross-domain-analysis.md

## Context

Covalence's knowledge graph has 6,800+ nodes across 47 ad-hoc `node_type` values and 113,000+ edges with 100+ free-form `rel_type` values, but no structural hierarchy, domain boundaries, or edge validation.

Problems this causes:

1. **Type soup**: `concept` (3,699 nodes) is a catch-all. A concept from a spec, a research paper, and an LLM hallucination are all the same type.

2. **No domain boundaries**: Code entities, design concepts, and research findings all live in one flat namespace. Cross-domain analysis (Wave 9) had to bolt on `PART_OF_COMPONENT`, `IMPLEMENTS_INTENT`, and `THEORETICAL_BASIS` edges to bridge what should be intrinsic structure.

3. **Domain derived at query time**: `source_layer_from_uri()` in the search service infers domain from URI patterns every time. This is fragile, duplicated across services, and not indexable.

4. **No multi-project support**: Everything lives in one graph. Adding a second codebase would conflate its entities with Covalence's.

5. **Synthesis confusion**: The `/ask` endpoint can't distinguish Covalence's own entity types from research paper concepts because there's no domain label.

## Decision

Add three classification fields and a set of traceability edge types:

### Source Labels
- `project TEXT NOT NULL DEFAULT 'covalence'` — project namespace for multi-project support
- `domain TEXT` — one of: `code`, `spec`, `design`, `research`, `external`

Domain is derived deterministically from source URI patterns at ingestion time via `derive_domain()`.

### Entity Classification
- `entity_class TEXT` on nodes — one of: `code`, `domain`, `actor`, `analysis`

Derived deterministically from `node_type` at entity creation via `derive_entity_class()`. Stored (not computed at query time) for indexing and filtering performance.

### Traceability Edges
Precise provenance chains between knowledge domains:
- `SPECIFIES` — spec concept → design decision
- `DECIDES` — design decision → code entity
- `INFORMS` — research → spec concept
- `VALIDATES` — research → code behavior

These coexist with existing Component bridge edges (IMPLEMENTS_INTENT, PART_OF_COMPONENT, THEORETICAL_BASIS). Bridge edges are bulk-automated via embedding similarity. Traceability edges are precise curated links.

### Edge Validation
Soft enforcement: warn on invalid entity_class pairs, don't reject. This avoids data loss during transition and accommodates legitimate edge cases.

## Consequences

### Positive

- **Intrinsic domain structure**: Sources and entities carry their domain classification as first-class fields, eliminating URI heuristics at query time.
- **Filterable search**: `entity_class` and `domain` enable SQL-level filtering in search, analysis, and synthesis without joins.
- **Multi-project ready**: The `project` field on sources enables ingesting multiple codebases without entity conflation.
- **Precise traceability**: SPECIFIES/DECIDES/INFORMS/VALIDATES edges enable "why does this code exist?" traversals from code through design through spec to research.
- **Graph sidecar enrichment**: `entity_class` in `NodeMeta` enables entity-class-aware graph algorithms.
- **Backward compatible**: All new fields have defaults or are nullable. Existing queries continue to work.

### Negative

- **Denormalization**: `entity_class` duplicates information derivable from `node_type`. The mapping must be maintained consistently between `derive_entity_class()` and the backfill SQL.
- **Backfill risk**: Retroactively classifying 419 sources and 6,800 nodes requires correct URI pattern matching. Misclassification affects downstream analysis.
- **Vocabulary maintenance**: The 5-domain and 4-class vocabularies must evolve as new source types appear. Adding a domain requires updating `derive_domain()`, the migration backfill, and the spec.

## Alternatives Considered

1. **Derive entity_class at query time**: Normalized but requires the mapping function at every query site. Rejected because it can't be indexed and adds latency to every search.

2. **Use AGE labels instead of property values**: Apache AGE supports native graph labels (`:Code`, `:Domain`). Rejected because we support both petgraph and AGE backends — property values work consistently across both.

3. **Merge entity_class into canonical_type**: The ontology clustering system already produces `canonical_type`. Rejected because `entity_class` and `canonical_type` are orthogonal — one classifies the *kind* of entity, the other normalizes the *type label*.

4. **Store domain on nodes instead of sources**: Would enable per-entity domain classification for merged entities. Rejected because domain is a property of provenance (where the information came from), not of the entity itself. A "PostgreSQL" entity mentioned in both code and research should trace back to both domains via its extraction provenance.

5. **Hard edge validation (reject invalid pairs)**: Rejected because it would cause data loss during the transition period and blocks legitimate edge cases (e.g., a research paper naming a specific code function).
