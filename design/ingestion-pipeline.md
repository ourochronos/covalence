# Design: Ingestion Pipeline

## Status: implemented (core + two-pass + URL + sidecar), partial (PII, landscape)

> **Updated 2026-03-10**: Massive engineering wave closed 47/50 GitHub issues. Key additions: table
> linearization (#45), byte-offset chunking (#30), URL ingestion (#28), SidecarExtractor (#44),
> extraction error logging (#49), two-pass extraction activated (#32), full local model pipeline
> tested (Fastcoref → GLiNER2 → NuExtract). GLiNER2 384-token limit identified — Rust-side
> windowing still needed.

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
- **Status**: 🟡 Implemented but not surfaced
- Computes cosine similarity distribution across chunk embeddings
- Detects topic diversity, embedding quality anomalies
- Model calibration metrics
- Not exposed via API or used in downstream decisions

### Stage 5.5b: Coreference Resolution
- **File**: `ingestion/coreference.rs`
- **Status**: ✅ Tested locally
- **NEW**: Fastcoref 90M model tested locally — handles 20KB context OK (confirmed benchmark).
  Cross-chunk coreference linking now functional.
- Pronoun resolution and entity tracking across chunks working in practice

### Stage 6: Entity & Relationship Extraction
- **File**: `ingestion/llm_extractor.rs`, `gliner_extractor.rs`, `two_pass_extractor.rs`,
  `sidecar_extractor.rs`
- **Status**: ✅ Two-pass active, sidecar wired
- **NEW (#44)**: `SidecarExtractor` implemented in Rust — calls Python sidecar via HTTP. Config:
  `COVALENCE_EXTRACTION_URL`. Falls back to single-pass LLM if sidecar is unreachable.
- **NEW (#49)**: `parse_extraction_json` now logs structured warnings on malformed sidecar
  responses — no more silent failures; extraction errors visible in logs.
- **NEW (#32)**: Two-pass extraction now active end-to-end:
  - **Pass 1**: GLiNER2 (~500MB, zero-shot NER) for entity extraction with types and spans.
    Truncates at **384 tokens** — Rust-side windowing (~1KB chunks with 20% overlap) needed to
    handle longer chunks safely. Currently a known gap.
  - **Pass 2**: NuExtract-1.5-tiny (0.5B) at 4K token context for relationship-only extraction,
    constrained to GLiNER2 entity spans. LLM token usage reduced 50-70% vs single-pass.
  - Gemini 2.5 Flash via OpenRouter ($0.30/M tokens) used as fallback/supplemental LLM.
- **Tested locally**: Fastcoref 90M + GLiNER2 ~500MB + NuExtract 0.5B full pipeline confirmed.
  Total RAM footprint: ~5.5GB with all local models loaded.

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

1. **GLiNER2 windowing** — GLiNER2 truncates at 384 tokens. Chunks > ~1KB are silently truncated,
   missing entities in the latter portion. Need Rust-side windowing that splits chunks with overlap
   before sending to GLiNER2, then deduplicates entities by span. Estimated: 1-2 hours of Rust work.

2. **PII detection not gated** — PII patterns are detected but don't block ingestion. In a
   multi-user deployment, this is a compliance risk.

3. **Landscape analysis not actionable** — embedding quality metrics are computed but not surfaced
   or used. Could flag low-quality sources or trigger re-embedding.

4. **confidence_breakdown not populated** — extraction produces confidence scores but they're not
   stored in the edge JSONB.

5. **Voyage not yet active for node embeddings** — switched for chunk embeddings but node embedding
   (`Stage 7.5`) still uses old provider config. Needs one-line config change.

## Open Issues

| Issue | Description | Status |
|-------|-------------|--------|
| #11 | Fine-tune relationship extraction | 🔴 Open |
| All others (28, 30, 32, 44, 45, 49...) | See above | ✅ Closed 2026-03-10 |

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| GLiNER zero-shot NER | Zaratiana et al. 2023 | ✅ Ingested |
| Late chunking | Günther et al. 2024 | ✅ Ingested |
| Matryoshka embeddings | Kusupati et al. 2022 | ✅ Ingested |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Ingested |
| Entity resolution | — | ❌ Need survey paper on entity resolution/deduplication |
| Information extraction | — | ❌ Need survey on relation extraction from text |

## Next Actions

1. Implement Rust-side GLiNER2 windowing (~1KB chunks, 20% overlap, span dedup)
2. Wire PII detection as ingestion gate (configurable: warn vs block)
3. Surface landscape analysis via API or admin endpoint
4. Populate `confidence_breakdown` JSONB during extraction
5. Switch node embeddings to Voyage (one-line config change)
6. Ingest entity resolution and relation extraction survey papers
