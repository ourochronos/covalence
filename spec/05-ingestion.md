# 05 — Ingestion Pipeline

**Status:** Implemented

The ingestion pipeline transforms raw unstructured sources into structured graph elements (statements, nodes, edges) with embeddings at multiple granularities.

## Design Goals

1. **Statement-first extraction** — Extract atomic, self-contained knowledge claims from source text as the primary path. Statements are independently searchable, embeddable, and composable. Noise (bibliography, boilerplate, author blocks) is eliminated at source, not downstream.
2. **Format-agnostic** — Handle documents, web pages, conversations, code, and arbitrary text
3. **Structure-preserving** — Capture document hierarchy (title, headings, sections, paragraphs) as metadata, not just flat text
4. **Metadata-rich** — Source type, author, date, URI, format-specific properties are all first-class
5. **Coreference resolution** — All statements have pronouns resolved to explicit referents during extraction. Self-contained claims are the retrieval unit.
6. **Two-pass extraction** — Statement extraction and entity extraction are separate passes to avoid attention dilution. Pass 1 extracts self-contained statements with coref resolution. Pass 2 extracts entities and relationships from those statements.
7. **Entity resolution** — Deduplicate entities across sources using vector similarity + graph context
8. **Incremental** — Re-ingesting an updated source should update, not duplicate
9. **Idempotent** — Re-ingesting the same content (same hash) is a no-op
10. **Markdown normalization** — All formats convert to extended Markdown as the intermediate representation

## Pipeline Architecture

The ingestion pipeline has **two paths** that share the same front-end stages (Accept → Convert → Parse → Normalize) but diverge after normalization:

```
                        ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
                        │  Accept   │─→│ Convert  │─→│  Parse   │─→│ Normalize│
                        │  Source   │  │  to MD   │  │  + Meta  │  │  to MD   │
                        └──────────┘  └──────────┘  └──────────┘  └────┬─────┘
                                                                       │
                                                    ┌──────────────────┼──────────────────┐
                                                    ▼                  ▼                  ▼
                                         ┌────────────────┐  ┌────────────────┐  ┌────────────────┐
                                         │  STATEMENT      │  │  CODE           │  │  CHUNK (legacy) │
                                         │  PIPELINE       │  │  PIPELINE       │  │  PIPELINE       │
                                         │  (default)      │  │  (source_type   │  │  (backward      │
                                         │                 │  │   = "code")     │  │   compat.)      │
                                         └────────────────┘  └────────────────┘  └────────────────┘
```

**Statement pipeline** (default for prose): Windowed LLM statement extraction → embed → HAC clustering → compile sections → compile source summary → entity extraction from statements → entity resolution → store.

**Code pipeline** (for `source_type = "code"`): Tree-sitter AST parse → chunk by AST boundary → semantic summary → embed → statement extraction → structural edge extraction → component linking → store. See [12-code-ingestion](12-code-ingestion.md).

**Chunk pipeline** (legacy, retained for backward compatibility): Hierarchical chunking → embed → embedding landscape analysis → graduated LLM extraction → entity resolution → store. See [Appendix: Legacy Chunk Pipeline](#appendix-legacy-chunk-pipeline).

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

**Document-Level Metadata Schema:**

```json
{
  "document_id": "sha256_hash",
  "source_uri": "https://...",
  "format_origin": "pdf | html | markdown | conversation | code",
  "ingestion_timestamp": "2026-03-07T10:29:20Z",
  "authors": ["extracted or provided author list"],
  "publication_date": "YYYY-MM-DD",
  "clearance_level": "local_strict | federated_trusted | federated_public",
  "federation_origin": "local | external_node_id",
  "content_version": 1,
  "supersedes": null
}
```

### Stage 2: Format Conversion

Before parsing, raw source content passes through a pluggable converter that produces Markdown. The `SourceConverter` trait defines the interface:

```rust
#[async_trait]
pub trait SourceConverter: Send + Sync {
    /// Convert raw bytes to Markdown.
    async fn convert(&self, content: &[u8], content_type: &str) -> Result<String>;
    /// MIME types this converter handles.
    fn supported_types(&self) -> &[&str];
}
```

A `ConverterRegistry` dispatches to the first registered converter whose `supported_types()` matches the incoming MIME type (parameters like `charset=utf-8` are stripped before matching). Built-in converters:

| Converter | Content Types | Behavior |
|-----------|--------------|----------|
| `MarkdownConverter` | `text/markdown`, `text/x-markdown` | UTF-8 passthrough (invalid bytes replaced with U+FFFD) |
| `PlainTextConverter` | `text/plain` | Wraps body under an `# Untitled Document` heading |
| `HtmlConverter` | `text/html`, `application/xhtml+xml` | State-machine tag stripping: `<h1>`–`<h6>` → `#`–`######`, `<p>` → blank line, `<br>` → newline, `<li>` → bullet, `<script>`/`<style>` blocks removed entirely, common HTML entities decoded |
| `ReaderLmConverter` | `text/html`, `application/xhtml+xml` | High-quality HTML → Markdown via ReaderLM-v2 MLX sidecar; falls back to `HtmlConverter` tag stripping when sidecar is unavailable. Registered front so it takes priority over `HtmlConverter`. |
| `PdfConverter` | `application/pdf` | PDF → text via external sidecar endpoint; falls back to error when sidecar is unavailable |
| `CodeConverter` | `text/x-rust`, `text/x-python`, `text/x-script.python` | Annotated Markdown via tree-sitter AST: extracts functions, structs, classes with signatures and doc comments |

Custom converters can be registered via `ConverterRegistry::register()`. The registry is attached to the source service with `SourceService::with_converter_registry()`, which enables the conversion step during `ingest()`. When no registry is configured, raw content is passed directly to the parser.

### Stage 3: Parse + Extract Metadata

Convert raw format into structured representation, preserving all available structure.

| Format | Parser | Structural Output |
|--------|--------|-------------------|
| PDF | pdf-extract / pdfium | Pages → paragraphs, with headings, page numbers |
| HTML/Web | readability + html5ever | Headings → sections → paragraphs |
| Markdown | pulldown-cmark | Headings → sections → paragraphs (passthrough) |
| Plain text | Paragraph splitting | Paragraphs (by blank lines) |
| Conversation | Custom (speaker turns) | Turn-based segments with speaker metadata |
| Code | tree-sitter | Functions, classes, modules with hierarchy |
| DOCX | docx-rs | Headings → sections → paragraphs, with styles |
| Tables (CSV/XLSX) | Custom | Row groups with column headers as context |

**Key principle:** Preserve as much structural information as possible. Heading levels, page numbers, speaker identity, code structure, table boundaries — all become chunk metadata.

### Stage 4: Normalize to Markdown

All parsed output is converted to extended Markdown as the canonical intermediate format before chunking.

**Why Markdown:**
- LLMs are natively trained on Markdown — `### Financial Outlook` is semantically understood as a subsection
- Preserves hierarchy via heading levels without token-heavy markup (unlike HTML)
- Tables render cleanly in Markdown pipe syntax (`| Col A | Col B |`), allowing relational reading
- Supports YAML frontmatter for injecting metadata directly into the document

**Normalization rules:**
1. Document metadata → YAML frontmatter block
2. Headings → `#`/`##`/`###` with appropriate nesting
3. Tables → Markdown pipe tables
4. Code blocks → fenced code blocks with language tags
5. Conversation turns → blockquotes with speaker attribution
6. Images/figures → placeholder with caption metadata (no embedding of binary content)

**Output format:**

```markdown
---
source_id: "abc-123"
title: "Q3 Earnings Report"
authors: ["Jane Smith"]
publication_date: "2026-01-15"
format_origin: "pdf"
---

# Q3 Earnings Report

## Financial Outlook

Revenue increased 15% year-over-year...

| Quarter | Revenue | Growth |
|---------|---------|--------|
| Q1      | $2.1B   | 12%    |
| Q2      | $2.3B   | 14%    |
| Q3      | $2.5B   | 15%    |
```

## Statement Pipeline (Primary Path)

After normalization (Stage 4), the statement pipeline takes over for all prose sources. This is the default and primary extraction path (see [ADR-0015](../docs/adr/0015-statement-first-extraction.md)).

The statement pipeline inverts the traditional chunk-first approach: extract knowledge from raw text first, then build hierarchy from the extracted knowledge. Noise (bibliography entries, boilerplate, author blocks) is eliminated at source — they are never extracted as statements because the LLM is prompted to skip them.

### Stage 5s: Windowed Statement Extraction

The normalized Markdown text is processed in overlapping windows. Each window is sent to the LLM with a prompt that instructs it to extract atomic, self-contained factual claims.

**Window configuration:**

| Env Var | Default | Description |
|---------|---------|-------------|
| `COVALENCE_STATEMENT_WINDOW_SIZE` | `5` | Number of paragraphs per extraction window |
| `COVALENCE_STATEMENT_WINDOW_OVERLAP` | `2` | Overlap paragraphs between adjacent windows |

**Key requirements in the extraction prompt:**

1. **Self-containment** — Every statement must be independently meaningful. No pronouns, no "this approach", no "the authors."
2. **Coreference resolution** — All anaphora resolved to explicit referents during extraction. This is the highest-value step in the pipeline. Statements with unresolved pronouns ("it models uncertainty") lose retrievability because the embedding captures "uncertainty modeling" without connecting it to the specific concept.
3. **Noise rejection** — Bibliography entries, acknowledgments, author affiliations, page headers/footers, and boilerplate are not extracted as statements.
4. **Heading context** — The document heading path at each window position is provided to the LLM for disambiguation.

**Example:**
- **Source text:** "Josang proposed Subjective Logic in 2001. It models uncertainty using opinion tuples."
- **Extracted statement:** "Subjective Logic, proposed by Audun Josang in 2001, models uncertainty using opinion tuples consisting of belief, disbelief, uncertainty, and base rate."

**Deduplication:** Statements are deduplicated within and across windows by content hash (exact) and embedding cosine similarity > 0.92 (semantic).

**Two-pass design rationale:** The Gemini architecture conversation identified that asking an LLM to do coreference resolution, statement extraction, AND entity/relationship extraction in a single prompt causes completeness to plummet due to attention dilution. The statement pipeline enforces separation: Pass 1 (this stage) handles statement extraction with coref resolution. Pass 2 (Stage 9s) handles entity/relationship extraction from the already-clean statements.

### Stage 6s: Embed Statements

Each extracted statement is embedded using the configured embedding provider. Statement embeddings use `COVALENCE_EMBED_DIM_STATEMENT` (default 1024) via Matryoshka truncation.

### Stage 7s: HAC Clustering

Statements are clustered into sections using Hierarchical Agglomerative Clustering:

- **Linkage:** Complete linkage (furthest-neighbor)
- **Distance metric:** Cosine distance between statement embeddings
- **Threshold:** 0.75 (configurable via `COVALENCE_CLUSTER_THRESHOLD`)

Each cluster becomes a Section — a coherent topic group of related statements.

### Stage 8s: Compile Sections

For each cluster/section, the LLM compiles a summary:

1. **Section title** — A concise topic label (3-7 words)
2. **Section body** — A compiled summary of the clustered statements (200-800 tokens)
3. **Section embedding** — The body is embedded for retrieval

Sections are the compiled equivalent of what articles are in the batch consolidation tier — right-sized summaries optimized for retrieval.

### Stage 8.5s: Compile Source Summary

The LLM generates a single-paragraph summary of the entire source based on all section titles and bodies. This summary is stored on the `sources.source_summary` field and embedded on the `sources.embedding` field.

### Stage 9s: Entity Extraction from Statements

Entities and relationships are extracted from statements (not chunks). This is the second pass of the two-pass pipeline:

- Each non-evicted statement is sent to the extraction LLM with the same prompt structure as Stage 8.3 (see [Appendix: Legacy Chunk Pipeline](#appendix-legacy-chunk-pipeline))
- Extraction runs concurrently with a semaphore to bound parallelism
- Noise entities are filtered via `is_noise_entity()` before storage
- Provenance traces to the statement, not a chunk: `extraction.statement_id` is set

Entity resolution then proceeds as in Stage 9 (see [Entity Resolution](#stage-9-entity-resolution--store)).

### Coreference Resolution

Statement extraction prompts explicitly require **full coreference resolution**. Every statement must be self-contained — no pronouns, no "this approach", no "the authors". The LLM resolves all anaphora to their explicit referents during extraction.

**Current approach:** The LLM performs coref resolution as part of statement extraction. The source text is not mutated — statements store `byte_start`/`byte_end` referencing the canonical (unmodified) source text.

**Future optimization — Offset Projection Ledger:** If a separate coref resolution pass (e.g., fastcoref) is added before LLM extraction to pre-resolve coreferences in the source text, the byte offsets of downstream entities would shift. An offset projection ledger would maintain a mapping between canonical and coref-resolved text:

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

This is explicitly a future approach — the current LLM-in-prompt coref resolution works well and avoids the complexity of text mutation and offset tracking. The ledger becomes valuable if extraction volume justifies a dedicated coref model (faster, cheaper than LLM per-token) as a preprocessing step.

### Re-extraction Logic

Re-extracting a source produces a superset of statements with verification of missing ones:

1. Load existing statements for the source
2. Extract new statement set from current source text
3. Match new vs existing by content hash (exact) then embedding cosine > 0.92 (semantic)
4. Existing statements not found in new set: verify via word-overlap heuristic (>30% match = still supported in source text → keep; otherwise → evict)
5. Store new statements with embeddings
6. Re-cluster all non-evicted statements
7. Recompile sections and source summary

This makes re-extraction a clean set operation with mechanical verification — no ambiguity about what changed.

---

## Appendix: Legacy Chunk Pipeline

The chunk-first pipeline described below is retained for backward compatibility and for sources where chunk-level retrieval is specifically desired. **For new prose sources, the statement pipeline (above) is the default.**

The chunk pipeline embeds text segments, uses embedding landscape analysis to determine extraction targets, then runs graduated LLM extraction. While superseded by the statement pipeline for primary extraction, it provides useful infrastructure (hierarchical chunking, landscape analysis) that informs future work.

### Stage 5: Hierarchical Chunking

Decompose the normalized Markdown into a chunk tree:

```
Section "Introduction"        — split at # headings
├── Paragraph 1               — split at \n\n when section exceeds max size
└── Paragraph 2
Section "Methods"
└── ...
```

There is no document-level chunk. The document embedding lives on the `Source` record directly (migration 003 added the `embedding` column to `sources`). This avoids a redundant chunk that would duplicate the source's metadata and embedding.

**Chunking strategy (hybrid):**

1. **Structural first** — Use Markdown heading levels as primary boundaries. Hard breaks at `#`, `##`, `###`. Tables and code blocks are never split mid-element.
2. **Semantic refinement** — Within structural chunks, embed adjacent sentences and compute pairwise cosine similarity. Valleys in the similarity curve indicate topic shifts that the structural chunking missed. Split at valleys whose prominence exceeds 1 standard deviation below the local mean (using calibrated model statistics from Stage 7.1). This is the "embedding first, chunking second" principle (cf. Max-Min Semantic Chunking) applied as a refinement pass on structural boundaries.
3. **Size constraints** — No chunk exceeds a max token count (configurable, default 1024 tokens for complex documents, 512 for short-form content). Chunks below a minimum (default 32 tokens) are merged with neighbors. Empirical finding: chunk size 1024 optimized faithfulness and relevancy on complex documents (LlamaIndex eval on SEC filings). Context completeness matters more than precision for complex content.

**Semantic boundary detection (sentence-level valley detection):**

```
For sentences [s1, s2, s3, ...] within a structural chunk:
  1. Embed each sentence
  2. Compute sim(i) = cosine_similarity(embed(s_i), embed(s_{i+1}))
  3. Compute local_mean and local_stddev of sim values
  4. Where sim(i) < local_mean - stddev (i.e., prominent valley), insert a break
  5. Resulting segments become child chunks
```

This replaces the naive fixed-threshold approach. Valley prominence is relative to the document's own similarity distribution, not an absolute cutoff.

**Contextualized embeddings (v1 default with voyage-context-3).** Voyage's voyage-context-3 model processes the full document and generates per-chunk embeddings that capture both chunk-level detail and document-level context. This is late chunking done at the model level — no token-level embedding access needed. Each chunk embedding already "knows" its document context, which means:
- Parent-child alignment analysis becomes a **confirmation metric** rather than the primary signal for extraction gating
- High-alignment chunks genuinely indicate redundancy (the model already captured the relationship)
- Low-alignment chunks genuinely indicate novelty (even with full context, the content diverges)
- Less sensitivity to chunking strategy (2.06% variance vs 4.34% for standard embeddings)

**Fallback path: standard embeddings (OpenAI text-embedding-3-small).** If using non-contextualized embeddings, parent-child alignment is the primary signal, as originally designed. The landscape analysis pipeline handles both modes — contextualized embeddings just make the signals cleaner.

**v2 path: Local late chunking.** For fully local inference, jina-embeddings-v3 supports `late_chunking: true` via API or local deployment. Embeds the full document through the transformer first, then applies chunk boundaries to the token embedding sequence after self-attention. Each chunk embedding retains full document context. Lower quality than voyage-context-3 (23.66% gap per Voyage benchmarks) but zero API dependency.

**Chunk-Level Metadata Schema:**

```json
{
  "chunk_id": "docID_chunk004",
  "parent_document_id": "sha256_hash",
  "chunk_index": 4,
  "structural_hierarchy": "Title > Chapter 2 > Section 2.1",
  "contains_table": false,
  "contains_code": false,
  "token_count": 412,
  "heading_text": "Section 2.1: Implementation Details",
  "page_number": 7,
  "speaker": null
}
```

The `structural_hierarchy` field is critical — it captures the Markdown heading ancestry above the chunk. This enables pre-filtering: a query about "Chapter 2" can filter chunks by hierarchy metadata *before* running vector search, eliminating 90%+ of candidates.

**Parent-child context linking:**

Each chunk stores its `parent_chunk_id`. At retrieval time, when a sentence-level chunk matches, the system can walk up to retrieve the full paragraph or section for context (parent-child retrieval pattern).

**Chunk overlap (overlapping window):**

The chunker uses a configurable overlapping window to preserve boundary context:

| Env Var | Default | Description |
|---------|---------|-------------|
| `COVALENCE_CHUNK_SIZE` | `1000` | Maximum chunk size in bytes before paragraph splitting |
| `COVALENCE_CHUNK_OVERLAP` | `200` | Characters from the end of the previous paragraph prepended to the next |

When a section exceeds `COVALENCE_CHUNK_SIZE`, it is split into paragraph chunks at `\n\n` boundaries. Each paragraph chunk after the first receives the last `COVALENCE_CHUNK_OVERLAP` characters of the preceding paragraph as a prefix (separated by `\n\n`). The `ChunkOutput::context_prefix_len` field records how many leading bytes are overlap context, so consumers can trim them from snippets or highlighting.

Overlap resets at section boundaries — the first paragraph of a new section never carries context from the previous section. This prevents cross-section leaking where unrelated content bleeds through heading boundaries.

Section-level chunks never have overlap; only paragraph-level children produced by size-triggered splitting carry the overlap prefix.

### Stage 6: Embed + Contextual Prefix

Generate vector embeddings for all chunks, with contextual enrichment.

**Contextual prefix generation:**

Before embedding, each chunk receives a short LLM-generated prefix that summarizes the document-level context. This addresses the "orphan chunk" problem where an isolated chunk like "It was a massive failure" loses meaning without surrounding context.

```
Contextual prefix: "From Q3 Earnings Report (src:abc-123) by Jane Smith, Section: Financial Outlook. "
Chunk content: "Revenue increased 15% year-over-year, exceeding analyst expectations."
Embedded text: prefix + content
```

**Important:** The prefix must include the `source_id` (or a short hash fragment) to prevent false clustering when multiple sources share similar titles or metadata. Without a unique discriminator, chunks from different documents titled "Q3 Report" would cluster together in vector space.

The prefix is generated once per document (or per top-level section) and prepended to all child chunks before embedding. This is cheaper than embedding the full parent — one LLM call per section, not per chunk.

**Embedding operations:**
- Batch embedding calls to minimize API round-trips (configurable batch size via `COVALENCE_EMBED_BATCH`, default `64`)
- Use the configured embedding provider (OpenAI-compatible endpoint)
- The `OpenAiEmbedder` passes the configured `dimensions` parameter in each API request, so models like `text-embedding-3-large` truncate output to match the DB schema dimension (`COVALENCE_EMBED_DIM`, default `2048`). When dimensions are not supported by the provider, the parameter is omitted.
- Store embeddings in the `chunks.embedding` column
- The full normalized document text is also embedded and stored on the `Source` record directly (see below)

### Stage 7: Embedding Landscape Analysis

**This is the architectural core of the pipeline.** Embeddings are cheap (one API call per batch). LLM extraction is expensive (one call per chunk, structured output parsing, entity resolution). The landscape analysis stage examines the embedding topology to decide *how* each chunk should contribute to the graph — from pure embedding linkage for well-understood content to full LLM extraction for genuinely novel material.

**Inputs:** All chunks with embeddings from Stage 6, organized by parent-child hierarchy.

**Outputs:** A per-chunk `ExtractionMethod` classification that determines how Stage 8 processes each chunk.

**Principle:** Nothing is silently skipped. Every chunk contributes to the graph. The question is *how* — via embedding linkage (cheap, always-on) or via LLM extraction (expensive, targeted). No heuristics, no regex — embeddings and LLMs only.

#### 7.1: Threshold Calibration

Cosine similarity distributions vary dramatically between embedding models. OpenAI's text-embedding-3-small clusters in a narrow cone (typical range 0.65–0.95), while BGE-base uses more of the space (typical range 0.30–0.90). Hardcoded thresholds are model-dependent and will silently break on model changes.

**On first ingestion with a new model** (or periodically as a calibration check), compute the empirical distribution:

```rust
struct ModelCalibration {
    model_name: String,
    /// Percentile-based thresholds computed from sample
    parent_child_p25: f64,  // 25th percentile of parent-child similarities
    parent_child_p50: f64,  // median
    parent_child_p75: f64,  // 75th percentile
    adjacent_mean: f64,     // mean adjacent similarity
    adjacent_stddev: f64,   // stddev of adjacent similarity
    calibrated_at: DateTime<Utc>,
    sample_size: usize,
}

/// Calibrate thresholds from the first N documents processed with this model.
/// Requires at least 500 parent-child pairs for stable percentiles.
fn calibrate_model(pairs: &[(f64, f64)]) -> ModelCalibration {
    // Compute percentiles from actual parent-child similarity distribution
    // Use these as relative thresholds instead of hardcoded values
}
```

**Alignment classification uses calibrated percentiles, not absolute values:**

| Alignment | Threshold | Meaning |
|-----------|-----------|---------|
| **High** | > p75 of parent-child distribution | Top quartile — child faithfully represents parent topic |
| **Medium** | p25–p75 | Normal range — child adds specificity |
| **Low** | < p25 | Bottom quartile — child diverges significantly |
| **Misaligned** | < p25 - 1.5 × IQR | Statistical outlier — child is semantically unrelated |

Until calibration completes (cold start), use conservative defaults that send more chunks to LLM extraction rather than fewer. Better to over-extract on the first few documents than to silently miss facts.

**Cold start behavior (< 500 chunks):**
- All chunks get `FullExtraction` (no embedding linkage shortcuts — no graph to link to)
- Gleaning enabled by default (first pass often misses relationships when entity types are unknown)
- `known_entity_types` and `known_rel_types` in extraction prompt are empty → LLM discovers the schema
- Community detection skipped (too few nodes for meaningful communities)
- Adaptive query routing falls back to `balanced` strategy (no score distribution history)
- After 500+ chunks: calibration runs, extraction method distribution stabilizes, community detection begins

#### 7.2: Parent-Child Alignment Scoring

For every child chunk, compute cosine similarity to its parent:

```
alignment(child) = cosine_similarity(child.embedding, parent.embedding)
```

Classify using calibrated thresholds from 7.1. The alignment score determines the **extraction method**, not a binary extract/skip decision:

| Alignment | Extraction Method | Rationale |
|-----------|------------------|-----------|
| **High** | **Embedding linkage** — vector similarity to existing graph nodes creates `MENTIONED_IN` edges. No LLM. | The embedding already captures this content. Link it to what the graph knows. |
| **Medium** | **Delta check** — cheap LLM call (4o-mini): "Does this chunk contain entities or relationships not present in the parent?" If yes → full extraction. If no → embedding linkage. | Catches the "silent signal" problem — negations, new relationships, specific facts hiding in high-similarity text. |
| **Low** | **Full LLM extraction** + cross-document novelty check (7.4) | Divergent content likely contains novel entities/relationships worth capturing. |
| **Misaligned** | **Full LLM extraction** + structural review flag | May indicate a chunking boundary error or genuinely unrelated content. |

```rust
/// How each chunk will be processed in Stage 8.
/// No chunk is silently skipped — every chunk contributes to the graph.
enum ExtractionMethod {
    /// Vector similarity to existing nodes → MENTIONED_IN edges. No LLM call.
    EmbeddingLinkage,
    /// Cheap LLM delta check, then either full extraction or embedding linkage.
    DeltaCheck,
    /// Full structured LLM extraction (entities, relationships, coreferences).
    FullExtraction,
    /// Full extraction + flag for human/system review of structural placement.
    FullExtractionWithReview,
}

struct ChunkAnalysis {
    chunk_id: ChunkId,
    parent_alignment: f64,
    adjacent_similarity: Option<f64>,
    sibling_outlier_score: Option<f64>,
    graph_novelty: Option<f64>,       // from cross-document analysis (7.4)
    extraction_method: ExtractionMethod,
    flags: Vec<AnalysisFlag>,
}

enum AnalysisFlag {
    TopicShift,           // Adjacent similarity valley
    OrphanChunk,          // Low alignment + low self-coherence
    NovelContent,         // Low alignment + high self-coherence
    SemanticBoundaryError, // Misaligned, structural chunking may be wrong
    ClusterOutlier,       // Doesn't cluster with siblings
    GraphNovel,           // No close matches in existing graph
    GraphRedundant,       // Very close match to existing graph content
}
```

#### 7.3: Adjacent Chunk Similarity (Peaks and Valleys)

For same-level siblings (e.g., consecutive paragraphs within a section), compute pairwise cosine similarity:

```
sim(chunk_i, chunk_{i+1}) = cosine_similarity(chunk_i.embedding, chunk_{i+1}.embedding)
```

Plot this as a similarity curve across the document. The topology reveals:

- **Peaks** (high similarity between adjacent chunks) — Continuous topic. These chunks could potentially be merged for retrieval purposes without information loss.
- **Valleys** (low similarity between adjacent chunks) — Topic boundaries. If the structural chunking didn't already break here, the semantic signal suggests it should have.
- **Plateaus** (sustained high similarity) — Extended discussion of a single topic. Good candidate for a single article compilation.
- **Cliffs** (sudden drop from high to low) — Abrupt topic shift. The chunk after the cliff gets its extraction method upgraded (e.g., DeltaCheck → FullExtraction) because it introduces something new.

```rust
struct SimilarityCurve {
    /// Ordered list of (chunk_id, similarity_to_next) pairs
    points: Vec<(ChunkId, f64)>,
    /// Detected valleys (topic boundaries)
    valleys: Vec<ValleyPoint>,
    /// Detected plateaus (sustained topics)
    plateaus: Vec<PlateauRange>,
}

struct ValleyPoint {
    between: (ChunkId, ChunkId),
    similarity: f64,
    /// How much deeper this valley is than the local average.
    /// Uses calibrated stddev — prominence = (local_mean - valley) / calibrated_stddev
    prominence: f64,
}
```

**Valley prominence** is computed relative to calibrated statistics, not absolute values. A valley is prominent when it's more than 1 standard deviation below the local mean for this model.

#### 7.4: Cross-Document Novelty Analysis

After intra-document analysis, check each chunk against the **existing graph** to determine whether the content is genuinely novel to the system or redundant with what's already known.

```rust
struct GraphNoveltyResult {
    chunk_id: ChunkId,
    /// Closest existing node embedding distance
    nearest_node_distance: f64,
    /// Number of existing nodes within similarity threshold
    nearby_node_count: usize,
    /// Closest existing chunk embedding distance (from other sources)
    nearest_chunk_distance: f64,
    /// Is this chunk saying something the graph doesn't already know?
    novelty_classification: NoveltyClass,
}

enum NoveltyClass {
    /// No close matches in existing graph. Genuinely new content.
    Novel,
    /// Close to existing nodes but may add new relationships or properties.
    Augmenting,
    /// Very close to existing content from other sources. Confirms existing knowledge.
    Confirming,
    /// Near-duplicate of existing chunk from another source.
    Redundant,
}
```

**How novelty modifies extraction method:**

| Intra-doc Alignment | Graph Novelty | Final Extraction Method |
|---------------------|--------------|------------------------|
| High | Novel | **Upgrade to DeltaCheck** — the graph doesn't know this yet, even if the parent does |
| High | Confirming/Redundant | Embedding linkage (creates `CONFIRMS` edges cheaply) |
| Medium | Novel | **Upgrade to FullExtraction** — new to parent AND new to graph |
| Medium | Confirming | DeltaCheck (standard) |
| Low | Novel | FullExtraction (already was) |
| Low | Redundant | **Downgrade to DeltaCheck** — diverges from parent but graph already knows it |

This prevents two failure modes:
1. **Over-extraction:** Ingesting a source that covers well-trodden ground wastes LLM budget re-extracting known entities.
2. **Under-extraction:** A chunk that aligns with its parent but contains content novel to the broader graph gets linked rather than extracted, missing new knowledge.

**Implementation:** Cross-document analysis requires querying the existing node and chunk embeddings during ingestion. Use pgvector approximate nearest neighbor search with a generous `ef_search` for speed:

```sql
-- Find closest existing nodes to this chunk's embedding
SELECT id, canonical_name, 1 - (embedding <=> $1) as similarity
FROM nodes
WHERE embedding IS NOT NULL
ORDER BY embedding <=> $1
LIMIT 5;
```

This adds one PG query per chunk but avoids any LLM calls. For a 100-chunk document, that's 100 fast ANN queries vs potentially 60+ saved LLM calls.

#### 7.5: Cross-Level Cluster Analysis

Beyond pairwise comparisons, analyze how chunks cluster in embedding space:

1. **Sibling clustering** — Do all paragraphs under a section cluster together? If one paragraph is an outlier among its siblings, it's worth upgrading its extraction method.
2. **Cross-section similarity** — Do chunks from different sections cluster together? This suggests a latent topic that spans structural boundaries — a potential graph edge between sections.
3. **Document-level centroid distance** — How far is each chunk from the document centroid? Extreme outliers may be off-topic noise or highly specific content.

```
sibling_outlier_score(chunk) = 1 - cosine_similarity(chunk.embedding, mean(sibling_embeddings))
```

Chunks with high sibling outlier scores get their extraction method upgraded by one level (e.g., EmbeddingLinkage → DeltaCheck, DeltaCheck → FullExtraction).

#### 7.6: Building the Final Extraction Method Map

Combine all signals — parent alignment (7.2), valley/cliff detection (7.3), cross-document novelty (7.4), and sibling outlier analysis (7.5) — into the final per-chunk extraction method:

```rust
fn determine_extraction_method(
    alignment: AlignmentClass,  // from calibrated thresholds
    novelty: NoveltyClass,      // from cross-document analysis
    follows_cliff: bool,        // from adjacent similarity
    sibling_outlier: bool,      // from cluster analysis
) -> ExtractionMethod {
    // Start with alignment-based default
    let mut method = match alignment {
        AlignmentClass::High => ExtractionMethod::EmbeddingLinkage,
        AlignmentClass::Medium => ExtractionMethod::DeltaCheck,
        AlignmentClass::Low => ExtractionMethod::FullExtraction,
        AlignmentClass::Misaligned => ExtractionMethod::FullExtractionWithReview,
    };

    // Cross-document novelty can upgrade or downgrade
    method = match (method, novelty) {
        (EmbeddingLinkage, Novel) => DeltaCheck,           // Graph doesn't know this
        (DeltaCheck, Novel) => FullExtraction,              // Novel to both parent and graph
        (FullExtraction, Redundant) => DeltaCheck,          // Graph already knows this
        (m, _) => m,                                        // No change
    };

    // Topological signals only upgrade, never downgrade
    if follows_cliff || sibling_outlier {
        method = method.upgrade_one_level();
    }

    method
}
```

**No budget-based skipping.** Every chunk gets at least embedding linkage. If the LLM budget runs out mid-ingestion, chunks classified for DeltaCheck or FullExtraction are queued for the next batch consolidation pass — but they still get embedding linkage immediately. The graph is never hollow.

#### 7.7: Storing Landscape Metrics

The landscape analysis results are valuable beyond extraction gating. Store them as chunk metadata:

```json
{
  "parent_alignment": 0.42,
  "alignment_class": "low",
  "adjacent_similarity_left": 0.78,
  "adjacent_similarity_right": 0.31,
  "sibling_outlier_score": 0.67,
  "graph_novelty": "novel",
  "nearest_node_distance": 0.72,
  "extraction_method": "full_extraction",
  "flags": ["topic_shift", "novel_content", "graph_novel"],
  "follows_valley": true,
  "valley_prominence": 1.8
}
```

These metrics are useful for:
- **Search quality** — low-alignment chunks trigger parent context injection at retrieval time
- **Article compilation** — plateaus identify natural article boundaries
- **Debugging** — understanding why each chunk got a particular extraction method
- **Re-extraction** — if the extraction model improves, re-process FullExtraction chunks first
- **Model migration** — when the embedding model changes, recalibrate thresholds and re-run landscape analysis; chunks whose alignment class changed get re-processed

### Stage 8: Graduated Entity Extraction

Extraction is **graduated, not binary.** Every chunk contributes to the graph — the extraction method from Stage 7 determines how.

**Parallel extraction with bounded concurrency:**

LLM extraction calls for individual chunks run concurrently, bounded by a tokio `Semaphore` to prevent overwhelming the API. The concurrency limit is controlled by `COVALENCE_EXTRACT_CONCURRENCY` (default `8`). All chunk extractions for a source are dispatched in parallel (up to the semaphore limit), then the results are processed sequentially: entity resolution and edge creation happen one extraction at a time so that dedup decisions (`name_to_node` map, advisory locks) see a consistent view of prior results. This two-phase approach — concurrent LLM calls, sequential graph writes — maximizes throughput while preserving dedup safety.

**Extractor backend selection:**

The entity extractor backend is configured via `COVALENCE_ENTITY_EXTRACTOR`:

| Value | Backend | Description |
|-------|---------|-------------|
| `llm` (default) | `LlmExtractor` | OpenAI-compatible chat completions endpoint; uses the model from `COVALENCE_CHAT_MODEL` |
| `gliner2` | `GlinerExtractor` | GLiNER2 HTTP sidecar for fast, local NER without LLM API calls |

Both backends implement the same `Extractor` trait and produce identical `ExtractionResult` structs (entities + relationships). The GLiNER2 sidecar is configured via:

| Env Var | Default | Description |
|---------|---------|-------------|
| `COVALENCE_EXTRACT_URL` | `http://localhost:8432` | Base URL of the GLiNER2 sidecar |
| `COVALENCE_GLINER_THRESHOLD` | `0.5` | Minimum confidence threshold; entities below this are discarded by the sidecar |

The GLiNER2 backend extracts entities from a fixed label set (`person`, `organization`, `location`, `concept`, `event`, `technology`) and optionally returns relationships if the sidecar supports them. It does not generate entity descriptions (those come from the LLM path or from batch consolidation).

#### 8.1: Embedding Linkage (All Chunks)

Every chunk, regardless of extraction method, gets embedding linkage. This is the baseline that ensures the graph is never hollow:

```sql
-- For each chunk, find the closest existing graph nodes
SELECT n.id, n.canonical_name, 1 - (n.embedding <=> $1) as similarity
FROM nodes n
WHERE n.embedding IS NOT NULL
  AND 1 - (n.embedding <=> $1) > $calibrated_linkage_threshold
ORDER BY n.embedding <=> $1
LIMIT 10;
```

For each match above the linkage threshold, create a `MENTIONED_IN` edge:
```
(Node: "Company X") -[MENTIONED_IN {confidence: similarity_score}]-> (Chunk)
```

This captures the "consensus" layer — what the graph already knows about, referenced in this chunk. Pure vector math, no LLM, no heuristics, handles pronouns and paraphrases because the embedding encodes them.

**Cost:** One ANN query per chunk. For a 100-chunk document: ~100ms total.

#### 8.2: Delta Check (Medium-Alignment + Novel High-Alignment Chunks)

A cheap LLM call to determine if full extraction is warranted. The prompt is structured to leverage the landscape analysis context:

```
Model: gpt-4o-mini (or equivalent cheap/fast model)
Response format: json_object

System: You are a semantic delta detector for a knowledge graph ingestion 
pipeline. You receive structured JSON describing a child text chunk and 
its parent context. Determine if the child contains entities, 
relationships, or specific facts NOT present in or inferable from the 
parent.

Focus on: named entities not in parent, specific relationships absent 
from parent, negations or contradictions, quantitative facts (dates, 
numbers, metrics), and coreferences that resolve differently than 
parent context suggests.

User:
{
  "parent_context": "{parent_chunk.content}",
  "child_text": "{chunk.content}",
  "structural_location": "{chunk.metadata.structural_hierarchy}",
  "embedding_analysis": {
    "parent_child_alignment": {analysis.parent_alignment},
    "alignment_class": "{analysis.alignment_class}",
    "graph_novelty": "{analysis.graph_novelty}",
    "flags": {analysis.flags}
  }
}

Expected response schema:
{
  "has_novel_facts": true | false,
  "novel_elements": [
    {
      "type": "entity | relationship | fact | negation | quantity",
      "description": "brief description of what's novel"
    }
  ],
  "reasoning": "one sentence explaining your decision"
}
```

- If `has_novel_facts: true` → upgrade to full extraction for this chunk, passing `novel_elements` as extraction hints
- If `has_novel_facts: false` → embedding linkage is sufficient

**Cost:** ~200-400 tokens per call. ~10x cheaper than full extraction.

This catches the "silent signal" problem Gemini identified — negations, new relationships, and specific facts hiding inside high-similarity text — without paying full extraction cost for every chunk.

#### 8.3: Full LLM Extraction (Low-Alignment + Upgraded Chunks)

Full structured extraction for chunks where the landscape analysis identified genuine novelty. The prompt is designed to leverage all available context — structural location, parent content, landscape analysis flags, delta check hints (if available), and embedding-derived entity type hints.

```
Model: gpt-4o (or equivalent capable model)
Response format: json_object

System: You are a precise entity and relationship extractor for a hybrid 
knowledge graph. Extract ONLY what is clearly supported by the text. 
Do not infer or hallucinate entities/relationships not present.

Your extractions build graph nodes and edges. Quality and precision 
matter more than recall — missed entities can be caught in later 
passes, but false entities corrupt the graph.

Rules:
- Use consistent canonical names across entities and relationships.
- If an entity matches one from nearby_graph_nodes, set 
  is_existing_match: true and use the graph's canonical_name.
- Relationship rel_type should be UPPER_SNAKE_CASE verbs.
- Confidence: 0.9+ explicitly stated, 0.7-0.9 strongly implied, 
  0.5-0.7 inferred. Below 0.5: do not extract.
- causal_level: "association" (co-occurrence), "intervention" (causal),
  "counterfactual" (hypothetical). Default "association" when uncertain.

User:
{
  "source_text": "{chunk.content}",
  "parent_context": "{parent_chunk.content}",
  "structural_location": "{chunk.metadata.structural_hierarchy}",
  "source_metadata": {
    "title": "{source.title}",
    "source_type": "{source.source_type}"
  },
  "extraction_context": {
    "flags": {analysis.flags},
    "parent_alignment": {analysis.parent_alignment},
    "alignment_class": "{analysis.alignment_class}",
    "graph_novelty": "{analysis.graph_novelty}",
    "delta_check_hints": {delta_check.novel_elements | null}
  },
  "nearby_graph_nodes": [
    {
      "canonical_name": "{node.canonical_name}",
      "node_type": "{node.node_type}",
      "description": "{node.description}"
    }
  ],
  "known_entity_types": ["{distinct node_type values from graph, e.g. person, organization, technology, ...}"],
  "known_rel_types": ["{distinct rel_type values from graph, e.g. WORKS_AT, CREATED_BY, ...}"]
}

Expected response schema:
{
  "entities": [
    {
      "name": "entity name as it appears in text",
      "canonical_name": "preferred name (use existing graph name if match)",
      "entity_type": "lowercase category (common: person, organization, location, concept, event, technology, metric, temporal; use existing graph types when possible, create new types sparingly)",
      "description": "brief factual description grounded in the text",
      "properties": {},
      "confidence": 0.0-1.0,
      "is_existing_match": true | false
    }
  ],
  "relationships": [
    {
      "source_name": "source entity canonical name",
      "target_name": "target entity canonical name",
      "rel_type": "UPPER_SNAKE_CASE verb",
      "description": "brief description grounded in text",
      "properties": {},
      "confidence": 0.0-1.0,
      "causal_level": "association | intervention | counterfactual"
    }
  ],
  "coreferences": [
    {
      "mentions": ["he", "the CEO", "Tim Cook"],
      "resolved_entity": "Tim Cook",
      "confidence": 0.0-1.0
    }
  ]
}
```

**Key design decisions in this prompt:**

1. **Nearby graph context is injected.** Embedding linkage (7.1) already found the closest existing nodes. Passing those names to the extractor enables entity resolution *during* extraction rather than as a separate post-processing step. This is the "Entity Resolution via Vectors" approach from the Gemini conversation.

2. **Delta check hints are forwarded.** If the chunk went through a delta check first, the novel elements identified there focus the extractor on what's actually new, reducing hallucination.

3. **Causal level is extracted per-relationship.** This feeds directly into Pearl's causal hierarchy (L0/L1/L2) used by the epistemic model. Most extractors ignore this — we're capturing it at source.

4. **`is_existing_match` flag** enables the entity resolver to short-circuit. If the LLM already identified a match to an existing graph node, the resolver can skip fuzzy matching and go straight to confirmation.

5. **Confidence floor at 0.5.** Below that, don't extract — let a future pass with better context catch it rather than polluting the graph with low-confidence noise.

**Extraction strategy:**

1. **Flag-aware prompting** — The extraction prompt includes *why* the chunk was flagged (topic shift, novel content, sibling outlier, graph novel). This guides the LLM to focus on what's actually anomalous.
2. **Batch by section** — Process all flagged chunks within a section together for better co-reference resolution.
3. **Confidence scoring** — Confidence is per-extraction, not per-chunk. Individual entities within the same chunk may have different confidence levels.
4. **Dedup within source** — Merge duplicate entities extracted from different chunks of the same source.
5. **Clearance inheritance** — Extracted nodes/edges inherit the `clearance_level` of their source. See [09-federation](09-federation.md).
6. **Cross-document confirmation** — When full extraction produces entities that match existing graph nodes (from embedding linkage in 7.1), create `CONFIRMS` edges instead of duplicate nodes. This strengthens existing knowledge rather than duplicating it.

**Gleaning (multi-pass extraction):**

For chunks classified as `FullExtractionWithReview` or any chunk where the initial extraction returns zero entities despite high novelty scores, perform up to 1 gleaning pass (configurable, default `max_gleanings: 1`). This follows the Microsoft GraphRAG pattern:

1. After initial extraction, ask the LLM: "Many entities and relationships were missed in the previous extraction. Please identify any additional entities and relationships not already listed."
2. Pass the previously extracted entities as context so the LLM focuses on what's missing.
3. Merge gleaning results with initial extraction (deduplicate by canonical_name).
4. **Cost guard:** Gleaning doubles extraction cost per chunk. Only trigger for chunks where `analysis.graph_novelty == "Novel"` AND initial extraction yielded fewer entities than expected (heuristic: < 2 entities for a chunk > 200 tokens).

```json
{
  "system": "You are reviewing a previous entity extraction for completeness. Many entities and relationships were missed. Identify additional entities and relationships not in the already_extracted list.",
  "user": {
    "source_text": "{chunk.content}",
    "already_extracted": {
      "entities": ["{previous_extraction.entities}"],
      "relationships": ["{previous_extraction.relationships}"]
    },
    "parent_context": "{parent_chunk.content}"
  }
}
```

### Stage 8.5: Node Embedding

After all entities for a source have been extracted and resolved, the pipeline batch-embeds their descriptions so that nodes are searchable via vector similarity. Each entity is embedded using the text `"{canonical_name}: {description}"` (or just the canonical name when no description is available). The resulting vectors are stored in `nodes.embedding`.

Node embeddings use a separate dimension configured by `COVALENCE_NODE_EMBED_DIM` (default `256`), which is typically smaller than the chunk embedding dimension to save storage. The same `OpenAiEmbedder` is used, with the `dimensions` parameter in the API request controlling truncation.

### Stage 8.6: Type Normalization (Emergent Ontology)

Before entity resolution, normalize extracted entity types and relationship types against the graph's emergent schema. This prevents drift without requiring a formal ontology.

**The problem:** Without normalization, the LLM produces `person`, `Person`, `individual`, `human`, `researcher` as separate entity types. Similarly, `WORKS_AT`, `employed_by`, `works_for` proliferate as distinct relationship types. This fragments the graph — community detection, structural search, and type-based queries all degrade.

**The approach: Extract-Define-Canonicalize** (EDC pattern, arXiv:2404.03868):

1. **Extract** — Already done in Stage 8.3. The LLM produced entity types and relationship types.
2. **Canonicalize** — For each extracted type:
   a. Embed the type string (e.g., embed "researcher")
   b. Search existing type embeddings: `SELECT type_name, embedding FROM type_registry WHERE embedding <=> $type_embedding < 0.15`
   c. If match found (cosine < 0.15): rewrite to the canonical type. "researcher" → "person"
   d. If no match: this is a genuinely new type. Add to registry.

**Type registry** (lightweight, in-memory + persisted):

```sql
CREATE TABLE type_registry (
    id SERIAL PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('entity', 'relationship')),
    canonical_name TEXT NOT NULL,
    embedding halfvec(2048) NOT NULL,
    aliases TEXT[] NOT NULL DEFAULT '{}',
    instance_count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_type_registry_canonical ON type_registry(kind, canonical_name);
CREATE INDEX idx_type_registry_embedding ON type_registry
    USING hnsw (embedding halfvec_cosine_ops);
```

**Merge decisions:**
- Cosine < 0.1: auto-merge (clearly same type). Add as alias.
- Cosine 0.1–0.15: merge if instance_count of existing type > 10× the new type's count. Otherwise flag for review.
- Cosine > 0.15: new type. Register it.

**Periodic consolidation (daily):**
- Cluster all types by embedding (DBSCAN, eps=0.12)
- Within each cluster, elect the type with the highest `instance_count` as canonical
- Rewrite all nodes/edges using non-canonical types
- This catches drift that accumulates between normalization runs

**Why this avoids the deep end:**
- No formal ontology language (no OWL, no SHACL, no inference rules)
- No predefined schema — the schema emerges from the data
- No human ontology engineering — the system self-organizes
- Types consolidate naturally as the graph grows (more data → more stable types)
- But it still prevents the chaos of completely uncontrolled types

### Stage 9: Entity Resolution + Store

Match extracted entities against existing nodes in the graph. Uses a three-phase approach (cf. Shereshevsky 2025) that leverages embedding linkage from Stage 8.1 as a natural blocking mechanism:

**Phase 1: Blocking via Embedding Similarity (already done)**

Embedding linkage (Stage 8.1) already identified the top-k nearest existing nodes for each chunk. The `is_existing_match` flag from the extraction prompt (Stage 8.3) further narrows candidates. This replaces traditional blocking strategies and naturally avoids O(n²) pairwise comparisons.

**Phase 2: 4-Tier Pairwise Resolution (`PgResolver`)**

For each extracted entity, the `PgResolver` attempts resolution in strict order, stopping at the first match:

1. **Exact canonical name** — Case-insensitive lookup on `nodes.canonical_name`. Fastest path.
2. **Alias match** — Case-insensitive lookup in `node_aliases`. Returns the aliased node's canonical name. On a fuzzy match (tier 4), an alias is automatically created so future lookups short-circuit to this tier.
3. **Vector cosine similarity** — Embed the entity name via the configured embedder, query the closest node by `embedding <=> $1::halfvec`. Accept the match only when similarity meets or exceeds `COVALENCE_RESOLVE_VECTOR_THRESHOLD` (default `0.85`). This step requires an embedder; when none is configured it is gracefully skipped.
4. **Fuzzy trigram** — `pg_trgm` `similarity(canonical_name, $1)` against all nodes, preferring nodes whose `node_type` matches the extracted entity type. Minimum threshold: `COVALENCE_RESOLVE_TRIGRAM_THRESHOLD` (default `0.4`).

If all four tiers miss, the entity is classified as `MatchType::New` and a new node is created.

| Env Var | Default | Description |
|---------|---------|-------------|
| `COVALENCE_RESOLVE_VECTOR_THRESHOLD` | `0.85` | Minimum cosine similarity for vector-based entity matching |
| `COVALENCE_RESOLVE_TRIGRAM_THRESHOLD` | `0.4` | Minimum trigram similarity for fuzzy entity matching |

**Relationship Type Resolution:**

`PgResolver` also resolves relationship type labels via `resolve_rel_type()` so that synonymous edge types converge on the canonical (most frequently used) form:

1. **Exact match** — If the normalized `rel_type` already exists among edges (case-insensitive), return the stored spelling.
2. **Fuzzy trigram** — Find the closest existing `rel_type` by `pg_trgm` similarity above the configured threshold, weighted by usage frequency. Return the canonical form.
3. **No match** — Return the input as-is (it is a genuinely new relationship type).

A companion `normalize_rel_type()` helper applies lowercase, unifies separators (spaces and hyphens become underscores), collapses multiple underscores, and strips semantically empty prefixes (`is_`, `has_`, `was_`). This normalization runs before both exact and fuzzy lookups so that `"is_author_of"`, `"authored-by"`, and `"author of"` converge.

**Phase 3: Transitive Closure**

After pairwise resolution, check for transitive chains: if entity A matches node X, and entity B also matches node X, then A and B should be merged even if they don't directly match each other. This catches cases like "Tim Cook", "Apple CEO", and "Cook" all resolving to the same node.

**Concurrency safety:** Entity resolution must be serialized per canonical name to prevent parallel workers from creating duplicate nodes. Use PostgreSQL advisory locks:

```rust
let lock_key = hash_to_i64(&canonical_name);
sqlx::query!("SELECT pg_advisory_xact_lock($1)", lock_key).execute(&mut tx).await?;
// Perform resolution within this transaction
```

**Resolution thresholds (calibrated per embedding model):**

| Signal | Match | Maybe | Miss |
|--------|-------|-------|------|
| LLM `is_existing_match` | Direct merge | - | - |
| Exact name | > 0.95 | - | - |
| Trigram | > 0.8 | 0.6–0.8 | < 0.6 |
| Vector cosine | > 0.9 | 0.7–0.9 | < 0.7 |
| Graph context (neighborhood centroid) | > 0.8 | 0.5–0.8 | < 0.5 |

- **Match** → Merge into existing node:
  - Update `last_seen`, increment `mention_count`, add alias if new name variant
  - **Description evolution:** When new extraction provides description content not present in the existing description, consolidate via LLM:
    ```json
    {
      "system": "Merge these descriptions of the same entity into a single comprehensive description. Keep all unique facts. Resolve contradictions by preferring the more recent or more specific information. Maximum 3 sentences.",
      "user": {
        "entity_name": "{node.canonical_name}",
        "existing_description": "{node.description}",
        "new_description": "{extracted.description}",
        "new_source_date": "{source.created_date}"
      }
    }
    ```
  - **Embedding update:** Re-embed the consolidated description. This is critical — the node's embedding is its vector identity. Stale embeddings mean the node drifts out of relevant search results.
  - **Cost guard:** Only trigger description consolidation when the new description has low similarity to the existing one (cosine < 0.85). If the new info is redundant (cosine ≥ 0.85), just update `last_seen` and skip the LLM call.
- **Maybe** → Use graph context to disambiguate. If graph context is also "Maybe", flag for review.
- **Miss** → Create new node

**Storage:**
1. Upsert nodes (new or merged)
2. Insert edges
3. Insert extraction provenance records (with `chunk_id` or `statement_id` depending on pipeline)
4. Update graph sidecar (incremental sync via outbox)

**Future enhancement — HDBSCAN catch-all:** The Gemini architecture conversation proposed a 5th resolution tier: HDBSCAN clustering of all unresolved entities to dynamically discover entity clusters for human curation. The current 4-tier PgResolver handles the common cases well, but HDBSCAN would catch remaining duplicates where no individual matching signal is strong enough but the combination of weak signals across a cluster reveals common identity. This is a v2 enhancement — requires accumulating enough unresolved entities to make clustering meaningful.

---

## Source Update Classes

Sources don't all update the same way. The system recognizes four classes of updates, each with distinct handling:

### Class 1: Append-Only (Conversations, Logs, Event Streams)

Past data is still valid; the timeline is extending.

**Detection:** Same URI, content starts with the same bytes as existing content but has additional content appended.

**Handling:**
- Hash and chunk only the **delta** (new messages/entries)
- Extract entities/relationships from new chunks only
- Link new chunks to existing ones via sequence:
  ```
  (Chunk_99) -[APPENDED_AFTER]-> (Chunk_98)
  ```
- The source record's `content_hash` updates; `metadata.content_version` increments
- Existing extractions remain untouched

**Examples:** Slack threads, GitHub issues, log files, ongoing meeting transcripts

### Class 2: Mutating & Versioned (Code, Documentation, Specs)

Content evolves; old versions don't become false, they become historical.

**Detection:** Same URI, different content hash, explicit version indicator or content diff shows mutations (not just appends).

**Handling:**
- Ingest as a **new source record** (new `document_id`)
- Full pipeline: parse, chunk, embed, extract
- Link to predecessor via structural edge:
  ```
  (Source_v2) -[SUPERSEDES]-> (Source_v1)
  ```
- `SUPERSEDES` edges also created between entity-level claims where applicable
- Old source's extractions receive an epistemic penalty (reduced confidence, not deleted)
- **Traversal rule:** Default graph traversal follows `SUPERSEDES` edges to the terminus before answering current-state questions

**Examples:** API documentation versions, code file updates, spec revisions, wiki page edits

### Class 3: Explicit Retractions & Corrections

A source doesn't just update — it explicitly states prior information was wrong.

**Detection:** New source references a prior claim and contradicts it, or metadata includes explicit retraction/correction markers.

**Handling:**
- Extract the corrected claim as a new entity
- Create an adversarial edge:
  ```
  (Claim_New: "The API uses GraphQL") -[CORRECTS]-> (Claim_Old: "The API uses REST")
  ```
- `CORRECTS` is stronger than `SUPERSEDES` — it immediately zeros out the epistemic confidence of the old claim
- The old claim persists in the graph (queryable for historical context) but is suppressed in current-state queries

**Examples:** Errata, CVE corrections, journalistic corrections, federated peer corrections

### Class 4: Structural Refactoring (Container Changes, Content Unchanged)

The knowledge doesn't change, but its container does (e.g., a large doc split into multiple files).

**Detection:** Incoming chunks have SHA-256 hashes matching existing chunks, but different `source_uri` or `structural_hierarchy`.

**Handling:**
- **Do not re-extract.** The graph topology is already correct.
- Update chunk-level metadata: `source_uri`, `structural_hierarchy`, page numbers
- Update provenance pointers to route to new file locations
- No new nodes, edges, or extractions created

**Examples:** Documentation restructuring, monolith → microservice repo splits, wiki reorganization

### Class 5: Takedown / Deletion (GDPR, User Deletion)

The source is permanently removed — not updated, not superseded, but deleted.

**Detection:** Explicit API call (`DELETE /sources/:id`) or GDPR data subject request.

**Handling:**
- Mark source as deleted (soft-delete, retain tombstone for audit)
- Execute **TMS Cascade**: locate all nodes and edges whose *sole* provenance was the deleted source
  - Sole-provenance entities: delete or mark inactive (clearance_level = -1)
  - Multi-provenance entities: decrement `mention_count`, remove extraction links to deleted source, recompute confidence
- **Orphan detection** — After cascade, identify nodes with zero edges (degree-0 in the graph sidecar). Nodes that are sole-provenance AND degree-0 are unreachable and should be archived. Multi-provenance degree-0 nodes are flagged for review (they may be legitimate isolates or a sign of incomplete extraction). Run this check as a post-cascade step, not a separate batch job — Graphiti's issue #1083 demonstrated that deferred orphan cleanup leads to unbounded graph growth.
- Chunks from the deleted source are purged (hard delete for GDPR compliance)
- Log the takedown action in `audit_logs` with full rationale
- Trigger epistemic delta recomputation for affected topic clusters

**Examples:** GDPR right-to-erasure requests, user deleting personal notes, retraction of a source due to legal action

### Epistemic Delta Threshold

Not every update warrants re-synthesis of derived articles/summaries. The system tracks an **epistemic delta** — the degree to which new or updated claims shift the coherence of a topic cluster.

```
epistemic_delta = Σ |confidence_change(claim)| for all affected claims in the topic cluster
```

- If `epistemic_delta > threshold` (default: 0.10 = 10% shift): trigger LLM re-synthesis of the topic's materialized article
- If below threshold: new facts sit in the atomic graph, incorporated at next scheduled synthesis or on-demand query

This prevents expensive LLM re-writes for trivial changes (typo fixes, minor addenda) while ensuring significant knowledge shifts are reflected promptly.

---

## Three-Timescale Consolidation

Ingestion is the online (fast) tier of a three-timescale consolidation pipeline modeled on hippocampal-neocortical memory consolidation (Complementary Learning Systems theory). See [01-architecture](01-architecture.md#layer-responsibilities) for the full picture.

### Online Tier (This Pipeline — Seconds)

Per-source processing: parse → chunk → embed → **landscape analysis** → targeted extract → resolve → store. The landscape analysis stage determines extraction priority per chunk. Produces raw chunks and graph elements. Updates confirms/contradicts/originates edges. Incremental confidence updates via Dempster-Shafer fusion and Subjective Logic.

### Batch Tier (Hours)

Groups sources by topic cluster. LLM-driven compilation synthesizes multiple sources into **articles** — right-sized summaries (200–4000 tokens) that serve as the primary retrieval unit. Applies Bayesian confidence aggregation across compiled articles. Detects and queues contentions. Triggered by timer or when epistemic delta exceeds threshold.

### Deep Tier (Daily+)

Structural maintenance: TrustRank global recalibration, community detection refresh, domain topology map update, landmark article identification. Bayesian Model Reduction for principled forgetting (see [07-epistemic-model](07-epistemic-model.md#forgetting-as-bayesian-model-reduction)). Cross-domain generalization discovery.

---

## Error Handling

- Each stage is independently retriable
- Failed chunks are marked in metadata and can be re-processed
- LLM extraction failures (rate limits, malformed output) → retry with exponential backoff, then skip and mark
- The pipeline should be resumable: if it crashes mid-source, it can pick up from the last completed stage

## Performance Considerations

- **Embedding batching** — Group chunks into batches of 100+ for API efficiency
- **Parallel extraction** — Multiple chunks can be sent to the LLM concurrently (respect rate limits)
- **Lazy sentence-level chunking** — Only decompose to sentence level for chunks above a size threshold
- **Skip extraction for low-information chunks** — Chunks with very low token count or very high similarity to parent may not need LLM extraction
- **Delta-only processing for append-only sources** — Hash comparison to avoid re-processing unchanged content

## Code Source Routing

When `source_type = "code"`, the ingestion pipeline routes to the AST-aware code pipeline (see [12-code-ingestion](12-code-ingestion.md)) instead of the statement or chunk pipeline. See the [Pipeline Architecture](#pipeline-architecture) section above for the routing diagram.

## Open Questions

- [x] Extraction model → General-purpose LLM for v1 (flexibility for open-domain KG). GLiNER (NAACL 2024) matches GPT-4o at fraction of cost — use for v2 production volume optimization. Hybrid: LLM for schema discovery, fine-tuned model for bulk extraction.
- [x] Multi-lingual sources → BGE-M3 supports 100+ languages in unified vector space (ACL Findings 2024). For v1, English-only with OpenAI text-embedding-3-small. Switch to BGE-M3 (1024 dims) when multi-lingual needed.
- [x] Streaming vs batch → Micro-batch (per-source batch). Buffer per-source, process as batch, merge into graph. Pure streaming makes entity resolution harder (iText2KG, arxiv 2409.03284).
- [x] Default embedding model → Voyage voyage-context-3 (2048 dims, Matryoshka to 512). Contextualized chunk embeddings: each chunk embedding captures both chunk-level detail and full document context. Outperforms OpenAI text-embedding-3-large by 14.24% on chunk-level retrieval. First 200M tokens free ($0.18/M thereafter). Drop-in API, no special infrastructure. Reduces (but doesn't eliminate) parent-child alignment problem — landscape analysis still valuable for cross-document novelty, sibling outlier detection, and extraction gating. OpenAI text-embedding-3-small as fallback ($0.02/M tokens, no contextualization).
- [x] Tabular data → For v1: convert tables to Markdown pipe format, extract entities normally. For v2: RML/YARRRML mappings (W3C standard) for structured relational semantics. Primary key → subject, foreign key → object property, column → predicate.
- [x] Ontology hints → Defer. Not needed for v1. Dynamic ontology via community detection is the design principle.
- [x] Contextual prefix scope → Per top-level section. 50-100 token prefix optimal (Anthropic Contextual Retrieval, Sept 2024). Include source_id in prefix to prevent false clustering. Cost: $1.02/M doc tokens with prompt caching.
- [x] Delta detection → Chunk-hash comparison (SHA-256 content_hash). More robust against whitespace/formatting changes.
- [x] Articles per cluster → Multiple when cluster is large. One "overview" article + specialized sub-articles for distinct sub-topics. Guided by Louvain hierarchy.
- [x] Compilation trigger → Both timer AND epistemic delta. Already specified.
- [x] Blanket vs targeted extraction → **Graduated.** No chunk is silently skipped. Every chunk gets at minimum embedding linkage (MENTIONED_IN edges via vector similarity to existing nodes). Medium-alignment chunks get a cheap delta check (4o-mini). Low-alignment and novel chunks get full LLM extraction. Lesson learned from prior claims-extraction approach: 29K flat extracted claims from blanket extraction produced noise, not signal. But also: silently skipping chunks produces a hollow graph where consensus lives only in vectors.
- [x] Landscape metric storage → Store in chunk metadata JSONB (parent_alignment, alignment_class, adjacent_similarity, sibling_outlier_score, graph_novelty, extraction_method, flags). Cheap to compute, valuable for debugging and re-extraction.
- [x] Extraction budget → If LLM budget exhausts mid-ingestion, chunks needing DeltaCheck or FullExtraction are queued for next batch consolidation pass. They still get embedding linkage immediately. Graph is never hollow.
- [x] Threshold calibration → Percentile-based thresholds from empirical parent-child similarity distribution. Requires 500+ pairs for stable calibration. Conservative defaults (more extraction) during cold start. Stored in model_calibrations table.
- [x] Cross-document novelty → Checked during landscape analysis via pgvector ANN queries against existing node/chunk embeddings. Novel chunks upgrade extraction method; redundant chunks downgrade. One ANN query per chunk (~1ms each).
- [x] Statement vs chunk pipeline → Statement pipeline is now default (ADR-0015). Two-pass design: statement extraction with coref → entity extraction from statements. Chunk pipeline retained for backward compatibility.
- [x] HDBSCAN entity resolution → Deferred to v2. 4-tier PgResolver (exact, alias, vector, trigram) handles common cases. HDBSCAN catch-all for dynamic clustering of remaining unresolved entities is a future enhancement.
- [x] Offset Projection Ledger → Future optimization. Current LLM-in-prompt coref resolution works well. Ledger becomes valuable if dedicated coref model (fastcoref) is added as preprocessing step.
- [x] Single-pass vs two-pass extraction → Two-pass. Statement extraction (Pass 1) and entity extraction (Pass 2) are separate LLM calls. Single-pass attention dilution is a known failure mode (Gemini architecture conversation).
