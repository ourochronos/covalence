# 05 — Ingestion Pipeline

**Status:** Implemented

The ingestion pipeline transforms raw unstructured sources into structured graph elements (statements, nodes, edges) with embeddings at multiple granularities.

## Design Goals

1. **Statement-first extraction** — Extract atomic, self-contained knowledge claims from source text. Statements are independently searchable, embeddable, and composable. Noise (bibliography, boilerplate, author blocks) is eliminated at source.
2. **Format-agnostic** — Handle documents, web pages, conversations, code, and arbitrary text.
3. **Structure-preserving** — Capture document hierarchy (title, headings, sections, paragraphs) as metadata, not just flat text.
4. **Metadata-rich** — Source type, author, date, URI, format-specific properties are all first-class.
5. **Offset Projection Ledger** — Use an automated fastcoref pass to normalize pronouns into explicit referents. Generate an offset projection ledger to map mutated text indices exactly back to the canonical source, ensuring perfect provenance.
6. **Two-pass LLM extraction** — Statement extraction and entity extraction are separate passes to avoid attention dilution. Pass 1 extracts self-contained statements from the coref-resolved text. Pass 2 extracts entities and relationships from those statements. Both passes use Gemini Flash 3.0 for high-throughput, massive-context processing.
7. **Entity resolution** — Deduplicate entities across sources using vector similarity, graph context, and an HDBSCAN catch-all for dynamic grouping of unclassified entities.
8. **Incremental & Idempotent** — Re-ingesting an updated source should update, not duplicate.

## Pipeline Architecture

The ingestion pipeline has two paths that share the same front-end stages (Accept → Convert → Parse → Normalize) but diverge after normalization:

```
                        ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
                        │  Accept   │─→│ Convert  │─→│  Parse   │─→│ Normalize│
                        │  Source   │  │  to MD   │  │  + Meta  │  │  to MD   │
                        └──────────┘  └──────────┘  └──────────┘  └────┬─────┘
                                                                       │
                                                    ┌──────────────────┴──────────────────┐
                                                    ▼                                     ▼
                                         ┌────────────────────┐                ┌────────────────────┐
                                         │  PROSE PIPELINE    │                │  CODE PIPELINE     │
                                         │  (default)         │                │  (source_type=code)│
                                         │                    │                │                    │
                                         │ 1. Fastcoref       │                │ 1. Tree-sitter     │
                                         │ 2. Statements      │                │ 2. AST Chunking    │
                                         │ 3. Embed           │                │ 3. Semantic Summary│
                                         │ 4. HAC Cluster     │                │ 4. Extract Stmts   │
                                         │ 5. Compile Sections│                │ 5. Extract Edges   │
                                         │ 6. Compile Summary │                │ 6. Component Link  │
                                         │ 7. Entity Extract  │                │                    │
                                         │ 8. Reverse Project │                │                    │
                                         │ 9. Entity Resolve  │                │                    │
                                         └────────────────────┘                └────────────────────┘
```

### Stage 1: Accept Source

Input: Raw content + metadata envelope.

```rust
struct SourceInput {
    content: SourceContent,     // enum: FilePath, Url, RawText, Conversation
    source_type: SourceType,
    metadata: SourceMetadata,   // author, title, date, uri, custom fields
    clearance_level: ClearanceLevel, // Local, FederatedTrusted, FederatedPublic
}

enum SourceType {
    Document,       // PDF, DOCX, etc.
    WebPage,        // HTML
    Conversation,   // Chat logs, transcripts, threads
    Code,           // Source code files
    Api,            // Structured API responses
    Manual,         // User-provided knowledge
}
```

- Compute content hash (SHA-256) for dedup
- Check if source already exists (by hash) — if so, route to update handling (see [Source Update Classes](#source-update-classes))
- Create `sources` record with full document-level metadata

### Stage 2: Format Conversion

Before parsing, raw source content passes through a pluggable converter that produces Markdown. Built-in converters handle PDF, HTML, Markdown, Plain Text, and Code.

### Stage 3: Parse + Extract Metadata

Convert raw format into structured representation, preserving all available structure (headings, page numbers, tables, code blocks).

### Stage 4: Normalize to Markdown

All parsed output is converted to extended Markdown as the canonical intermediate format.
- Document metadata → YAML frontmatter block
- Headings → `#`/`##`/`###`
- Tables → Markdown pipe tables

## Prose Pipeline (Primary Path)

After normalization (Stage 4), the prose pipeline takes over for all non-code sources.

### Stage 5: Coreference Resolution & Offset Projection Ledger

A dedicated fastcoref sidecar model processes the canonical Markdown text. It resolves all pronouns and anaphora to their explicit referents.

Because modifying the text destroys the absolute coordinate system of the original document, this stage generates an **Offset Projection Ledger** alongside the `mutated_text`.

```json
{
  "canonical_text": "Tim Cook took the stage. He announced the product.",
  "mutated_text": "Tim Cook took the stage. Tim Cook announced the product.",
  "ledger": [
    {
      "canonical_span": [25, 27],
      "canonical_token": "He",
      "mutated_span": [25, 33],
      "mutated_token": "Tim Cook",
      "delta": +6
    }
  ]
}
```

### Stage 6: Windowed Statement Extraction (Gemini Flash 3.0)

The `mutated_text` is processed in overlapping windows (e.g., 5 paragraphs with 2-paragraph overlap). Each window is sent to **Gemini Flash 3.0** with a strict JSON-schema prompt that extracts atomic, self-contained factual claims.

**Requirements:**
1. **Self-containment** — Every statement must be independently meaningful.
2. **Noise rejection** — Bibliography entries, author affiliations, and boilerplate are explicitly skipped.
3. **Heading context** — The document heading path is provided to the LLM for disambiguation.

The LLM returns statements with their `mutated_byte_start`/`end` indices referencing the `mutated_text`.

### Stage 7: Embed Statements

Each extracted statement is embedded using the configured embedding provider (e.g., Voyage AI `voyage-context-3`).

### Stage 8: HAC Clustering & Compilation

Statements are clustered into **Sections** using Hierarchical Agglomerative Clustering (HAC) on their embeddings (cosine distance, complete linkage, threshold 0.75).

For each cluster:
- **Gemini Flash 3.0** compiles a Section title and narrative body.
- The Section body is embedded for retrieval.

Finally, an LLM pass compiles a **Source Summary** based on all section titles and bodies.

### Stage 9: Entity & Triple Extraction (Gemini Flash 3.0)

Entities and relationships are extracted directly from the self-contained statements (Pass 2 of the two-pass pipeline).
- Prompt: "Extract (subject -> relationship -> object) triples from the provided statement. Relationship `rel_type` should be UPPER_SNAKE_CASE verbs."
- Using statements as the source instead of raw chunks eliminates noise and dramatically improves relationship precision.
- Nearby graph nodes are injected into the prompt context to encourage the LLM to use `is_existing_match: true` and map directly to canonical names.

### Stage 10: Reverse Projection

Before committing to the database, the `mutated_byte_start`/`end` indices of every Statement and extracted Entity are run backward through the Offset Projection Ledger.
- Logic: "Entity found at [25, 33]. Does this overlap with a `mutated_span` in the ledger? Yes. Replace with `canonical_span` [25, 27]."
- The database stores the exact canonical indices, guaranteeing UI deep-dives land precisely on the original text.

### Stage 11: Entity Resolution & Store

Match extracted entities against existing nodes in the graph using a 5-Tier Resolution cascade:

1. **Exact canonical name** — Case-insensitive lookup on `nodes.canonical_name`.
2. **Alias match** — Case-insensitive lookup in `node_aliases`.
3. **Vector cosine similarity (Gravity Well)** — Query closest node via pgvector. Match accepted if similarity ≥ `COVALENCE_RESOLVE_VECTOR_THRESHOLD` (0.85).
4. **Fuzzy trigram** — `pg_trgm` `similarity()` against all nodes.
5. **HDBSCAN Catch-All** — Entities failing tiers 1-4 are dumped into an unclassified residual pool. HDBSCAN runs periodically over this pool to detect high-density clusters of synonyms or variations that fell below pairwise thresholds. Clustered entities are flagged for human curation in a single dashboard to be officially promoted into new Canonical Entities (Gravity Wells).

Storage Phase:
1. Upsert nodes (new or merged)
2. Insert edges
3. Insert extraction provenance records (referencing `statement_id`)
4. Update graph sidecar

---

## Code Pipeline

When `source_type = "code"`, the pipeline diverges to use AST-aware processing. See [12-code-ingestion](12-code-ingestion.md) for full details. The code pipeline outputs the same unified primitives (Statements, Nodes, Edges) and shares the same vector space via Semantic Summaries.

---

## Source Update Classes

Sources don't all update the same way. The system recognizes four classes of updates, each with distinct handling:

### Class 1: Append-Only (Conversations, Logs, Event Streams)
- Hash and process only the **delta** (new messages/entries).
- Link new statements to existing ones via `APPENDED_AFTER` sequence edges.

### Class 2: Mutating & Versioned (Code, Documentation, Specs)
- Ingest as a **new source record** (new `document_id`).
- Link to predecessor via `SUPERSEDES` structural edge.

### Class 3: Explicit Retractions & Corrections
- Extract the corrected claim as a new entity.
- Create an adversarial `CORRECTS` edge to zero out the epistemic confidence of the old claim.

### Class 4: Structural Refactoring (Container Changes)
- Incoming text matches existing SHA-256 hash but has different URI or hierarchy.
- **Do not re-extract.** Update chunk/statement-level metadata and provenance pointers.

### Class 5: Takedown / Deletion (GDPR, User Deletion)
- Soft-delete source, execute TMS Cascade to mark sole-provenance nodes as inactive or deleted. Run orphan detection to clean up graph islands.

---

## Three-Timescale Consolidation

Ingestion is the online (fast) tier of a three-timescale consolidation pipeline.

1. **Online Tier (Seconds):** Parse → Fastcoref → Statement Extract → Entity Extract → Resolve → Store.
2. **Batch Tier (Hours):** Group sources by topic cluster. Compile global Articles (right-sized retrieval units). Detect contentions.
3. **Deep Tier (Daily+):** Structural maintenance (TrustRank recalibration, HDBSCAN entity clustering, domain topology maps).

---

## Error Handling

- Each stage is independently retriable.
- LLM extraction failures (rate limits, malformed output) → retry with exponential backoff.
- Resumable pipeline state prevents re-running expensive LLM calls on crash.

## Performance Considerations

- **Gemini Flash 3.0 Throughput:** Leverage high TPS and massive context window for concurrent windowed extraction.
- **Parallel extraction:** Multiple windows sent to LLM concurrently (respecting bounded semaphore limits).
- **Ledger efficiency:** Reverse projection is mathematically negligible compared to LLM-driven coref mutation logic.

## Open Questions

- [x] Statement vs chunk pipeline → Statement pipeline is the absolute primary path. Chunk legacy removed.
- [x] Single-pass vs two-pass extraction → Two-pass (Statements -> Triples) prevents attention dilution.
- [x] Offset Projection Ledger → Implemented at Stage 5 and 10 to ensure pristine canonical provenance.
- [x] HDBSCAN entity resolution → Implemented as the definitive Tier 5 catch-all for dynamic entity grouping.
- [x] LLM Selection → Gemini Flash 3.0 used for bulk extraction due to required context window and throughput.
