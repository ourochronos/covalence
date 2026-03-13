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
  supersedes_id: UUID?,     -- FK → Source (previous version, if applicable)
  content_version: Int,     -- version counter, increments on update
}
```

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
  description: Text?,
  properties: JSONB,
  embedding: Vector?,           -- optional: derived from description or aggregated from chunks
  embedding_model: String?,     -- tracks which model produced the embedding (null if no embedding)
  confidence_breakdown: JSONB?, -- Subjective Logic opinion tuple (b, d, u, a)
  clearance_level: Int,         -- most restrictive of all sources this node was extracted from
  first_seen: Timestamp,
  last_seen: Timestamp,
  mention_count: Int,
}
```

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

Component ──IMPLEMENTS_INTENT──→ Node (spec/topic concept)
Component ←──PART_OF_COMPONENT── Node (code_* entity)
Component ──THEORETICAL_BASIS──→ Node (research concept)

Node ──1:N──→ NodeAlias
Node ──M:N──→ Node (via Edge)

Source ──N:M──→ Article (via compilation)
Article ──1:N──→ Node (ORIGINATES / covers)
```

## Open Questions

- [x] Property graph vs triples vs hybrid → Hybrid: property graph primary + provenance_triples view
- [x] Should Node embeddings be stored or always computed on-the-fly? → Stored. Computed from description on creation, updated on merge. Aggregating chunk embeddings at query time is too expensive.
- [x] Do we need a Community entity for storing detected communities? → Ephemeral. Recomputed during deep consolidation, cached in sidecar memory. No DB table for v1.
- [x] How do we handle versioning when a source is re-ingested with updated content? → Four update classes defined in [05-ingestion](05-ingestion.md#source-update-classes)
- [x] Should chunks store their embedding inline or in a separate embeddings table? → Inline, single embedding model for v1. Model migration via full re-embedding job.

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
