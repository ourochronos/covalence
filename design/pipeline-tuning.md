# Pipeline Tuning Guide

## Status: draft

## Purpose

Document each stage of the ingestion pipeline, its current implementation, available alternatives, and how to independently test and benchmark each stage. The goal is to make provider/model choices data-driven rather than assumed.

## Pipeline Stages

```
Source → Accept → Convert → Parse → Normalize → Chunk → Embed → Extract → Landscape → Resolve
```

Each stage has a trait, a current implementation, and candidate replacements. Every stage should be independently testable with fixture inputs and measurable outputs.

---

### 1. Accept

**Trait**: `AcceptResult` — dedup via SHA-256 content hash, MIME detection, size validation.

**Current**: Built-in (Rust). No external dependency.

**Options**: None needed — this is pure logic.

**Test**: Given duplicate content, verify dedup. Given various MIME types, verify detection.

---

### 2. Convert

**Trait**: `SourceConverter` — raw bytes + content type → Markdown string.

**Current**:
- `MarkdownConverter` — passthrough
- `PlainTextConverter` — wraps in heading
- `HtmlConverter` — hand-rolled tag stripper (naive, no tables)
- `ReaderLmConverter` — MLX sidecar using ReaderLM-v2 (#41, in progress)

**Candidates**:
| Option | Type | Quality | Speed | Cost |
|--------|------|---------|-------|------|
| Hand-rolled `HtmlConverter` | Built-in | Low (no tables, no semantic) | Instant | Free |
| ReaderLM-v2 (MLX) | Local SLM | High (tables, structure) | ~1-2s/doc | Free |
| Jina Reader API | Cloud API | High | ~1s/doc | $0.01/page? |
| Trafilatura (Python) | Library | Medium (good extraction) | Fast | Free |
| Mozilla Readability | Library | Medium | Fast | Free |

**Test fixture**: Set of HTML pages (simple, tables, nav-heavy, JS-rendered) → compare markdown output quality. Measure: structural preservation, noise removal, byte-for-byte determinism (needed for #30 byte offsets).

**Key question**: Does the converter produce deterministic output? Byte offsets (#30) require it.

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

**Options**: None needed — this is standards-based.

**Test**: Given various Unicode inputs, verify NFC output. Verify idempotency.

---

### 5. Chunk

**Trait**: `chunk_document` — normalized markdown → chunks with metadata.

**Current**: Configurable size/overlap chunker. Section-aware (respects heading boundaries).

**Candidates**:
| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| Current (size + section) | Fixed token window, section-aware | Predictable, fast | May split mid-paragraph |
| Semantic chunking | Embed sentences, split at low-similarity boundaries | Better coherence | Slower, needs embedder |
| Late chunking (Voyage) | Chunk after embedding full doc | Context preserved | Tied to Voyage |
| Recursive character splitter | LangChain-style | Simple | No semantic awareness |

**Test fixture**: Set of documents of various sizes → measure: chunk count, avg size, cross-chunk entity splits (entity mentioned in chunk N but referent in chunk N-2), retrieval quality downstream.

**Key question**: How much does chunk quality affect downstream search? Need A/B comparison.

---

### 6. Embed

**Trait**: `Embedder` — batch of text → batch of vectors. Also `embed_document_chunks` for contextual.

**Current**: OpenAI `text-embedding-3-large` (quota issues).

**Candidates**:
| Option | Type | Dims | Quality | Speed | Cost |
|--------|------|------|---------|-------|------|
| OpenAI `text-embedding-3-large` | Cloud API | 3072 (truncatable) | High | Fast | $0.13/M tokens |
| Voyage `voyage-3-large` | Cloud API | 2048 (truncatable) | High | Fast | $0.06/M tokens |
| Voyage `voyage-context-3` | Cloud API | 1024 | High (contextual) | Fast | ~$0.06/M |
| `nomic-embed-text` (Ollama/MLX) | Local | 768 | Medium | Medium | Free |
| BGE-M3 (MLX?) | Local | 1024 | High | Medium | Free |
| Jina `jina-embeddings-v3` | Cloud/Local | 1024 | High | Fast | $0.02/M |

**Test fixture**: Standard IR benchmark (or custom Covalence eval set). Measure: retrieval precision@k, recall@k, MRR on known-relevant pairs. Compare across models.

**Key question**: Can a local embedding model match API quality for our domain? Our content is technical (ML papers, Rust code, system design) — domain matters.

---

### 7. Extract

**Trait**: `Extractor` — chunk text → entities + relationships.

**Current**: LLM chat completions (now Gemini Flash Lite via OpenRouter).

**Candidates**:
| Option | Type | Entity Quality | Relationship Quality | Speed | Cost |
|--------|------|---------------|---------------------|-------|------|
| LLM (GPT-4o) | Cloud API | High | High | Slow | Expensive |
| LLM (Gemini Flash Lite) | Cloud API | Medium | Medium | Fast | $0.10/M |
| LLM (Haiku) via proxy | Cloud/Sub | Medium-High | Medium-High | Fast | Sub cost |
| GLiNER2 | Local (CPU) | High (entities) | None (entity-only) | Fast | Free |
| Two-pass: GLiNER2 + LLM | Hybrid | High | Medium | Medium | Cheap |
| REBEL (HF) | Local | Medium | Medium (closed set) | Fast | Free |
| UniRel (HF) | Local | Medium | Medium | Fast | Free |
| Fine-tuned BERT NER | Local | Domain-specific | None | Fast | Free (training cost) |

**Test fixture**: Set of chunks with human-annotated entities and relationships. Measure: precision, recall, F1 for entities. Relationship accuracy. Entity name consistency (does it produce "Rust" vs "Rust programming language" vs "rust-lang" for the same thing?).

**Key question**: How much does extraction quality matter vs post-extraction normalization (HDBSCAN + resolver)?  If the resolver can clean up noisy extraction, a fast/cheap extractor wins.

---

### 8. Landscape

**Trait**: `analyze_landscape` — assess extraction confidence, decide extraction method.

**Current**: Cosine similarity analysis, model calibration, extraction method selection.

**Options**: Tightly coupled to extraction — evolves with #42.

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

**Key question**: What's the false merge rate? A false merge (combining two different entities) is worse than a false split (keeping duplicates).

---

## Evaluation Framework

Each stage needs:
1. **Fixture inputs** — representative samples from our actual corpus
2. **Expected outputs** — human-annotated or high-quality-model-generated gold standard
3. **Metrics** — stage-appropriate (see above)
4. **Comparison harness** — run N providers on same fixtures, compare metrics
5. **Integration test** — does a change in stage K improve end-to-end search quality?

The eval harness (`covalence-eval`) already exists with RAGAS metrics. We need to extend it with per-stage benchmarks.

## Priority Order

1. **Embed** — biggest impact on search quality, most expensive ongoing cost
2. **Extract** — biggest impact on graph quality, second most expensive
3. **Convert** — enables #30 (byte offsets), already in progress (#41)
4. **Chunk** — affects both search and extraction, under-explored
5. **Resolve** — already sophisticated, tune thresholds with data

## Related Issues
- #30 — Byte offset chunks (depends on deterministic conversion)
- #41 — ReaderLM-v2 converter
- #42 — Extraction alternatives research
- #4 — Layer-by-layer evaluation harness (closed, but extend it)
- #11 — Fine-tune relationship extraction
