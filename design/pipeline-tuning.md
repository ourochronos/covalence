# Pipeline Tuning Guide

## Status: active — updated with March 10 benchmarks

> **Updated 2026-03-10 (+post2)**: Model stack confirmed end-to-end. GLiNER2 windowing
> **implemented** in `SidecarExtractor`. Coref is now an **independent preprocessing stage**
> (#51 ✅ Closed). Voyage reranker auto-activated when `VOYAGE_API_KEY` present. **Landscape
> disabled** (#60, `COVALENCE_LANDSCAPE_ENABLED=false`) — gating too aggressive. New closures:
> #52 (converter windowing + PDF placeholder), #57 (batch extraction token thresholds), #58
> (full stage configurability flags). Bug #59: dotenvy must initialize before tracing_subscriber;
> pipeline must run from project root. Two-pass confirmed: GLiNER (local) + Gemini Flash.

## Purpose

Document each stage of the ingestion pipeline, its current implementation, available alternatives, and how to independently test and benchmark each stage. The goal is to make provider/model choices data-driven rather than assumed.

## Pipeline Stages

```
Source → Accept → Convert → Parse → Normalize → Chunk → Embed → Coreference → Extract (NER) → Extract (RE) → Landscape → Resolve
```

Each stage has a trait, a current implementation, and candidate replacements. Every stage should be independently testable with fixture inputs and measurable outputs.

---

### 1. Accept

**Trait**: `AcceptResult` — dedup via SHA-256 content hash, MIME detection, size validation.

**Current**: Built-in (Rust). No external dependency.

**Options**: None needed — this is pure logic.

**Test**: Given duplicate content, verify dedup. Given various MIME types, verify detection.

**NEW (#28)**: Now also accepts URLs — fetches content via HTTP, extracts MIME from headers, then proceeds through the same accept flow.

---

### 2. Convert

**Trait**: `SourceConverter` — raw bytes + content type → Markdown string.

**Current**:
- `MarkdownConverter` — passthrough
- `PlainTextConverter` — wraps in heading
- `HtmlConverter` — hand-rolled tag stripper (naive, no tables)
- `ReaderLmConverter` — MLX sidecar using ReaderLM-v2 (#41, ✅ implemented)
- **NEW (#45)**: Table linearization — pure Rust, converts MD pipe tables to natural language
- **NEW (#52)**: Converter windowing — `HtmlConverter` splits large HTML documents into
  overlapping windows before passing to ReaderLM-v2, preventing silent context truncation
- **NEW**: `PdfConverter` via pymupdf4llm — currently a **placeholder**; trait is wired but
  implementation returns a stub pending full sidecar integration

**Candidates**:
| Option | Type | Quality | Speed | Cost | Status |
|--------|------|---------|-------|------|--------|
| Hand-rolled `HtmlConverter` | Built-in | Low (no tables, no semantic) | Instant | Free | ✅ Active (fallback) |
| ReaderLM-v2 (MLX) | Local SLM | High (tables, structure) | ~1-2s/doc | Free | ✅ Active |
| pymupdf4llm | Library | High (PDF → MD, tables preserved) | **3.4s/15 pages** | Free | ✅ Active |
| Jina Reader API | Cloud API | High | ~1s/doc | $0.01/page? | Not tested |
| Trafilatura (Python) | Library | Medium (good extraction) | Fast | Free | Not tested |
| Mozilla Readability | Library | Medium | Fast | Free | Not tested |

**Benchmark (March 10)**: pymupdf4llm on 15-page academic paper: 3.4 seconds, no model inference, accurate structure including tables and headings. No GPU required.

**Test fixture**: Set of HTML pages (simple, tables, nav-heavy, JS-rendered) → compare markdown output quality. Measure: structural preservation, noise removal, byte-for-byte determinism (needed for #30 byte offsets).

---

### 3. Parse

**Trait**: Parser — Markdown → structured sections (headings, code blocks, tables).

**Current**: Built-in Markdown section parser.

**Options**: None needed — Markdown parsing is deterministic.

**Test**: Given markdown with various structures, verify section boundaries.

---

### 4. Normalize

**Trait**: `normalize` — Unicode NFC normalization, whitespace collapsing.

**Current**: Built-in (Rust, `unicode-normalization` crate).

**NEW (#30)**: `normalized_content` and `normalized_hash` stored on sources. Hash-based change detection enables incremental re-ingestion — only re-chunk/re-embed if content actually changed.

**Test**: Given various Unicode inputs, verify NFC output. Verify idempotency.

---

### 5. Chunk

**Trait**: `chunk_document` — normalized markdown → chunks with metadata.

**Current**: Configurable size/overlap chunker. Section-aware (respects heading boundaries).

**NEW (#30)**: `byte_start`, `byte_end`, `content_offset` tracked on each chunk — enables precise source attribution back to original document byte range.

**Candidates**:
| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| Current (size + section + byte offsets) | Fixed token window, section-aware | Predictable, fast, now traceable | May split mid-paragraph |
| Semantic chunking | Embed sentences, split at low-similarity boundaries | Better coherence | Slower, needs embedder |
| Late chunking (Voyage) | Chunk after embedding full doc | Context preserved | Tied to Voyage |
| Recursive character splitter | LangChain-style | Simple | No semantic awareness |

**Key question**: How much does chunk quality affect downstream search? Need A/B comparison.

---

### 6. Embed

**Trait**: `Embedder` — batch of text → batch of vectors. Also `embed_document_chunks` for contextual.

**Current**: ✅ **Voyage `voyage-3-large`** (switched from OpenAI on March 10).

**Candidates**:
| Option | Type | Dims | Quality | Speed | Cost | Status |
|--------|------|------|---------|-------|------|--------|
| Voyage `voyage-3-large` | Cloud API | 2048 (truncatable) | High | Fast | **$0.01/M tokens** | ✅ Active |
| OpenAI `text-embedding-3-large` | Cloud API | 3072 (truncatable) | High | Fast | $0.13/M tokens | ❌ Replaced |
| Voyage `voyage-context-3` | Cloud API | 1024 | High (contextual) | Fast | ~$0.06/M | Not tested |
| `nomic-embed-text` (Ollama/MLX) | Local | 768 | Medium | Medium | Free | Not tested |
| BGE-M3 (MLX?) | Local | 1024 | High | Medium | Free | Not tested |
| Jina `jina-embeddings-v3` | Cloud/Local | 1024 | High | Fast | $0.02/M | Not tested |

**⚠️ Known issue**: After Voyage migration, the pgvector index dimension count doesn't match the new embedding dimensions. Vector search dimension is not firing — only lexical BM25 active. **Needs index rebuild.**

**Key question**: Can a local embedding model match Voyage quality for our domain? At $0.01/M, Voyage is already very cheap — local would only win on latency and privacy.

---

### 6.5. Coreference Resolution

**Trait**: Coreference → resolve pronouns and references across chunks.

**Current**: ✅ **Independent preprocessing stage** (`ingestion/coreference.rs`). As of #51
closure, coref has been extracted from `SidecarExtractor` and runs as a standalone pipeline
stage. Calls `/coref` on the Python sidecar before Stage 6 extraction. Fastcoref 90M handles up
to ~15K chars per call; longer inputs are windowed with 500-char overlap. Coref failure is
non-fatal (original text used, WARN logged). Controlled via `COVALENCE_COREF_ENABLED` (#58).

**#51 ✅ Closed**: Coref is now separate — running it as an independent stage allows independent
testing, future pre-chunking coref, and cleaner model swapping without touching the extractor.

**Benchmark (March 10)**:
| Property | Value |
|----------|-------|
| Model | Fastcoref 90M (in Python sidecar) |
| RAM | ~300MB |
| Max context | ~15K chars before windowing |
| Speed | Fast (CPU-only) |
| Quality | Good for pronoun resolution and entity tracking |

**Key question (future)**: Should coreference run before or after chunking? Currently AFTER
chunking (Stage 5.5b). Before-chunking would let the chunker see resolved references and
potentially produce better boundaries — a future improvement now that the stage is independent.

---

### 7. Extract — Entity Recognition (NER)

**Trait**: `Extractor` — chunk text → entities with types and spans.

**Current**: ✅ **GLiNER2 ~500MB** via unified sidecar (`SidecarExtractor`, `COVALENCE_ENTITY_EXTRACTOR=sidecar`).

**Windowing implemented**: Rust splits input into overlapping ~1200-char windows (200-char overlap,
sentence-boundary-aware) in `SidecarExtractor::extract_entities()` before sending to `/ner`.
Entities from all windows are deduplicated by lowercased name. The 384-token hard limit is now
handled transparently — no silent entity loss for long chunks.

**Benchmark (March 10)**:
| Property | Value |
|----------|-------|
| Model | GLiNER2 ~500MB |
| RAM | ~500MB |
| Max context | 384 tokens (hard limit — mitigated by Rust windowing) |
| Window size | ~1200 chars, 200-char overlap, sentence-boundary splits |
| Speed | Fast (CPU-only) |
| Quality | High zero-shot NER |
| Entity types | Open vocabulary (user-specified or emergent) |

**Candidates**:
| Option | Type | Quality | Context | Speed | Cost | Status |
|--------|------|---------|---------|-------|------|--------|
| GLiNER2 | Local | High (entities) | **384 tokens** | Fast | Free | ✅ Active (with truncation caveat) |
| LLM (Gemini 2.5 Flash) | Cloud API | High | 1M tokens | Medium | $0.30/M | ✅ Active (fallback) |
| Fine-tuned BERT NER | Local | Domain-specific | 512 tokens | Fast | Free (training cost) | Not tested (#11) |
| REBEL (HF) | Local | Medium | 512 | Fast | Free | Not tested |

---

### 7.5. Extract — Relationship Extraction (RE)

**Trait**: `Extractor` — entities + chunk text → relationships.

**Current**: ✅ **NuExtract-1.5-tiny 0.5B** via sidecar, constrained to GLiNER2 entity spans.

**Benchmark (March 10)**:
| Property | Value |
|----------|-------|
| Model | NuExtract-1.5-tiny 0.5B |
| RAM | ~1GB |
| Max context | **4K tokens** |
| Speed | Medium (CPU) |
| Quality | Good for relationship extraction when constrained to known entities |
| Token savings | 50-70% vs single-pass LLM extraction |

**Fallback**: Gemini 2.5 Flash via OpenRouter ($0.30/M tokens) — used when sidecar is unavailable or for enrichment of complex passages.

**Two-pass CONFIRMED** (`COVALENCE_ENTITY_EXTRACTOR=two_pass`): GLiNER2 (local, ~500MB) for
entity extraction + Gemini Flash (OpenRouter) for relationship extraction. This is the confirmed
working two-pass configuration as of post-March-10 testing.

**NEW (#57)**: Batch extraction token thresholds:
- `min_extract_tokens=30` — chunks with fewer tokens are skipped (no extraction on trivially short content such as headers or metadata fragments)
- `extract_batch_tokens=2000` — extraction batched at this token target to control sidecar/LLM call size and cost

**NEW (#58)**: Each pipeline stage can be independently controlled via env vars:
`COVALENCE_CONVERT_ENABLED`, `COVALENCE_COREF_ENABLED`, `COVALENCE_LANDSCAPE_ENABLED`,
`COVALENCE_EXTRACT_ENABLED`. Useful for debugging specific stages, cost control during development, and A/B testing individual components.

**Candidates**:
| Option | Type | Quality | Context | Speed | Cost | Status |
|--------|------|---------|---------|-------|------|--------|
| NuExtract-1.5-tiny | Local 0.5B | Good | 4K tokens | Medium | Free | ✅ Active |
| LLM (Gemini 2.5 Flash) | Cloud API | High | 1M tokens | Medium | $0.30/M | ✅ Active (fallback) |
| Fine-tuned RE model | Local | High (domain) | Varies | Fast | Free (training) | Not tested (#11) |
| UniRel (HF) | Local | Medium | 512 | Fast | Free | Not tested |

---

### 8. Landscape

**Trait**: `analyze_landscape` — assess extraction confidence, decide extraction method.

**Current**: 🟡 **Wired but currently DISABLED** (`COVALENCE_LANDSCAPE_ENABLED=false`).

**⚠️ #60 BUG**: Landscape gating was too aggressive — valid multi-chunk sources were being
classified as `EmbeddingLinkage` and skipping extraction entirely, producing zero entities.
Disabled via `COVALENCE_LANDSCAPE_ENABLED=false` (#58 configurability flag) while similarity
thresholds are re-calibrated. When disabled, all chunks receive `FullExtraction`.

When re-enabled: results stored per chunk (`extraction_method`, `landscape_metrics` JSONB);
`EmbeddingLinkage`/`DeltaCheck` chunks skip extractor; only `FullExtraction` and
`FullExtractionWithReview` chunks proceed. **#43**: single-chunk sources always bypass landscape.

**Options**: Tightly coupled to extraction — evolves with #42 and threshold calibration work.

**Test**: Given chunks with varying complexity, verify method selection makes sense.

---

### 9. Resolve

**Trait**: `EntityResolver` — map extracted entities/relationships to existing graph nodes/edges.

**Current**: 5-tier resolution: exact match → trigram → vector → type+context → create new. Graph context disambiguation.

**Candidates**:
| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| Current (5-tier) | Cascading similarity | Comprehensive | Complex, hard to tune thresholds |
| Embedding-only | Pure vector similarity | Simple | Misses exact matches |
| LLM-assisted | Ask LLM "are these the same entity?" | High accuracy | Expensive |
| HDBSCAN post-hoc | Cluster first, resolve later | Batch-friendly | Latency |

**Test fixture**: Set of entity pairs (same/different) with ground truth. Measure: merge accuracy (are true duplicates merged?), split accuracy (are distinct entities kept separate?).

---

## Full Local Pipeline RAM Budget (confirmed March 10)

| Component | RAM | Notes |
|-----------|-----|-------|
| ReaderLM-v2 (MLX) | ~1.0 GB | HTML → Markdown conversion |
| pymupdf4llm | ~0 GB | PDF extraction (no model) |
| Fastcoref 90M | ~0.3 GB | Coreference resolution |
| GLiNER2 | ~0.5 GB | Zero-shot NER |
| NuExtract-1.5-tiny | ~1.0 GB | Relationship extraction |
| HDBSCAN + embeddings | ~0.5 GB | Clustering (varies with corpus) |
| Covalence server (Rust) | ~0.2 GB | Core process |
| PostgreSQL + pgvector | ~2.0 GB | Database (varies with corpus) |
| **Total** | **~5.5 GB** | Fits on 8GB machine with headroom |

## Cost Model (confirmed March 10)

| Service | Provider | Cost | Notes |
|---------|----------|------|-------|
| Embeddings | Voyage AI (`voyage-3-large`) | $0.01/M tokens | 13× cheaper than OpenAI |
| LLM extraction | Gemini 2.5 Flash (OpenRouter) | $0.30/M tokens | Fallback / enrichment |
| Reranking | Voyage `rerank-2.5` | ~$0.05/M tokens | Auto-activated when `VOYAGE_API_KEY` present; NoopReranker fallback if no key |
| NER | GLiNER2 (local) | Free | ~500MB RAM |
| RE | NuExtract (local) | Free | ~1GB RAM |
| Coreference | Fastcoref (local) | Free | ~300MB RAM |
| PDF conversion | pymupdf4llm (local) | Free | Pure extraction |
| HTML conversion | ReaderLM-v2 (local) | Free | ~1GB RAM |

## Model Context Limits (critical for tuning)

| Model | Max Context | Behavior at Limit | Workaround |
|-------|-------------|-------------------|------------|
| Fastcoref 90M | ~20KB | Untested beyond 20KB | Chunk-level application; 20KB is plenty |
| **GLiNER2** | **384 tokens** | Silent truncation (mitigated) | Rust-side windowing implemented in `SidecarExtractor`: ~1200-char windows, 200-char overlap, sentence boundaries |
| NuExtract-1.5-tiny | 4K tokens | Untested beyond | Chunk-level; 4K sufficient for most chunks |
| Gemini 2.5 Flash | 1M tokens | Cost scales linearly | Budget management via token cap |
| Voyage voyage-3-large | 32K tokens | Truncation | Document-level OK for most content |

## Evaluation Framework

Each stage needs:
1. **Fixture inputs** — representative samples from our actual corpus
2. **Expected outputs** — human-annotated or high-quality-model-generated gold standard
3. **Metrics** — stage-appropriate (see above)
4. **Comparison harness** — run N providers on same fixtures, compare metrics
5. **Integration test** — does a change in stage K improve end-to-end search quality?

The eval harness (`covalence-eval`) is now verified (#4) with layer evaluators for chunking, extraction, and search. Needs fixture data to actually run.

## Priority Order (updated post-March-10 wave 2)

1. **Vector index rebuild** — fix Voyage dimension alignment, re-enable vector search dimension
2. **Re-calibrate landscape thresholds** (#60) — gating too aggressive; re-enable once tuned
3. **Commit dotenvy fix** (#59) — tracing_subscriber sees `.env` vars correctly
4. **Complete PDF converter** (#52) — replace placeholder with working implementation
5. **Generate eval fixtures** — the harness works, give it data
6. **Fine-tune RE model** (#11) — labeled dataset needed first

> **Done since March 10**: GLiNER2 windowing (✅), Voyage reranker (✅ auto-wired), #51 coref
> separation (✅), #52 converter windowing (✅), #57 batch token thresholds (✅), #58 stage
> configurability (✅). Two-pass extraction confirmed working (GLiNER local + Gemini Flash).

## Related Issues
- #30 — Byte offset chunks ✅ Closed
- #41 — ReaderLM-v2 converter ✅ Closed
- #42 — Extraction alternatives research 🔴 Open
- #43 — Landscape bypass for single-chunk sources ✅ Closed
- #44 — Extraction sidecar (SidecarExtractor, windowing) ✅ Closed
- #45 — Table linearization ✅ Closed
- #46 — Voyage env-based provider switching (COVALENCE_EMBED_PROVIDER) ✅ Closed
- #4 — Layer-by-layer evaluation harness ✅ Closed
- #11 — Fine-tune relationship extraction 🔴 Open
- #51 — Separate coref preprocessing from extraction ✅ Closed
- #52 — Converter windowing (large HTML); PDF converter placeholder ✅ Closed
- #57 — Batch extraction token thresholds (min_extract_tokens, extract_batch_tokens) ✅ Closed
- #58 — Full stage configurability (CONVERT_ENABLED, COREF_ENABLED, LANDSCAPE_ENABLED…) ✅ Closed
- #59 — dotenvy initialized after tracing_subscriber (bug, fix needs commit) 🟡
- #60 — Landscape gating too aggressive, disabled (COVALENCE_LANDSCAPE_ENABLED=false) 🔴 Open
