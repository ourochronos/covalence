# Design: Ingestion Pipeline

## Status: implemented (core), partial (two-pass, PII, landscape)

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

### Stage 1.5: Convert
- **File**: `ingestion/converter.rs`
- **Status**: ✅ Implemented
- `ConverterRegistry` with pluggable converters: Markdown, PlainText, HTML
- Missing: PDF, DOCX, audio transcription converters
- All content normalized to markdown for downstream processing

### Stage 2-3: Parse & Normalize
- **File**: `ingestion/normalize.rs`
- **Status**: ✅ Implemented
- Unicode normalization, whitespace cleanup
- Text normalization for consistent entity matching

### Stage 4: Chunk
- **File**: `ingestion/chunker.rs`
- **Status**: ✅ Implemented (fixed in #29)
- Hierarchical chunking: respects heading boundaries, then paragraph, then sentence
- Configurable: `COVALENCE_CHUNK_SIZE=1000`, `COVALENCE_CHUNK_OVERLAP=200`
- UTF-8 safe after #29 fix (multi-byte boundary handling)
- Missing: byte offset tracking (#30), sentence-level chunking (removed as simplification)

### Stage 5: Embed Chunks
- **File**: `ingestion/embedder.rs`, `openai_embedder.rs`, `voyage.rs`
- **Status**: ✅ Implemented
- Matryoshka truncation: `truncate_embedding()` for per-table dimension tiering
- Providers: OpenAI `text-embedding-3-large` (active), Voyage `voyage-context-3` (implemented, not wired as default)
- Batch embedding with retry and partial failure resilience (#27)
- Missing: Voyage as default provider, late chunking support

### Stage 5.5: Embedding Landscape Analysis
- **File**: `ingestion/landscape.rs`
- **Status**: 🟡 Implemented but not surfaced
- Computes cosine similarity distribution across chunk embeddings
- Detects topic diversity, embedding quality anomalies
- Model calibration metrics
- Not exposed via API or used in downstream decisions

### Stage 5.5b: Coreference Resolution
- **File**: `ingestion/coreference.rs`
- **Status**: 🟡 Exists but basic
- Cross-chunk coreference linking
- Likely needs improvement for pronoun resolution, entity tracking across chunks

### Stage 6: Entity & Relationship Extraction
- **File**: `ingestion/llm_extractor.rs`, `gliner_extractor.rs`, `two_pass_extractor.rs`
- **Status**: 🟡 Single-pass active, two-pass implemented but needs GLiNER sidecar
- **Single-pass** (active): LLM extracts entities AND relationships together. Works but expensive and produces noisy relationships.
- **Two-pass** (implemented, not active):
  - Pass 1: GLiNER (local, fast) for entity extraction with types and spans
  - Pass 2: LLM for relationship-only extraction, constrained to GLiNER entities
  - Benefits: 50-70% LLM token reduction, grounded entities, shorter prompts
  - Blocker: GLiNER Python sidecar not deployed (#32)
- Missing: relationship-only LLM prompt (current prompt is single-pass)

### Stage 7: Entity Resolution
- **File**: `ingestion/resolver.rs`, `pg_resolver.rs`
- **Status**: ✅ Implemented
- Four-tier matching: exact name → fuzzy trigram → vector similarity → new entity
- PG advisory locks for concurrency safety
- Configurable similarity threshold
- Known issue: entity duplication still occurs (e.g., "NLI" not merging with existing "NLI" node from different source)

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
The `Embedder` and `Extractor` traits allow swapping providers without touching pipeline logic. This enabled the OpenAI → Voyage migration path and the GLiNER sidecar integration.

### Why entity resolution uses four tiers
Exact match is fast but brittle ("PageRank" ≠ "pagerank"). Fuzzy trigram catches spelling variations. Vector similarity catches semantic equivalence ("ML" ≈ "machine learning"). Each tier has decreasing precision but increasing recall — the system tries cheap-and-precise first.

### Why single-pass extraction is still default
Two-pass is better but requires the GLiNER sidecar. Single-pass LLM extraction works out of the box with just an API key. The fallback is explicit: if GLiNER sidecar is unavailable, use single-pass.

## Gaps Identified

1. **GLiNER sidecar not deployed** — two-pass extraction is implemented but blocked. This is the single biggest quality improvement available.

2. **Entity resolution misses** — "NLI" from two different sources creates duplicate nodes instead of merging. The resolver's vector similarity threshold may be too conservative, or the matching fails when exact names differ slightly.

3. **PII detection not gated** — PII patterns are detected but don't block ingestion. In a multi-user deployment, this is a compliance risk.

4. **Landscape analysis not actionable** — embedding quality metrics are computed but not surfaced or used. Could flag low-quality sources or trigger re-embedding.

5. **No URL-based ingestion** (#28) — must provide content directly, can't fetch from URL.

6. **Voyage not wired as default** — OpenAI embeddings active, Voyage implemented but not configured.

7. **confidence_breakdown not populated** — extraction produces confidence scores but they're not stored in the edge JSONB.

8. **Example entity pollution** (#40) — spec examples ("John works at Google") extracted as real entities.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| GLiNER zero-shot NER | Zaratiana et al. 2023 | ✅ Ingested |
| Late chunking | Günther et al. 2024 | ✅ Ingested |
| Matryoshka embeddings | Kusupati et al. 2022 | ✅ Ingested |
| Ontology engineering | Noy & McGuinness 2001 | ✅ Just ingested |
| Entity resolution | — | ❌ Need survey paper on entity resolution/deduplication |
| Information extraction | — | ❌ Need survey on relation extraction from text |

## Next Actions

1. Deploy GLiNER Python sidecar → activate two-pass extraction
2. Investigate entity resolution misses (threshold tuning or algorithm improvement)
3. Wire Voyage as default embedding provider
4. Wire PII detection as ingestion gate (configurable: warn vs block)
5. Surface landscape analysis via API or admin endpoint
6. Ingest entity resolution and relation extraction survey papers
