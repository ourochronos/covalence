# ADR-0022: Knowledge Primitives

**Status:** Proposed
**Date:** 2026-03-20
**Deciders:** Chris Jacobs, Claude Opus

## Context

Covalence needs a small set of fundamental primitives that are:
- **Irreducible** — can't be expressed in terms of other primitives
- **Necessary** — the system can't function without them
- **Sufficient** — together, they can express any knowledge pattern
- **Domain-agnostic** — work for code, math, law, medicine, or any domain

These primitives form the foundation that everything else builds on. Domain-specific concepts (functions, theorems, statutes) are compositions of these primitives, not primitives themselves.

## Decision

Seven knowledge primitives, plus two operational concepts.

### The Seven Primitives

#### 1. Entity

A named, embeddable thing in the knowledge graph.

An Entity has identity (canonical name, aliases), classification (via Schema), and can be the subject of Observations, Opinions, and Relationships. Entities are resolved across sources — the same real-world thing mentioned in different places converges to a single Entity through resolution.

Entities don't have hardcoded types. What an Entity "is" comes from Schema.

#### 2. Relationship

A typed, directed, weighted connection between two Entities.

Relationships carry confidence, temporal validity (valid_from, valid_until), and can be invalidated. They're the edges of the knowledge graph. Like Entities, their types come from Schema, not from hardcoded constants.

Relationships can be first-class subjects of Opinions — "we believe this connection exists with confidence X."

#### 3. Source

The origin of content. Where knowledge came from before it was processed.

A Source has: raw content, normalized content, a URI, temporal metadata (when created, when ingested), trust scores, and a supersession chain (version history). Sources are the ground truth — everything derived traces back to a Source.

Sources are not knowledge themselves. They're the container from which Observations extract knowledge.

#### 4. Observation

The act of extracting knowledge from a Source. The bridge between raw content and structured knowledge.

An Observation records:
- **What** was observed (an Entity, a Relationship, or a Claim)
- **Where** it was observed (Source, byte offsets)
- **How** it was observed (method, model, prompt version, parameters)
- **When** it was observed (timestamp)
- **How confidently** (extraction confidence)

This is the concept that Covalence currently splits across three tables (extractions, statements, processing JSONB) and should unify. Observation subsumes:

| Current concept | As Observation |
|----------------|---------------|
| Statement | Observation where output is a claim |
| Entity extraction | Observation where output is an Entity |
| Relationship extraction | Observation where output is a Relationship |
| ExtractionRun | A batch of Observations sharing method/model/time |
| `processing` JSONB | Observation metadata (model, duration, prompt_version) |

The same claim can be Observed multiple times from different Sources. Each Observation is independent evidence. Confidence accumulates across Observations via the epistemic model.

#### 5. Opinion

Epistemic state on any knowledge item. A Subjective Logic tuple (belief, disbelief, uncertainty, base rate).

Opinions attach to Entities, Relationships, and Observations. They represent not what we know, but **how confidently we know it**. An Opinion of (0, 0, 1, 0.5) means "we have no evidence either way" — which is fundamentally different from (0.5, 0.5, 0, 0.5) meaning "evidence is split."

Opinions are the primitive that makes Covalence epistemic rather than just a graph database.

#### 6. View

An orthogonal lens on the knowledge graph. Different Views reveal different truths about the same graph.

Based on MAGMA's argument for orthogonal graph views, extended with Observation and Schema views:

| View | What it reveals | Example query |
|------|----------------|--------------|
| **Semantic** | Meaning, conceptual relationships | "What concepts relate to X?" |
| **Temporal** | Time, evolution, validity periods | "What was true about X in 2024?" |
| **Causal** | Causation, intervention effects | "What happens if we change X?" |
| **Entity** | Classification, taxonomy, identity | "What kind of thing is X?" |
| **Observation** | How knowledge was produced | "Which model extracted X? When? What prompt?" |
| **Schema** | Meta-knowledge, ontology structure | "What entity types exist? What relationships are valid?" |

Views are not filters. They're perspectives. The Temporal View doesn't just filter by date — it reveals the evolution of knowledge over time, including what was believed then superseded.

The Observation View is critical for auditability: "show me everything Claude Haiku extracted" or "compare extractions from prompt v2 vs v3."

#### 7. Schema

Meta-knowledge about knowledge. What types of Entities exist, what Relationships are valid, what constraints apply.

Schema is itself knowledge — queryable, versionable, and domain-configurable. A Schema defines:

- **Entity types** and their categories (concept, process, artifact, agent, property, collection)
- **Relationship types** and their semantics (is_a, part_of, depends_on, derived_from, supports, contradicts, precedes, uses)
- **View → edge type mappings** (which relationships appear in which View)
- **Validation constraints** (e.g., "a proof must derive_from at least one axiom")
- **Noise patterns** (what to filter during extraction)
- **Extraction prompts** (what to look for in content)

Different projects use different Schemas. A code project defines `function` as a `process` entity type. A math project defines `theorem` as a `concept`. The core engine doesn't know or care — it operates on the universal categories.

### Universal Categories (Schema Defaults)

Every Schema starts with these six universal entity categories:

| Category | What it captures |
|----------|-----------------|
| **concept** | An abstract idea, definition, or named thing |
| **process** | Something that transforms, computes, or produces |
| **artifact** | A concrete, addressable, citable thing |
| **agent** | Something that acts, decides, or creates |
| **property** | A measurable attribute or quality |
| **collection** | A grouping, containment, or namespace |

And eight universal relationship types:

| Type | What it captures |
|------|-----------------|
| **is_a** | Classification, taxonomy |
| **part_of** | Composition, containment |
| **depends_on** | Dependency, prerequisite |
| **derived_from** | Derivation, production |
| **supports** | Epistemic agreement, evidence |
| **contradicts** | Epistemic disagreement, counter-evidence |
| **precedes** | Temporal ordering |
| **uses** | Reference, invocation |

Domain-specific types map to these. `CALLS` → `uses`. `proves` → `derived_from`. `IMPLEMENTS_INTENT` → `derived_from`. The core reasons about universals; the UI shows domain types.

### Operational Concepts (Derived)

Two concepts that are operationally necessary but ontologically derived:

**Chunk** — a retrieval-sized span of a Source with an embedding. Exists because search needs granularity. Derived from Source (it's a span) + Embedding (it's a vector). Not a knowledge primitive.

**Embedding** — a vector representation of any knowledge item. Exists because search needs similarity. It's a property of Entities, Chunks, and Sources — not a thing in itself.

### Sidecar Contract

Sidecars are **Observation factories**. They take content and Schema, produce Observations:

```
# STDIO contract (stateless transforms)
stdin:  {"content": "...", "schema": {...}, "config": {...}}
stdout: {"observations": [
  {"type": "entity", "name": "...", "category": "concept", "confidence": 0.95},
  {"type": "relationship", "source": "...", "target": "...", "rel_type": "derived_from", "confidence": 0.87},
  {"type": "claim", "content": "...", "confidence": 0.91}
]}
```

Stateless transforms use STDIO (pipes). Stateful services (models that need loading) use HTTP. Both produce the same output: Observations.

## Consequences

### Positive
- Seven primitives are sufficient to model any knowledge domain
- Observation unifies three current tables into one coherent concept
- Schema-as-knowledge enables querying and versioning the ontology
- Six Views provide comprehensive orthogonal perspectives
- Universal categories enable cross-domain reasoning without domain knowledge
- STDIO sidecar contract enables language-agnostic observation factories

### Negative
- Unifying Statement + Extraction + Processing into Observation requires data migration
- Schema indirection adds a lookup step to every type check
- Six Views may be more than most domains need
- Universal categories may not perfectly fit every domain (some entities resist classification)

### Open Questions

1. Should Chunk be promoted to a full primitive? It's operationally central but ontologically derived.
2. Are six universal categories the right number? Is `property` necessary, or can it be a `concept` with attributes?
3. Should Views be extensible (user-defined views) or fixed at six?
4. How does the Observation primitive interact with the existing `extractions` table migration path?
5. What's the right granularity for Schema versioning? Per-change or per-release?

## References

- MAGMA: Multi-Graph based Agentic Memory Architecture (2601.03236) — orthogonal views
- ADR-0020: Domain-Agnostic Core with Configurable Ontology
- ADR-0021: Simplify Pipeline to Statements and Chunks
- ADR-0005: Subjective Logic for Confidence Representation
- SUMO, DOLCE, BFO — upper ontology prior art (simpler than any of these)
