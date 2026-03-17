# Graph Type System: Labels, Layers, and Enforcement

## Status: DRAFT — Pending adversarial review

## Problem Statement

Covalence's knowledge graph has 6,800+ nodes across 47 node types and 113,000+ edges across 100+ relationship types, but **no structural hierarchy or domain separation**. This creates several problems:

1. **Type soup**: `concept` (3,699 nodes) is a catch-all. A "concept" from a spec, a research paper, and LLM hallucination are all the same type. Entity extraction produces whatever type the LLM decides — there's no controlled vocabulary.

2. **No domain boundaries**: Code entities (`function`, `struct`), design concepts (from specs), and research findings (from papers) all live in one flat namespace. Cross-domain analysis (Wave 9) had to bolt on `PART_OF_COMPONENT`, `IMPLEMENTS_INTENT`, and `THEORETICAL_BASIS` edges to bridge what should be intrinsic structure.

3. **Missing metadata enforcement**: Sources have `source_type` (code/document) but no domain, project, or layer classification. Nodes have `node_type` but no domain provenance. Edges have `rel_type` but no hierarchy constraint (an article could theoretically point to a statement).

4. **No multi-project support**: Everything is in one graph. Adding a second codebase would conflate its entities with Covalence's. There's no namespace isolation.

5. **Synthesis disconnection**: Articles (RAPTOR compilations) reference `source_node_ids` but have no structural relationship to statements or sections. Sections reference `statement_ids` but aren't linked to the entities those statements mention.

## Current State

### Data Model (what exists in PG)

```
sources (419)
  ├── source_type: code | document
  ├── uri: file://engine/... | https://arxiv/... | file://spec/...
  └── No: domain, project, layer fields

chunks (25,100) → belongs_to source
statements (34,200) → belongs_to source, has heading_path
sections (12,300) → belongs_to source, has statement_ids[]

nodes (6,800) → 47 distinct node_types
  ├── Code types: function(300), struct(106), impl_block(33), enum(9), trait(5), constant(5), module(1)
  ├── Domain types: concept(3699), technology(1253), person(369), organization(247)
  ├── Analysis types: component(9) — added in Wave 9
  └── Other: event(127), location(90), dataset(55), algorithm(11), model(3), ...
  └── No: domain, layer, project fields

edges (113,000) → 100+ rel_types
  ├── Synthetic: co_occurs (68,000 — 60% of all edges)
  ├── Bridge: THEORETICAL_BASIS(1868), IMPLEMENTS_INTENT(1440), PART_OF_COMPONENT(537)
  ├── Semantic: uses, implements, includes, contains, applies_to, ...
  └── 19,846 invalidated (17.6% — invisible to graph, see #135)

extractions → links nodes to chunks/statements with provenance

articles (823) → RAPTOR community summaries, reference source_node_ids[]
```

### Source Domain Distribution

| Domain | Source Type | Count | How Identified |
|--------|-----------|-------|----------------|
| Code: engine/ | code | 193 | URI heuristic |
| Code: cli/ | code | 15 | URI heuristic |
| Code: dashboard/ | code | 2 | URI heuristic |
| Spec | document | 14 | URI `file://spec/` |
| ADR | document | 14 | URI `file://docs/adr/` |
| Design docs | document | ~10 | URI `file://` non-spec/adr |
| Research: arxiv | document | 136 | URI `https://arxiv` |
| Research: doi | document | 3 | URI `https://doi` |
| Research: web | document | 9 | URI `https://` |
| Other | document/code | 27 | Mixed |

**Key insight**: Domain is currently inferred from URI patterns at query time (e.g., in cross-domain analysis). It should be a first-class field set at ingestion time.

## Proposed: Layered Label System

### Layer Model

```
Layer 0: SOURCES — raw material
  Labels: project, domain, source_type

Layer 1: EXTRACTIONS — derived content
  Labels: inherits project+domain from source
  Types: chunk, statement, section

Layer 2: ENTITIES — resolved knowledge
  Labels: entity_class (code|domain|actor|analysis), node_type
  Inherits project from extraction provenance

Layer 3: SYNTHESIS — compiled knowledge
  Labels: synthesis_type (article|community_summary)
  Cross-domain by nature (synthesizes across layers)
```

### Source Labels

Add three fields to sources:

```sql
ALTER TABLE sources ADD COLUMN project TEXT NOT NULL DEFAULT 'covalence';
ALTER TABLE sources ADD COLUMN domain TEXT;  -- code, spec, design, research, external
ALTER TABLE sources ADD COLUMN layer TEXT NOT NULL DEFAULT 'source';
```

**Domain values** (controlled vocabulary):
- `code` — source code files (.rs, .go, .js, etc.)
- `spec` — specification documents (spec/*.md)
- `design` — architecture decisions, design docs (docs/adr/*.md, VISION.md, design/*.md)
- `research` — academic papers, external knowledge
- `external` — third-party documentation, API references

**Domain assignment rules** (at ingestion time, not query time):
```
URI matches file://engine/ OR file://cli/ OR file://dashboard/ → domain = code
URI matches file://spec/ → domain = spec
URI matches file://docs/adr/ OR file://VISION.md OR file://design/ → domain = design
URI matches https://arxiv OR https://doi → domain = research
URI matches file://CLAUDE.md OR file://MILESTONES.md → domain = design
Otherwise document → domain = research (default for ingested papers)
Otherwise code → domain = code
```

### Entity Labels (Node Classification)

Replace the 47 ad-hoc `node_type` values with a two-level classification:

**Level 1: Entity Class** (4 values — controlled, enforced):
- `code` — entities extracted from source code (function, struct, trait, enum, impl_block, constant, module, class, macro)
- `domain` — domain concepts from any textual source (concept, technology, algorithm, technique, framework, method, metric, dataset, benchmark, model)
- `actor` — people, organizations, locations (person, organization, location, role)
- `analysis` — system-generated entities (component, community_summary)

**Level 2: Node Type** (existing field — kept, but normalized):
- Code class: function, struct, trait, enum, impl_block, constant, module, class, macro
- Domain class: concept, technology, algorithm, dataset, metric, model, framework
- Actor class: person, organization, location
- Analysis class: component

```sql
ALTER TABLE nodes ADD COLUMN entity_class TEXT;
-- Backfill from existing node_type
UPDATE nodes SET entity_class = CASE
  WHEN node_type IN ('function','struct','trait','enum','impl_block','constant','module','class','macro') THEN 'code'
  WHEN node_type IN ('person','organization','location','role') THEN 'actor'
  WHEN node_type = 'component' THEN 'analysis'
  ELSE 'domain'
END;
```

### Edge Hierarchy Constraints

Edges should respect layer relationships. Not all edge types make sense between all entity classes:

**Within-layer edges** (entity↔entity):
- `co_occurs` — any entity pair within the same chunk/statement
- `uses`, `implements`, `includes`, `is_a`, `is_part_of` — semantic relationships
- `contains`, `enables`, `applies_to` — domain relationships

**Cross-layer bridge edges** (controlled):
- `PART_OF_COMPONENT` — code entity → analysis component
- `IMPLEMENTS_INTENT` — analysis component → domain concept (from specs)
- `THEORETICAL_BASIS` — analysis component → domain concept (from research)

**Edges that should NOT exist** (enforcement):
- article → statement (articles synthesize from nodes, not statements)
- code entity → actor (a function doesn't "know" a person — the paper that describes the function mentions the person)
- statement → chunk (statements are derived from chunks, not related to them)

### Multi-Project Namespacing

The `project` field on sources propagates through extractions to entities:

```
source(project=covalence, domain=code)
  → chunk → extraction → node (inherits project=covalence)

source(project=valence, domain=code)
  → chunk → extraction → node (inherits project=valence)
```

**Cross-project entities**: Some entities (e.g., "PostgreSQL", "Rust") are genuinely cross-project. These get `project = NULL` (global) through entity resolution — when two projects both have a "PostgreSQL" entity, resolution merges them into a global entity.

**Query scoping**:
```sql
-- Covalence-only search
WHERE n.project = 'covalence' OR n.project IS NULL

-- Cross-project search
WHERE n.project IN ('covalence', 'valence') OR n.project IS NULL
```

## Enforcement Strategy

### At Ingestion Time

1. **Source domain assignment**: When a source is created (`SourceService::ingest`), compute `domain` from URI pattern. Reject sources without a classifiable domain (require explicit override for edge cases).

2. **Entity class assignment**: When the LLM extractor produces entities, the `node_type` is validated against the controlled vocabulary. Unknown types are mapped to the closest valid type (e.g., `process` → `concept`, `activity` → `concept`). The `entity_class` is derived deterministically from `node_type`.

3. **Edge validation**: When edges are created (in the pipeline or via entity resolution), validate that the source→target entity classes are compatible. Log warnings for violations but don't reject (to avoid data loss during transition).

### Retroactive Backfill

1. Backfill `domain` on all 419 existing sources using the URI pattern rules above.
2. Backfill `entity_class` on all 6,800 nodes using the node_type mapping.
3. Normalize `node_type` values: merge the long tail (47 → ~20 canonical types).
4. Audit edge type consistency across entity classes.

### Noise Filter Integration

The existing noise filter (`services/noise_filter.rs`) catches extraction noise. The entity class system complements it — if the LLM says something is a `struct` but it came from a research paper (domain=research), that's a signal it might be noise (a struct name mentioned in a paper, not an actual code entity).

## Migration Plan

**Phase 1: Schema** (additive, no behavior change)
- Add `project`, `domain` columns to sources
- Add `entity_class` column to nodes
- Backfill existing data
- Migration: idempotent, safe to re-run

**Phase 2: Ingestion enforcement**
- Compute domain at source creation
- Derive entity_class at entity extraction
- Normalize node_type vocabulary
- Log warnings for edge constraint violations

**Phase 3: Query integration**
- Cross-domain analysis uses intrinsic `domain` instead of URI heuristics
- Search filters by entity_class and domain
- Dashboard shows domain breakdown
- `/ask` includes domain provenance in context blocks

**Phase 4: Multi-project**
- Add project scoping to search, analysis, and ask endpoints
- Entity resolution considers project when matching
- CLI supports `--project` flag

## Open Questions (for adversarial review)

1. **Should `entity_class` be on the node or derived from `node_type` at query time?** Storing it is denormalized but faster. Deriving it is normalized but requires the mapping at every query.

2. **How should entity resolution handle cross-domain merges?** If a spec mentions "PgResolver" (concept) and the code has "PgResolver" (struct), should they merge? Currently they do. Should they stay separate with a cross-domain link instead?

3. **Should edges have a `layer` field?** Or is the source/target entity_class sufficient to infer the layer relationship?

4. **How do we handle the 3,699 `concept` nodes?** Many are legitimate domain concepts, but many are noise (generic words that passed the noise filter). Should we sub-classify concepts (e.g., `concept:retrieval`, `concept:epistemic`)?

5. **What about `other` (305 nodes)?** This is a catch-all from LLM extraction. Should it be eliminated entirely, or kept as a staging area for manual classification?

6. **Temporal dimension**: Should domain/project be immutable on a source, or can a source's domain change (e.g., a design doc becomes a spec)?

7. **AGE integration**: With Apache AGE, labels could be native graph labels (`:Code`, `:Domain`, `:Actor`) rather than property values. Should we design for this from the start?

## References

- spec/02-data-model.md — Current entity model
- spec/12-code-ingestion.md — Code entity types, Component model
- spec/13-cross-domain-analysis.md — Cross-domain bridging (the bolted-on solution)
- #135 — Epistemic integration of invalidated edges
- #137 — GraphEngine trait + AGE migration (CLOSED — completed)
- services/analysis/constants.rs — MODULE_PATH_MAPPINGS (current domain inference)
- services/noise_filter.rs — Entity noise detection
