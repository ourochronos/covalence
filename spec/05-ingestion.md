# 05 — Ingestion Pipeline

**Status:** Implemented

The ingestion pipeline transforms raw unstructured sources into structured graph elements (statements, nodes, edges) with embeddings at multiple granularities.

## Design Goals

1. **Statement-first extraction** — Extract atomic, self-contained knowledge claims from source text. Statements are independently searchable, embeddable, and composable. Noise (bibliography, boilerplate, author blocks) is eliminated at source.
2. **Format-agnostic** — Handle documents, web pages, conversations, code, and arbitrary text.
3. **Structure-preserving** — Capture document hierarchy (title, headings, sections, paragraphs) as metadata, not just flat text.
4. **Metadata-rich** — Source type, author, date, URI, format-specific properties are all first-class.
5. **Offset Projection Ledger** — Use an automated fastcoref pass to normalize pronouns into explicit referents. Generate an offset projection ledger to map mutated text indices exactly back to the canonical source, ensuring perfect provenance.
6. **Two-pass LLM extraction** — Statement extraction and entity extraction are separate passes to avoid attention dilution. Pass 1 extracts self-contained statements from the coref-resolved text. Pass 2 extracts entities and relationships from those statements. Both passes use the pluggable `ChatBackend` abstraction (see [LLM Backend Abstraction](#llm-backend-abstraction)), enabling multi-provider failover without changing pipeline logic.
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

A dedicated fastcoref sidecar model processes the canonical Markdown text. It resolves all pronouns and anaphora to their explicit referents. The sidecar is validated at engine startup via `FastcorefClient::validate()` — if the sidecar is unreachable or returns an unexpected format, coref is disabled with an error log rather than silently failing during ingestion. All HTTP sidecars (fastcoref, PDF converter) follow this validate-on-startup pattern.

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

### Stage 6: Windowed Statement Extraction

The `mutated_text` is processed in overlapping windows (e.g., 5 paragraphs with 2-paragraph overlap). Each window is sent to the configured `ChatBackend` with a strict JSON-schema prompt that extracts atomic, self-contained factual claims.

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
- The `ChatBackend` compiles a Section title and narrative body.
- The Section body is embedded for retrieval.

Finally, an LLM pass compiles a **Source Summary** based on all section titles and bodies.

### Stage 9: Entity & Triple Extraction

Entities and relationships are extracted directly from the self-contained statements (Pass 2 of the two-pass pipeline).
- Prompt: "Extract (subject -> relationship -> object) triples from the provided statement. Relationship `rel_type` should be UPPER_SNAKE_CASE verbs."
- Using statements as the source instead of raw chunks eliminates noise and dramatically improves relationship precision.
- Nearby graph nodes are injected into the prompt context to encourage the LLM to use `is_existing_match: true` and map directly to canonical names.

### Stage 10: Reverse Projection

Before committing to the database, the `mutated_byte_start`/`end` indices of every Statement and extracted Entity are run backward through the Offset Projection Ledger.
- Logic: "Entity found at [25, 33]. Does this overlap with a `mutated_span` in the ledger? Yes. Replace with `canonical_span` [25, 27]."
- The database stores the exact canonical indices, guaranteeing UI deep-dives land precisely on the original text.

### Stage 11: Entity Resolution & Store

**Pre-resolution coreference substitution:** Before entering the 5-tier cascade, each extracted entity name is checked against the `coref_map` built in Stage 5. If the entity name is a coreferent mention (abbreviation, pronoun artifact, partial name), the canonical referent name is substituted. This prevents duplicate nodes — e.g., "NLP" resolves as "Natural Language Processing" from the start, rather than creating a separate node that later needs deduplication. The original mention is preserved as an alias.

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

Sources don't all update the same way. The system recognizes five classes of updates, each with distinct handling. Update class detection is automatic: when a source is ingested with a URI that matches an existing source, the system compares content overlap to determine the class.

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

### Supersession Tracking

When a source update creates a new source record (Classes 2 and 3), the system maintains explicit lineage:
- The new source records `supersedes_id` pointing to the old source, along with the detected `update_class` and an incremented `content_version`.
- The old source is marked with `superseded_by` (pointing to the new source) and `superseded_at` (timestamp).
- Old extractions are marked as superseded via `ExtractionRepo::mark_superseded_by_source` so they no longer pollute search results.
- Old data (extractions, chunks, ledger entries) is cleaned up only **after** the new pipeline succeeds, ensuring rollback safety.

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

### Persistent Retry Queue

Failed pipeline stages are handled by a persistent retry queue backed by PostgreSQL (`retry_jobs` table). The `RetryQueueService` manages the full lifecycle:

**Job kinds:**
| Kind | Description |
|------|-------------|
| `ReprocessSource` | Full source reprocessing (re-chunk, re-extract, re-embed) |
| `ExtractChunk` | Extract entities from a single chunk |
| `ExtractStatements` | Extract statements from a source's chunks |
| `ExtractEntities` | Extract entities from a source's statements |
| `SummarizeEntity` | Generate semantic summary for a single code entity |
| `ComposeSourceSummary` | Fan-in: compose file-level summary from entity summaries |
| `EmbedBatch` | Embed a batch of items (nodes or chunks) |
| `SynthesizeEdges` | Synthesize co-occurrence edges across the graph |

**Error classification and backoff:**
- **Permanent** (404, bad payload) — sent to dead-letter queue immediately (no retry).
- **Rate limit** (429, quota exhaustion) — long backoff starting at 15 minutes, doubling per attempt.
- **Transient** (timeout, connection reset) — standard exponential backoff (`base * 2^(attempt-1)`, capped at max).

**Concurrency control:**
- Per-kind semaphores limit concurrent reprocess jobs and edge synthesis jobs independently.
- Job claiming uses `FOR UPDATE SKIP LOCKED` to prevent double-dispatch across workers.
- Fan-in triggers use PostgreSQL advisory locks (`pg_try_advisory_xact_lock`) to prevent duplicate compose jobs when multiple chunk extractions complete simultaneously.

**Fan-in triggers:**
The async pipeline forms a DAG: `ExtractChunk → SummarizeEntity → ComposeSourceSummary`. When all jobs of one stage complete for a source, a fan-in check automatically enqueues the next stage. Advisory locks ensure exactly-once triggering even under concurrent completion.

**Watchdog:**
A background watchdog task (every 2 minutes) detects sources that have stalled mid-pipeline — all child jobs completed but the fan-in trigger didn't fire — and re-enqueues the missing compose job.

**Scheduler:**
A periodic scheduler auto-enqueues maintenance jobs on a cadence (edge synthesis every 6 hours, garbage collection every 7 days).

**Recovery:**
On startup, the worker recovers orphaned `running` jobs (from engine crashes) back to `pending` status with immediate retry eligibility.

**Administration:**
Dead-letter jobs can be inspected, retried, resurrected, or cleared via the admin API.

## Performance Considerations

- **LLM Throughput:** High-throughput providers are preferred for concurrent windowed extraction. The `ChainChatBackend` enables automatic failover to maintain throughput when a provider is rate-limited.
- **Parallel extraction:** Multiple windows sent to LLM concurrently (respecting bounded semaphore limits).
- **Ledger efficiency:** Reverse projection is mathematically negligible compared to LLM-driven coref mutation logic.

## LLM Backend Abstraction

All LLM-driven pipeline stages (statement extraction, entity extraction, section compilation, source summary, semantic summaries) use a pluggable `ChatBackend` trait rather than calling a specific provider directly.

### ChatBackend Trait

```rust
#[async_trait]
pub trait ChatBackend: Send + Sync {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<ChatResponse>;
}
```

Every call returns a `ChatResponse` containing both the response text and a `provider` label (e.g. `"claude(haiku)"`, `"gemini(2.5-flash)"`, `"http(gpt-4)"`).

### Implementations

| Backend | Transport | Use case |
|---------|-----------|----------|
| `CliChatBackend` | CLI subprocess (claude, gemini, copilot) | Primary: shells out to installed CLI tools |
| `HttpChatBackend` | OpenAI-compatible HTTP API | Any provider with an HTTP endpoint |
| `FallbackChatBackend` | Two-layer failover (primary → secondary) | Legacy compatibility |
| `ChainChatBackend` | Multi-provider ordered failover | Production: tries providers in sequence |

The `ChainChatBackend` is the standard production configuration. It tries each provider in order, falling back to the next on any error, and logs which provider ultimately handled the request.

### Provider Attribution

Every LLM call records which provider handled the request in the entity or source's `processing` metadata (a JSONB column). This includes the provider label, model name, timestamp, latency in milliseconds, and prompt version. Example:

```json
{
  "summary": {
    "model": "haiku",
    "provider": "claude(haiku)",
    "at": "2026-03-15T14:30:00Z",
    "ms": 1250,
    "prompt_version": 3
  }
}
```

This enables auditing which provider generated each piece of extracted knowledge and selective reprocessing when prompts or models change.

## Prompt Templates

Extraction and compilation prompts are loaded from `engine/prompts/*.md` files at runtime. If the file is not found on disk (e.g. in test environments), a compiled-in fallback is used via `include_str!`.

| File | Stage |
|------|-------|
| `entity_extraction.md` | Stage 9: entity extraction from statements |
| `relationship_extraction.md` | Stage 9: triple extraction from statements |
| `section_compilation.md` | Stage 8: section title + narrative compilation |
| `source_summary.md` | Stage 8: source-level summary from sections |
| `code_summary.md` | Code pipeline: semantic summary for code entities |

Templates are loaded once via `OnceLock` and cached for the process lifetime. Each template has a version constant (`SUMMARY_PROMPT_VERSION`) tracked in processing metadata, enabling selective reprocessing of entities summarized with older prompt versions.

This design allows prompt iteration without recompilation — edit the `.md` file and restart the engine.

## Open Questions

- [x] Statement vs chunk pipeline → Statement pipeline is the absolute primary path. Chunk legacy removed.
- [x] Single-pass vs two-pass extraction → Two-pass (Statements -> Triples) prevents attention dilution.
- [x] Offset Projection Ledger → Implemented at Stage 5 and 10 to ensure pristine canonical provenance.
- [x] HDBSCAN entity resolution → Implemented as the definitive Tier 5 catch-all for dynamic entity grouping.
- [x] LLM Selection → Backend-agnostic via `ChatBackend` trait. Default production chain: Claude Haiku → Copilot → Gemini Flash.
