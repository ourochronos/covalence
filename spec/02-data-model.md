# 02 — Data Model

**Status:** Implemented

## Design Decision: Hybrid Property Graph + Provenance View

**Decision: Option C (Hybrid).**

Property graph is the primary model — nodes and edges with JSONB properties. This is the native model of petgraph and maps cleanly to PostgreSQL. For fine-grained provenance and epistemic operations, a `provenance_triples` SQL view decomposes the property graph into (subject, predicate, object) triples on-the-fly (see [03-storage](03-storage.md#provenance-triples-view)).

**Rationale:**
- Property graph is natural for petgraph, has no join overhead for properties, and is familiar to most developers
- Triple decomposition via SQL view is zero-maintenance (no sync needed) and sufficient for provenance queries and epistemic model operations
- The view can be promoted to a `MATERIALIZED VIEW` with periodic refresh if performance warrants it

**Tradeoffs considered:**
- Pure triples (valence-v2 approach): maximally composable but verbose (a node with 5 properties = 5+ triples) and join-heavy
- Pure property graph: loses fine-grained per-fact provenance
- Hybrid gives both without maintaining a separate triple store

---

## Core Entities

Regardless of the graph model chosen, the system has these core entity types:

### Source

A provenance record for any ingested material. Each source carries a trust prior modeled as a Beta distribution, updated as evidence accumulates.

```
Source {
  id: UUID,
  source_type: Enum,       -- "document", "web_page", "conversation", "api", "manual", "code"
  uri: String?,             -- original location (URL, file path, etc.)
  title: String?,
  author: String?,
  project: Text,            -- project namespace (default 'covalence'), NULL = global
  domain: Text?,            -- 'code' | 'spec' | 'design' | 'research' | 'external' (set at ingest, see Domain Classification)
  created_date: Timestamp?, -- when the source was originally created/published
  ingested_at: Timestamp,   -- when we ingested it
  content_hash: Bytes,      -- SHA-256 hash of raw content for dedup
  metadata: JSONB,          -- format-specific metadata (page count, mime type, etc.)
  raw_content: Text?,       -- optional: store the original text
  trust_alpha: Float,       -- Beta distribution α parameter (confirmations)
  trust_beta: Float,        -- Beta distribution β parameter (contradictions)
  reliability_score: Float, -- Beta(α,β).mean() = α/(α+β), cached for query use
  clearance_level: Int,     -- 0=local_strict, 1=federated_trusted, 2=federated_public
  update_class: Enum?,      -- "append_only", "versioned", "correction", "refactor" (null for first ingest)
  supersedes_id: UUID?,     -- FK → Source: forward pointer (this source supersedes that one)
  superseded_by: UUID?,     -- FK → Source: backward pointer (this source was superseded by that one)
  superseded_at: Timestamp?, -- when this source was marked as superseded
  content_version: Int,     -- version counter, increments on update
  processing: JSONB,        -- latest pipeline processing state per stage (default '{}')
}
```

**Supersession model:** Sources form version chains via two directional pointers. When source B replaces source A: A's `superseded_by` is set to B's id and `superseded_at` is timestamped (backward pointer), while B's `supersedes_id` is set to A's id (forward pointer). This enables both "what replaced this?" and "what did this replace?" queries. Non-superseded sources (`superseded_by IS NULL`) are the active set. Superseded sources are preserved for temporal queries and audit trail. See migration 017.

### Chunk

A text segment at a specific granularity level, linked to its parent and source. All source formats are normalized to extended Markdown before chunking (see [05-ingestion](05-ingestion.md#stage-3-normalize-to-markdown)).

```
Chunk {
  id: UUID,
  source_id: UUID,            -- FK → Source
  parent_chunk_id: UUID?,      -- FK → Chunk (null for document-level)
  level: Enum,                -- "document", "section", "paragraph", "sentence"
  ordinal: Int,               -- position within parent
  content: Text,              -- Markdown-normalized text
  content_hash: Bytes,        -- SHA-256 of content (for structural refactoring detection)
  embedding: Vector,          -- pgvector halfvec
  embedding_model: String,    -- e.g. "voyage-context-3" — tracks which model produced this embedding
  contextual_prefix: Text?,   -- LLM-generated context summary prepended before embedding
  token_count: Int,
  structural_hierarchy: Text, -- "Title > Chapter 2 > Section 2.1"
  clearance_level: Int,       -- inherited from source, overridable per chunk
  metadata: JSONB,            -- heading text, page number, speaker, contains_table, etc.
  -- Landscape analysis (populated during ingestion Stage 6)
  parent_alignment: Float?,   -- cosine(child.embedding, parent.embedding), null for document-level
  extraction_method: Enum,    -- "embedding_linkage", "delta_check", "full_extraction", "full_extraction_with_review"
  landscape_metrics: JSONB?,  -- adjacent_similarity, sibling_outlier_score, graph_novelty, flags, valley_prominence
  created_at: Timestamp,
}
```

The chunk hierarchy enables parent-child retrieval: search at sentence granularity, retrieve at paragraph or section granularity for context.

### Node (Graph Entity)

An entity extracted from one or more chunks.

```
Node {
  id: UUID,
  canonical_name: String,
  node_type: String,            -- dynamically assigned during extraction
  entity_class: Text?,          -- 'code' | 'domain' | 'actor' | 'analysis' (see Entity Classification below)
  description: Text?,
  properties: JSONB,
  embedding: Vector?,           -- derived from description or aggregated from chunks (pgvector halfvec)
  embedding_model: String?,     -- tracks which model produced the embedding (null if no embedding)
  domain_entropy: Float?,       -- Shannon entropy of source-domain distribution (low = single-domain, high = cross-cutting)
  primary_domain: Text?,        -- most-mentioned source domain ('code', 'spec', 'design', 'research', 'external')
  confidence_breakdown: JSONB?, -- Subjective Logic opinion tuple (b, d, u, a)
  clearance_level: Int,         -- most restrictive of all sources this node was extracted from
  processing: JSONB,            -- latest pipeline processing state per stage (default '{}')
  first_seen: Timestamp,
  last_seen: Timestamp,
  mention_count: Int,
}
```

`domain_entropy` and `primary_domain` are computed from extraction provenance: for each node, the system counts how many times it was extracted from sources of each domain, then computes Shannon entropy over those counts. A node mentioned only in code sources has `domain_entropy = 0.0` and `primary_domain = 'code'`. A node mentioned across research, spec, and code has high entropy. These fields are used by the DDSS (Domain-Dominant Self-referential Score) search routing to detect self-referential queries and boost internal-domain results. See migration 015.

### Edge (Graph Relationship)

A typed, directed relationship between two nodes. Edges carry rich temporal and causal metadata per Pearl's Causal Hierarchy and the epistemic model in [07-epistemic-model](07-epistemic-model.md).

```
Edge {
  id: UUID,
  source_node_id: UUID,     -- FK → Node
  target_node_id: UUID,     -- FK → Node
  rel_type: String,          -- see edge vocabulary in 07-epistemic-model
  causal_level: Enum,        -- "association", "intervention", "counterfactual"
  properties: JSONB,         -- additional causal metadata, context
  weight: Float,             -- default 1.0, adjustable (causal_weight for epistemic edges)
  confidence: Float,         -- extraction confidence (modified by epistemic propagation)
  confidence_breakdown: JSONB?, -- Subjective Logic opinion tuple + per-source contributions
  clearance_level: Int,      -- min(source_node_clearance, target_node_clearance)
  is_synthetic: Bool,        -- true for ZK edges generated during federation egress
  -- Bi-temporal model (Graphiti/Zep pattern):
  valid_from: Timestamp?,    -- when the fact became true in the world (assertion time, extracted from text)
  valid_until: Timestamp?,   -- when the fact stops being true (null if still current)
  recorded_at: Timestamp,    -- when the system learned of this edge (transaction time)
  invalid_at: Timestamp?,    -- when a contradicting edge invalidated this one (null if still valid)
  invalidated_by: UUID?,     -- FK → Edge that superseded this one
  created_at: Timestamp,
}
```

**Bi-temporal semantics:**
- `valid_from` / `valid_until` = **assertion time** — when the fact is true in the world. Extracted from source text by the LLM ("John joined Google in 2020"). Null if unknown.
- `recorded_at` = **transaction time** — when the system ingested this edge. Always set, immutable.
- `invalid_at` / `invalidated_by` = **invalidation** — when and by what this edge was superseded. Set during contention resolution. The original edge is preserved (not deleted) for temporal queries.

**Example lifecycle:**
1. Source A says "John works at Google" → Edge created: `valid_from=2020, invalid_at=null`
2. Source B says "John works at Apple" → Contention detected. Resolution: temporally invalidate Edge 1 → `invalid_at=now(), invalidated_by=edge2_id`. New Edge 2 created: `valid_from=2024, invalid_at=null`
3. Query "where does John work?" → returns Edge 2 (active). Query "where did John work in 2021?" → returns Edge 1 (valid_from ≤ 2021, valid_until null or > 2021).

**Edge properties schema** (stored in JSONB `properties`):

```json
{
  "causal_strength": 0.85,
  "direction_confidence": 0.92,
  "evidence_type": "llm_extracted",
  "hidden_conf_risk": "low"
}
```

Causal metadata fields are present only on L1+ edges (see [07-epistemic-model](07-epistemic-model.md#edge-type-vocabulary)).
```

### Extraction (Provenance Link)

Links graph elements back to the chunks they were extracted from.

```
Extraction {
  id: UUID,
  chunk_id: UUID,          -- FK → Chunk
  entity_type: Enum,       -- "node", "edge"
  entity_id: UUID,         -- FK → Node or Edge
  extraction_method: String, -- "llm_gpt4", "ner_spacy", "manual", ...
  confidence: Float,
  extracted_at: Timestamp,
}
```

### NodeAlias (Entity Resolution)

Tracks alternative names that resolve to the same canonical node.

```
NodeAlias {
  id: UUID,
  node_id: UUID,           -- FK → Node
  alias: String,           -- "Apple Inc.", "AAPL", "the iPhone maker"
  alias_embedding: Vector,
  source_chunk_id: UUID?,  -- where this alias was first seen
}
```

### Article (Compiled Summary)

A synthesized summary produced during batch consolidation. Articles are optimal retrieval units (200–4000 tokens) — right-sized for both embedding quality and LLM context windows.

```
Article {
  id: UUID,
  title: String,
  body: Text,               -- compiled Markdown content
  embedding: Vector,
  confidence: Float,         -- aggregate Bayesian confidence
  confidence_breakdown: JSONB, -- Subjective Logic opinion tuple
  domain_path: Text[],       -- topic hierarchy ["AI", "Knowledge Graphs", "Confidence"]
  version: Int,
  content_hash: Bytes,
  source_node_ids: UUID[],   -- nodes/chunks this article was compiled from
  clearance_level: Int,
  created_at: Timestamp,
  updated_at: Timestamp,
}
```

Articles are produced by the batch consolidation tier (see [01-architecture](01-architecture.md#layer-responsibilities)). They serve as the primary retrieval unit for search — empirically validated chunk size range for high faithfulness and relevancy.

### Statement (Atomic Knowledge Claim)

A self-contained factual claim extracted from a source via windowed LLM extraction (see [ADR-0015](../docs/adr/0015-statement-first-extraction.md)). Statements are the fundamental retrieval unit — each is independently meaningful with all pronouns resolved to explicit referents.

```
Statement {
  id: UUID,
  source_id: UUID,            -- FK → Source
  content: Text,              -- self-contained claim (no pronouns, no context dependencies)
  content_hash: Bytes,        -- SHA-256 of content for dedup across extraction windows
  embedding: Vector?,         -- pgvector halfvec (dimension matches chunk table)
  byte_start: Int,            -- byte offset in Source.normalized_content where supporting text starts
  byte_end: Int,              -- byte offset in Source.normalized_content where supporting text ends
  heading_path: Text?,        -- heading context at extraction location ("Chapter 2 > Methods")
  paragraph_index: Int?,      -- paragraph position within source section
  ordinal: Int,               -- position within the source's statement sequence
  confidence: Float,          -- extraction confidence from the LLM
  section_id: UUID?,          -- FK → Section (set after clustering)
  clearance_level: Int,       -- inherited from source
  is_evicted: Bool,           -- true if re-extraction determined this claim is no longer supported
  created_at: Timestamp,
}
```

### Section (Topic Cluster Summary)

A compiled summary of semantically similar statements within a single source. Sections represent local topics — they are clustered by semantic similarity (HAC on statement embeddings), not by document structure. The document's physical layout is preserved as metadata on the constituent statements, not as section boundaries.

```
Section {
  id: UUID,
  source_id: UUID,            -- FK → Source
  title: Text,                -- LLM-generated topic title
  body: Text,                 -- LLM-compiled narrative summary of the clustered statements
  embedding: Vector?,         -- pgvector halfvec
  ordinal: Int,               -- position within the source's section sequence
  clearance_level: Int,       -- inherited from source
  created_at: Timestamp,
}
```

### Component (Design Doc Bridge)

A logical grouping that bridges natural language intent (specs, design docs, research) to code implementation. Components are the translation layer between business logic expressed in prose and execution expressed in code.

```
Component {
  id: UUID,
  name: String,               -- e.g. "Ingestion Pipeline", "Statement Extractor", "RRF Fusion"
  description: Text?,         -- natural language summary of this component's purpose
  embedding: Vector?,         -- pgvector halfvec (embedded from description)
  source_id: UUID?,           -- FK → Source (the design doc or spec this component was extracted from)
  metadata: JSONB,            -- language, file patterns, module paths, etc.
  created_at: Timestamp,
  updated_at: Timestamp,
}
```

Components are connected to the graph via typed edges:
- `IMPLEMENTS_INTENT`: Component → Spec/Topic Node (upward, toward design intent)
- `PART_OF_COMPONENT`: Code Node → Component (upward, toward logical grouping)
- `THEORETICAL_BASIS`: Component → Research Node (lateral, toward academic foundation)

### ProcessingLog (Audit Trail)

An append-only audit trail recording every processing step applied to any data item. Enables cost tracking, prompt version management, and selective reprocessing when prompts evolve.

```
ProcessingLog {
  id: UUID,
  item_table: Text,           -- 'chunks', 'nodes', 'statements', 'sources'
  item_id: UUID,              -- FK to the item that was processed
  stage: Text,                -- 'extraction', 'summary', 'embedding', 'compose', etc.
  model: Text?,               -- 'claude-haiku-4.5', 'voyage-3-large', etc.
  duration_ms: Int?,          -- wall-clock time for this processing step
  status: Text,               -- 'success' | 'error' (default 'success')
  error_message: Text?,       -- error details if status = 'error'
  ingestion_id: UUID?,        -- groups all processing for one source reprocess run
  prompt_version: Int?,       -- tracks prompt evolution for selective reprocessing
  input_chars: Int?,          -- size of input sent to model
  output_chars: Int?,         -- size of output received
  metadata: JSONB,            -- additional stage-specific data (default '{}')
  created_at: Timestamp,
}
```

The `item_table` + `item_id` pair is a polymorphic reference that can point to any data entity. Indexed for fast per-item history lookup. The `ingestion_id` groups all processing steps from a single reprocess run, enabling cost accounting per ingestion.

### SourcePipelineStatus (Fan-In Counters)

Tracks the progress of the async ingestion pipeline for a single source. Uses atomic counter decrements for fan-in stage transitions: when `pending_extractions` hits 0, the pipeline advances from `extracting` to `extracted` and spawns the next stage's jobs.

```
SourcePipelineStatus {
  source_id: UUID,              -- PK, FK → Source (ON DELETE CASCADE)
  ingestion_id: UUID,           -- groups this pipeline run
  pending_extractions: Int,     -- chunks awaiting LLM extraction (default 0)
  pending_summaries: Int,       -- entities awaiting semantic summary (default 0)
  pending_statements: Int,      -- statements awaiting processing (default 0)
  current_stage: Text,          -- pipeline stage (default 'chunked')
  created_at: Timestamp,
  updated_at: Timestamp,
}
```

**Stage progression:** `chunked` -> `extracting` -> `extracted` -> `summarizing` -> `summarized` -> `composing` -> `complete`. Each transition is triggered by the corresponding counter reaching zero, checked atomically via `UPDATE ... SET pending_X = pending_X - 1 ... RETURNING pending_X`. See migration 016.

### Code Entities (Specialized Node Types)

Code entities are graph Nodes with specialized `node_type` values and code-specific properties in JSONB. They are created by the AST-aware code ingestion pipeline (see [spec/12-code-ingestion](12-code-ingestion.md)).

**Code node types:**
- `code_function` — a function, method, or closure
- `code_struct` — a struct, class, or record type
- `code_trait` — a trait, interface, or protocol
- `code_module` — a module, package, or namespace
- `code_impl` — an impl block (Rust) or class implementation
- `code_type` — a type alias, enum, or constant
- `code_test` — a test function or test module

**Code-specific Node properties** (stored in `properties` JSONB):

```json
{
  "language": "rust",
  "file_path": "engine/crates/covalence-core/src/ingestion/embedder.rs",
  "line_start": 42,
  "line_end": 87,
  "signature": "pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>",
  "visibility": "public",
  "semantic_summary": "Embeds a batch of text strings into vector representations using the configured embedding provider (Voyage or OpenAI). Handles rate limiting and retries.",
  "raw_source": "...",
  "ast_hash": "sha256_of_ast_structure"
}
```

The `semantic_summary` is the key bridge: it's a natural language description of the code's business logic, generated by passing the AST chunk to an LLM. The Node's `embedding` is computed from this summary, not from the raw code — placing code entities in the same semantic vector space as prose concepts.

`ast_hash` enables structural change detection without re-running the LLM: if the AST hash changes, the semantic summary needs regeneration.

## Source Labels

Sources carry metadata that classifies them by project and knowledge domain. These labels are set at ingestion time (not derived at query time) and propagate through extractions to entities. Introduced in [ADR-0018](../docs/adr/0018-graph-type-system.md) and migration 014.

### Project

```
Source.project: TEXT NOT NULL DEFAULT 'covalence'
```

Identifies which project a source belongs to. Multi-project support enables Covalence to ingest multiple codebases without entity conflation. Entities extracted from a project-scoped source inherit the project label. Cross-project entities (e.g., "PostgreSQL", "Rust") are resolved to `project = NULL` (global) during entity resolution when two projects both reference the same concept.

### Domain

```
Source.domain: TEXT  -- 'code' | 'spec' | 'design' | 'research' | 'external'
```

Classifies the source's role in the knowledge graph:

| Domain | Description | Examples |
|--------|-------------|----------|
| `code` | Source code files | `file://engine/**/*.rs`, `file://cli/**/*.go` |
| `spec` | Specification documents | `file://spec/*.md` |
| `design` | Architecture decisions, design docs, project meta | `file://docs/adr/*.md`, `file://VISION.md`, `file://design/*.md`, `file://CLAUDE.md`, `file://MILESTONES.md` |
| `research` | Academic papers, external knowledge | `https://arxiv.org/...`, `https://doi.org/...`, ingested papers |
| `external` | Third-party documentation, API references | External docs not classifiable as research |

**Domain assignment rules** (applied at ingestion time by `derive_domain()` in the source service, based on `source_type` and URI patterns -- not derived at query time):

```
source_type = code                                              → domain = code
URI matches file://spec/                                        → domain = spec
URI matches file://docs/adr/ | file://VISION.md | file://design/ | file://CLAUDE.md | file://MILESTONES.md
                                                                → domain = design
URI matches https://arxiv | https://doi                         → domain = research
URI matches file://engine/ | file://cli/ | file://dashboard/    → domain = code
URI matches http:// | https:// (other)                          → domain = research
Otherwise document                                              → domain = external
```

## Entity Classification

Nodes carry an `entity_class` that groups the 47+ ad-hoc `node_type` values into four controlled categories. This is stored (not derived at query time) for performance and indexing.

```
Node.entity_class: TEXT  -- 'code' | 'domain' | 'actor' | 'analysis'
```

| Entity Class | Node Types | Description |
|-------------|------------|-------------|
| `code` | function, struct, trait, enum, impl_block, constant, module, class, macro | Entities extracted from source code |
| `domain` | concept, technology, algorithm, technique, framework, method, metric, dataset, benchmark, model | Domain concepts from any textual source |
| `actor` | person, organization, location, role | People, organizations, places |
| `analysis` | component | System-generated analysis entities |

**Derivation:** `entity_class` is derived from `node_type` with source-domain context at entity creation time via `derive_entity_class_with_context(node_type, source_domain)`. The base mapping (`derive_entity_class()`) maps `node_type` to one of the four classes, with unknown types defaulting to `domain`. The context-aware wrapper then applies a demotion rule: if the base class is `code` but the source domain is not `code`, the entity is demoted to `domain`. This prevents a "struct" mentioned in a spec document from being classified as a code entity -- it is a domain concept that happens to use a code-type name. Only entities extracted from actual source code (`domain = 'code'`) retain the `code` entity class. See [ADR-0018](../docs/adr/0018-graph-type-system.md).

**Interaction with `canonical_type`:** The ontology clustering system (Wave 8) produces `canonical_type` by merging noisy `node_type` variants. `entity_class` is orthogonal — it classifies the *kind* of entity regardless of how the type was normalized. Both fields coexist. The `entity_class` derivation uses `COALESCE(canonical_type, node_type)` as input, preferring the ontology-clustered type when available.

## Traceability Edge Types

In addition to the existing bridge edges (IMPLEMENTS_INTENT, PART_OF_COMPONENT, THEORETICAL_BASIS) which are bulk-generated via embedding similarity and module path matching, the following **traceability edges** provide precise provenance chains between knowledge domains:

| Edge Type | From | To | Description |
|-----------|------|----|-------------|
| `SPECIFIES` | Node (domain, from spec) | Node (domain, from design) | A spec concept specifies a design decision |
| `DECIDES` | Node (domain, from design) | Node (code) | A design decision leads to a code entity |
| `INFORMS` | Node (domain, from research) | Node (domain, from spec) | Research informs a specification concept |
| `VALIDATES` | Node (domain, from research) | Node (code) | Research validates a code behavior |

These coexist with the Component bridge edges. Components provide automated bulk linking (MODULE_PATH_MAPPINGS → PART_OF_COMPONENT, embedding similarity → IMPLEMENTS_INTENT/THEORETICAL_BASIS). Traceability edges provide precise, curated provenance chains (this ADR DECIDES to use petgraph → this code file implements it).

**Edge validation model:** Soft enforcement. When edges are created, the system validates that source/target entity_class pairs are compatible and logs warnings for violations. Edges are not rejected — this avoids data loss during the transition period and accommodates legitimate edge cases (e.g., a research paper mentioning a specific code entity by name).

## Edge Type Vocabulary (Extended)

In addition to the dynamically-assigned edge types from LLM extraction (see [07-epistemic-model](07-epistemic-model.md#edge-type-vocabulary)), the following structured edge types are used:

### Code Structure Edges

| Edge Type | From | To | Description |
|-----------|------|----|-------------|
| `CALLS` | code_function | code_function | Static call graph edge (from AST) |
| `USES_TYPE` | code_function | code_struct/code_type | Type reference (parameter, return, field) |
| `IMPLEMENTS` | code_impl | code_trait | Trait implementation |
| `CONTAINS` | code_module | code_function/code_struct/code_trait | Module membership |
| `DEPENDS_ON` | code_module | code_module | Module dependency (imports) |

### Cross-Domain Bridge Edges

| Edge Type | From | To | Description |
|-----------|------|----|-------------|
| `IMPLEMENTS_INTENT` | Component | Node (concept/topic) | Links implementation to design intent |
| `PART_OF_COMPONENT` | Node (code_*) | Component | Groups code into logical components |
| `THEORETICAL_BASIS` | Component/Node | Node (concept) | Links implementation to academic foundation |
| `SUPERSEDES` | Source | Source | Version chain for mutating sources |
| `CORRECTS` | Node (claim) | Node (claim) | Explicit retraction or correction |

### Traceability Edges (Curated)

| Edge Type | From (entity_class) | To (entity_class) | Description |
|-----------|---------------------|-------------------|-------------|
| `SPECIFIES` | domain (spec) | domain (design) | Spec concept specifies a design decision |
| `DECIDES` | domain (design) | code | Design decision leads to code entity |
| `INFORMS` | domain (research) | domain (spec) | Research informs a specification |
| `VALIDATES` | domain (research) | code | Research validates code behavior |

### Analysis Edges (Computed)

| Edge Type | From | To | Description |
|-----------|------|----|-------------|
| `SEMANTIC_DRIFT` | Node (code_*) | Component | Generated when code's semantic summary diverges from component description |
| `COVERAGE_GAP` | Node (concept) | Component | Generated when a spec concept has no implementing code |

## Relationships Between Entities

```
Source ──1:N──→ Chunk ──1:N──→ Chunk (parent-child hierarchy)
                  │
                  └──1:N──→ Extraction ──→ Node or Edge

Source ──1:N──→ Statement ──N:1──→ Section
                  │
                  └──1:N──→ Extraction ──→ Node or Edge (statement provenance)

Source ──1:1──→ SourcePipelineStatus (async pipeline tracking)
Source ──supersedes_id──→ Source (this supersedes that)
Source ←──superseded_by── Source (this was superseded by that)

Component ──IMPLEMENTS_INTENT──→ Node (spec/topic concept)
Component ←──PART_OF_COMPONENT── Node (code_* entity)
Component ──THEORETICAL_BASIS──→ Node (research concept)

Node ──1:N──→ NodeAlias
Node ──M:N──→ Node (via Edge)

Source ──N:M──→ Article (via compilation)
Article ──1:N──→ Node (ORIGINATES / covers)

ProcessingLog ──item_table+item_id──→ Source | Chunk | Node | Statement (polymorphic)
```

## Open Questions

- [x] Property graph vs triples vs hybrid → Hybrid: property graph primary + provenance_triples view
- [x] Should Node embeddings be stored or always computed on-the-fly? → Stored. Computed from description on creation, updated on merge. Aggregating chunk embeddings at query time is too expensive.
- [x] Do we need a Community entity for storing detected communities? → Ephemeral. Recomputed during deep consolidation, cached in sidecar memory. No DB table for v1.
- [x] How do we handle versioning when a source is re-ingested with updated content? → Four update classes defined in [05-ingestion](05-ingestion.md#source-update-classes)
- [x] Should chunks store their embedding inline or in a separate embeddings table? → Inline, single embedding model for v1. Model migration via full re-embedding job.
- [x] Should `entity_class` be stored or derived at query time? → Stored. Denormalized but enables direct SQL indexing and filtering without joining a mapping table. Derived deterministically from `node_type` at entity creation. See [ADR-0018](../docs/adr/0018-graph-type-system.md).
- [x] Should edges have a `layer` field? → No. The source/target entity_class is sufficient to infer the layer relationship. Adding a layer field would be redundant and hard to keep in sync.
- [x] Do SPECIFIES/DECIDES traceability edges coexist with Component bridge edges? → Yes. Component bridges are bulk-automated (embedding similarity + module paths). Traceability edges are precise curated links. Different tools for different questions.

## Privacy and PII Handling

Graph RAG systems are **more vulnerable** to structured data extraction than plain RAG systems (Zhang et al., arXiv:2508.17222). While raw text leakage may be reduced (entities are extracted from text, not stored verbatim), entity names and relationships form a structured attack surface that's easier to enumerate.

**Mitigations:**

1. **PII detection at ingestion.** Before chunking, scan source text for PII (names, emails, phone numbers, addresses, SSNs). Use Presidio or equivalent NER-based detector. PII entities are flagged with `is_pii: true` in node metadata.
2. **PII-aware clearance.** Nodes with `is_pii: true` automatically inherit `clearance_level: 0` (local_strict) unless explicitly overridden. This prevents PII from being federated or exposed via public APIs.
3. **Redacted search results.** When a search result references PII nodes, the API redacts entity names to `[REDACTED]` for requests below the required clearance level. The relationship structure is preserved (for graph traversal) but identifying information is hidden.
4. **Entity-level access control.** Beyond clearance levels, support per-entity ACLs for fine-grained access control. This is integrated directly rather than deferred.
5. **Audit log.** Every access to a PII-flagged node is logged with requestor identity, timestamp, and access purpose. Required for compliance (GDPR, CCPA).

**Note:** PII in knowledge graphs is fundamentally different from PII in documents. A document might contain "John Smith works at Acme Corp" — but a KG stores `(John_Smith) -[WORKS_AT]-> (Acme_Corp)` as a first-class fact with an embedding, making it directly queryable. This is both a feature (structured knowledge) and a risk (structured exfiltration).

**Graph poisoning defense:**
GRAGPoison (arXiv:2602.08668) demonstrates that modifying just 0.06% of corpus text can drop QA accuracy from 95% to 50% through relation-centric poisoning. Mitigations:
- **Source reliability tracking** — Our epistemic model (spec 07) assigns reliability scores per source. Low-reliability sources produce low-confidence edges that rank lower in search.
- **Multi-source corroboration** — Edges confirmed by multiple independent sources have higher confidence. A poisoned edge from a single source won't outrank a multi-corroborated edge.
- **Anomaly detection** — New edges that dramatically contradict high-confidence existing edges trigger contention detection, not silent overwrite.
- **Write access control** — Restrict which sources can be ingested. In production, not all data pipelines should have equal trust.
