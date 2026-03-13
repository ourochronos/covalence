# Design: Ingestion Pipeline

## Status: implemented (core + two-pass + URL + sidecar), partial (PII, landscape)

> **Updated 2026-03-10 (+post2)**: Massive engineering wave closed 47/50 GitHub issues. Key
> additions: table linearization (#45), byte-offset chunking (#30), URL ingestion (#28),
> SidecarExtractor (#44), extraction error logging (#49), two-pass extraction (#32), Voyage
> embedding provider switching (#46), landscape bypass for single-chunk sources (#43), full local
> model pipeline tested. GLiNER2 windowing **now implemented** in `SidecarExtractor`. Additional
> post-wave closures: #51 (coref now independent stage), #52 (converter windowing + PDF
> placeholder), #57 (batch extraction token thresholds), #58 (full stage configurability).
> Bug #59 (dotenvy ordering, fixed locally). **Landscape disabled** (#60,
> `COVALENCE_LANDSCAPE_ENABLED=false`) — gating was too aggressive. Two-pass extraction
> confirmed: GLiNER (local) for entities + Gemini Flash for relationships. Pipeline **must run
> from project root** (dotenvy reads `.env` from CWD).

## Spec Sections: 05-ingestion.md, 02-data-model.md

## Architecture Overview

The ingestion pipeline transforms raw content into a knowledge graph through 8 stages: accept → convert → parse → normalize → chunk → embed → extract → resolve. Each stage is modular with trait-based abstractions allowing provider swaps (OpenAI ↔ Voyage embeddings, LLM ↔ GLiNER extraction).

## Pipeline Stages

### Stage 1: Accept & Dedup
- **File**: `ingestion/accept.rs`
- **Status**: ✅ Implemented
- Content hash (SHA-256) computed, checked against existing sources
- Duplicate detection prevents re-ingestion of identical content
- Supersession support: new version of existing source triggers update flow
- **NEW (#28)**: API now accepts URLs directly — fetches content then ingests; metadata extracted from HTTP headers

### Stage 1.5: Convert
- **File**: `ingestion/converter.rs`
- **Status**: ✅ Implemented (substantially improved)
- `ConverterRegistry` with pluggable converters: Markdown, PlainText, HTML, PDF
- **NEW (#45)**: Table linearization implemented in pure Rust — converts Markdown pipe tables to
  natural-language sentences (e.g., `| A | B |` → "A is B"). No ML model needed.
- **NEW**: PDF conversion tested via `pymupdf4llm` — 3.4s for a 15-page paper, no model inference
  required. Accurate structure preservation including tables and headings.
- **NEW (#52)**: Converter windowing for large HTML — `HtmlConverter` (ReaderLM-v2 path) now
  splits documents that exceed the ReaderLM context limit into overlapping windows, converts each
  independently, then rejoins. Prevents silent truncation of large HTML pages.
- **NOTE (#52)**: `PdfConverter` currently exists as a **placeholder** — the `pymupdf4llm` Python
  integration is wired at the trait level but the implementation returns a stub pending a full
  sidecar integration. Direct Rust-callable PDF extraction is the planned path.
- All content normalized to markdown for downstream processing

### Stage 2-3: Parse & Normalize
- **File**: `ingestion/normalize.rs`
- **Status**: ✅ Implemented
- Unicode normalization, whitespace cleanup
- **NEW (#30)**: `normalized_content` and `normalized_hash` now stored on the `sources` table,
  enabling hash-based change detection for incremental re-ingestion (only re-chunk if content
  actually changed)
- Text normalization for consistent entity matching

### Stage 4: Chunk
- **File**: `ingestion/chunker.rs`
- **Status**: ✅ Fully Implemented (including byte offsets)
- Hierarchical chunking: respects heading boundaries, then paragraph, then sentence
- Configurable: `COVALENCE_CHUNK_SIZE=1000`, `COVALENCE_CHUNK_OVERLAP=200`
- UTF-8 safe (multi-byte boundary handling, fixed in #29)
- **NEW (#30)**: `byte_start`, `byte_end`, and `content_offset` tracked on each chunk — enables
  precise source attribution and incremental re-ingestion. Hash-based change detection: chunks are
  only re-embedded/re-extracted if their content hash changes.

### Stage 5: Embed Chunks
- **File**: `ingestion/embedder.rs`, `openai_embedder.rs`, `voyage.rs`
- **Status**: ✅ Implemented
- Matryoshka truncation: `truncate_embedding()` for per-table dimension tiering
- **UPDATED**: Voyage `voyage-3-large` now the active default provider (switched from OpenAI).
  Cost: $0.01/M tokens vs $0.13/M for OpenAI.
- Batch embedding with retry and partial failure resilience (#27)

### Stage 5.5: Embedding Landscape Analysis
- **File**: `ingestion/landscape.rs`
- **Status**: 🟡 Wired but currently **DISABLED** (`COVALENCE_LANDSCAPE_ENABLED=false`)
- Computes cosine similarity distribution across chunk embeddings
- Detects topic diversity, embedding quality anomalies; determines extraction method per chunk
- Model calibration metrics (parent-child alignment, adjacent similarity, sibling outlier score)
- **#43**: Single-chunk sources bypass landscape analysis entirely and always force
  `FullExtraction` — avoids spurious `EmbeddingLinkage` decisions when the chunk IS the entire
  document and parent-child alignment would be meaninglessly high.
- Results stored on chunks: `extraction_method` and `landscape_metrics` JSONB fields in the DB.
- Chunks classified as `EmbeddingLinkage` (high parent alignment → redundant content) or
  `DeltaCheck` (moderate alignment) skip full entity/relationship extraction; only
  `FullExtraction` and `FullExtractionWithReview` chunks are sent to the extractor.
- **⚠️ #60 BUG (disabled)**: Landscape gating was too aggressive — correctly-formed multi-chunk
  sources were being classified as `EmbeddingLinkage` and skipping extraction entirely, resulting
  in zero entities extracted. Disabled via `COVALENCE_LANDSCAPE_ENABLED=false` (#58 flag) while
  thresholds are re-calibrated. When disabled, all chunks receive `FullExtraction`.
- Not yet exposed via API for external inspection or surfaced in admin UI.

### Stage 5.5b: Coreference Resolution
- **File**: `ingestion/coreference.rs`
- **Status**: ✅ Independent preprocessing stage (#51 ✅ Closed)
- **#51 ✅ CLOSED**: Coref is now a **standalone pipeline stage** — extracted from
  `SidecarExtractor` and runs as an independent preprocessing step between embedding (Stage 5)
  and extraction (Stage 6). This enables running coref *before* chunking in the future and makes
  it independently testable and swappable.
- Calls the Python sidecar `/coref` endpoint with the chunk text; Fastcoref 90M handles inputs
  up to ~15K chars; longer texts are processed in large windows (500-char overlap).
- Coref failure is non-fatal: if `/coref` returns an error the original text is used and a
  `WARN`-level log entry is emitted.
- Controlled via `COVALENCE_COREF_ENABLED` (#58 stage configurability). Defaults to enabled
  when sidecar URL is configured.

### Stage 6: Entity & Relationship Extraction
- **File**: `ingestion/llm_extractor.rs`, `gliner_extractor.rs`, `two_pass_extractor.rs`,
  `sidecar_extractor.rs`
- **Status**: ✅ Two-pass active, sidecar wired
- **NEW (#44)**: `SidecarExtractor` implemented in Rust — a **three-stage** client calling the
  unified Python sidecar via HTTP: `/coref` → `/ner` → `/relationships`. Config:
  `COVALENCE_ENTITY_EXTRACTOR=sidecar`, `COVALENCE_EXTRACT_URL` (default:
  `http://localhost:8433`). Each stage is independently fault-tolerant: coref failure → use
  original text; NER failure → return empty result; relationship failure → return entities only.
- **GLiNER2 windowing IMPLEMENTED**: `SidecarExtractor` automatically splits inputs into
  overlapping ~1200-char windows (200-char overlap, sentence-boundary-aware) before calling
  `/ner`. Entity results are deduplicated by lowercased name across windows. The 384-token hard
  limit is now handled transparently in Rust — no silent entity loss for longer chunks.
- **NEW (#49)**: `SidecarExtractor` logs structured `WARN`-level messages on HTTP failures and
  JSON parse errors — no more silent failures; extraction errors visible in logs.
- **NEW (#32)**: Two-pass extraction available via `COVALENCE_ENTITY_EXTRACTOR=two_pass`:
  - **Pass 1**: GLiNER2 (~500MB, zero-shot NER) for entity extraction with types and spans.
  - **Pass 2**: LLM (e.g., Gemini 2.5 Flash via OpenRouter) for relationship-only extraction,
    constrained to GLiNER2 entity spans. LLM token usage reduced 50-70% vs single-pass.
  - Falls back to single-pass LLM if GLiNER2 sidecar is unreachable.
- **`COVALENCE_ENTITY_EXTRACTOR` modes**: `llm` (default), `two_pass`, `gliner2`, `sidecar`.
  The `sidecar` mode is the unified three-stage client (coref + NER + rels); `two_pass` uses
  separate `GlinerExtractor` + `LlmExtractor` clients.
- **Two-pass extraction CONFIRMED** (`COVALENCE_ENTITY_EXTRACTOR=two_pass`): GLiNER2 (local,
  ~500MB) for entity extraction + Gemini Flash (OpenRouter) for relationship extraction. This is
  the confirmed working configuration as of post-March-10.
- Gemini 2.5 Flash via OpenRouter ($0.30/M tokens) used for LLM pass/fallback.
- **Tested locally**: Fastcoref 90M + GLiNER2 ~500MB + NuExtract 0.5B full pipeline confirmed.
  Total RAM footprint: ~5.5GB with all local models loaded.
- **NEW (#57)**: Batch extraction with token thresholds — chunks with fewer than
  `min_extract_tokens=30` tokens are skipped (no extraction on trivially short content);
  extraction is batched with a target batch size of `extract_batch_tokens=2000` tokens.
- **NEW (#58)**: Full stage configurability via environment variables. Each major pipeline stage
  can be independently enabled/disabled: `COVALENCE_CONVERT_ENABLED`, `COVALENCE_COREF_ENABLED`,
  `COVALENCE_LANDSCAPE_ENABLED` (currently `false` — see #60), `COVALENCE_EXTRACT_ENABLED`.
  Useful for debugging, cost control, and selective pipeline runs.

### Stage 7: Entity Resolution
- **File**: `ingestion/resolver.rs`, `pg_resolver.rs`
- **Status**: ✅ Implemented
- Four-tier matching: exact name → fuzzy trigram → vector similarity → new entity
- PG advisory locks for concurrency safety
- Configurable similarity threshold

### Stage 7.5: Node Embedding
- Embeds node descriptions for vector search
- Partial failure resilience

### PII Detection
- **File**: `ingestion/pii.rs`
- **Status**: 🟡 Implemented (regex-based)
- Detects: email, phone, SSN, credit card patterns
- Not wired into pipeline as a blocking gate

### Content Takedown
- **File**: `ingestion/takedown.rs`
- **Status**: 🟡 Exists
- Mechanism for removing content post-ingestion

## Key Design Decisions

### Why hierarchical chunking over fixed-size
Heading boundaries are semantic boundaries. A chunk that splits mid-paragraph loses context. Hierarchical chunking preserves document structure: split on H1 first, then H2, then paragraphs, then sentences. Each chunk inherits its heading hierarchy as context.

### Why trait-based provider abstraction
The `Embedder` and `Extractor` traits allow swapping providers without touching pipeline logic. This enabled the OpenAI → Voyage migration and the GLiNER sidecar integration without any changes to pipeline orchestration.

### Why entity resolution uses four tiers
Exact match is fast but brittle ("PageRank" ≠ "pagerank"). Fuzzy trigram catches spelling variations. Vector similarity catches semantic equivalence ("ML" ≈ "machine learning"). Each tier has decreasing precision but increasing recall — the system tries cheap-and-precise first.

### Why two-pass extraction is now default
GLiNER2 for entities + NuExtract for relationships proved correct. 50-70% LLM token reduction vs single-pass, with better entity grounding (relationships are constrained to actual entity spans, not hallucinated names). The sidecar fallback ensures single-pass still works when the sidecar is down.

### Why table linearization in pure Rust (#45)
Markdown pipe tables are common in technical docs and papers but were previously passed through as-is, producing garbled embeddings ("| Concept | Paper |" as a semantic unit is meaningless). Pure Rust linearization (no model) is instant and deterministic — essential for byte-offset reproducibility (#30).

### Why pymupdf4llm for PDF conversion
No model inference — pure text extraction with structure preservation. 3.4s for 15 pages is fast enough for real-time ingestion. Compared to alternatives (Nougat, Surya), pymupdf4llm is simpler to deploy and deterministic.

## Gaps Identified

1. **GLiNER2 windowing** — ✅ **FIXED** (in `SidecarExtractor`). Input text is split into
   ~1200-char overlapping windows (200-char overlap, sentence-boundary-aware) before calling
   `/ner`. Entities are deduplicated by lowercased name across windows. The 384-token truncation
   limit is now handled transparently in Rust.

2. **PII detection not gated** — PII patterns are detected but don't block ingestion. In a
   multi-user deployment, this is a compliance risk.

3. **Landscape analysis disabled (#60)** — landscape gating was too aggressive, incorrectly
   classifying valid chunks as `EmbeddingLinkage` and skipping extraction. Currently disabled via
   `COVALENCE_LANDSCAPE_ENABLED=false`. Thresholds need calibration before re-enabling. Once
   re-enabled, landscape metrics should also be exposed via API for external inspection.

4. **confidence_breakdown not populated** — extraction produces confidence scores but they're not
   stored in the edge JSONB.

5. **Voyage node embeddings** — ✅ **FIXED**: `PgResolver` and `AdminService` both receive the
   same `Arc<dyn Embedder>` as chunk embedding (VoyageEmbedder when `VOYAGE_API_KEY` present), so
   node embeddings now use Voyage consistently via the shared embedder instance.

## Runtime & Environment Notes

- **Project root required**: The pipeline uses `dotenvy` to load `.env` at startup. `dotenvy`
  reads from the current working directory — **always run the server from the project root**,
  otherwise `.env` is not found and all env-var config is silently missing.
- **`VOYAGE_API_KEY`**: Required for Voyage AI embeddings (`voyage-3-large`) and reranking
  (`rerank-2.5`). Set in `.env` or shell environment. Without this key, the system falls back
  to OpenAI embeddings if `OPENAI_API_KEY` is set, or fails embedding entirely.
- **Bug #59 (fixed locally)**: `dotenvy` was being initialized *after* `tracing_subscriber`,
  meaning `.env` values (e.g., `RUST_LOG`) were not visible to the logging framework at startup.
  Fixed by moving `dotenvy::dotenv().ok()` to the very top of `main()`, before any other
  initialization. Commit pending.

## Open Issues

| Issue | Description | Status |
|-------|-------------|--------|
| #11 | Fine-tune relationship extraction | 🔴 Open |
| #51 | Separate coref preprocessing from extraction | ✅ Closed (coref is now standalone stage) |
| #52 | Converter windowing for large HTML; PDF converter placeholder | ✅ Closed |
| #57 | Batch extraction (min_extract_tokens=30, extract_batch_tokens=2000) | ✅ Closed |
| #58 | Full stage configurability (CONVERT_ENABLED, COREF_ENABLED, LANDSCAPE_ENABLED…) | ✅ Closed |
| #59 | dotenvy initialized after tracing_subscriber (bug, fixed locally) | 🟡 Fix needs commit |
| #60 | Landscape gating too aggressive — disabled (COVALENCE_LANDSCAPE_ENABLED=false) | 🔴 Open |
| All others (28, 30, 32, 43, 44, 45, 46, 49...) | See above | ✅ Closed |

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| GLiNER zero-shot NER | Zaratiana et al. 2023 | ✅ Ingested |
| Matryoshka embeddings | Kusupati et al. 2022 | ✅ Ingested |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Ingested |
| Entity resolution | — | ❌ Need survey paper on entity resolution/deduplication |
| Information extraction | — | ❌ Need survey on relation extraction from text |

## Next Actions

1. **Re-calibrate landscape thresholds** (#60) — tune `EmbeddingLinkage`/`DeltaCheck` similarity
   cutoffs so valid chunks aren't skipped; then re-enable `COVALENCE_LANDSCAPE_ENABLED=true`
2. **Commit dotenvy fix** (#59) — move `dotenvy::dotenv().ok()` before `tracing_subscriber` init
3. **Complete PDF converter** (#52) — replace placeholder with working `pymupdf4llm` sidecar call
4. Wire PII detection as ingestion gate (configurable: warn vs block)
5. Surface landscape analysis via API or admin endpoint (metrics stored per chunk)
6. Populate `confidence_breakdown` JSONB during extraction
7. Ingest entity resolution and relation extraction survey papers
