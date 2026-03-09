# Provider Matrix

Covalence uses external providers for embeddings and (optionally) LLM-based entity extraction. This document maps out which providers are supported, what they're used for, and how to configure them.

## Capabilities

| Capability | Provider Options | Required? | Config |
|---|---|---|---|
| Embeddings | OpenAI, Voyage AI, Jina, any OpenAI-compatible | **Yes** | `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `COVALENCE_EMBED_MODEL` |
| Entity extraction (LLM) | Any OpenAI-compatible chat API | No | `COVALENCE_CHAT_MODEL`, `COVALENCE_CHAT_API_KEY` |
| Entity extraction (local) | GLiNER2 sidecar | No | `COVALENCE_ENTITY_EXTRACTOR=gliner2` |
| Reranking | Not yet implemented | No | — |
| Article compilation | Not yet implemented | No | — |

**Minimum viable deployment:** PostgreSQL + one embedding provider. Entity extraction is optional but strongly recommended for graph construction.

## Provider Feature Matrix

| Feature | OpenAI | Voyage AI | Jina | Ollama |
|---|---|---|---|---|
| Dimensionality control (`output_dimension` / `dimensions`) | text-embedding-3-* only | voyage-3-large, voyage-4-large | v3 only | No |
| Late chunking (contextual) | No | voyage-context-3 (auto-activated) | jina-embeddings-v3 | No |
| Matryoshka embeddings | text-embedding-3-* | voyage-3-large, voyage-4-large | jina-embeddings-v3 | No |
| Batch API | Yes | Yes | Yes | No |
| Chat/extraction | Yes (gpt-4o, etc.) | No | No | Yes (llama3, etc.) |

### Dimensionality control

Covalence supports **per-table embedding dimensions** to optimize quality vs. storage/performance at each granularity. Models with Matryoshka embedding support (OpenAI `text-embedding-3-*`, Jina v3) allow truncating the output vector to fewer dimensions. Covalence requests embeddings at the maximum configured dimension and truncates + renormalizes per table.

| Model | Native dims | Recommended config |
|---|---|---|
| `text-embedding-3-large` | 3072 | `SOURCE=2048, CHUNK=1024, NODE=256` (defaults) |
| `text-embedding-3-small` | 1536 | `SOURCE=1024, CHUNK=512, NODE=256` |
| `voyage-3` | 1024 | `SOURCE=1024, CHUNK=1024, NODE=256` |
| `voyage-context-3` | 1024 | `SOURCE=1024, CHUNK=1024, NODE=256` |
| `jina-embeddings-v3` | 1024 | `SOURCE=1024, CHUNK=1024, NODE=256` |
| Ollama (nomic-embed-text) | 768 | `SOURCE=768, CHUNK=768, NODE=256` |

**Default per-table dimensions:**

| Table | Dimension | Rationale |
|---|---|---|
| `sources` | 2048 | Fewest records, richest content — max fidelity is cheap |
| `chunks` | 1024 | Most records, searched frequently — good quality/performance balance |
| `articles` | 1024 | Searched alongside chunks, should match |
| `nodes` | 256 | Short text (name + description), used in resolution lookups |
| `node_aliases` | 256 | Must match nodes for cosine comparisons |

**Important:** Per-table dimensions must match the DB column dimensions. After changing, run migration 007 or manually `ALTER TABLE ... ALTER COLUMN embedding TYPE halfvec(N)` on each table. The legacy `COVALENCE_EMBED_DIM` env var is still supported as the fallback for chunk and article dimensions.

### Late chunking (contextual embeddings)

When the configured model supports it (`voyage-context-3`), Covalence automatically uses the Voyage `/contextualizedembeddings` endpoint for chunk embeddings. All chunks from a single document are sent together, and each chunk's embedding reflects the surrounding document context — preventing the "orphan chunk" problem where a chunk about "it" loses the referent.

Late chunking is auto-activated: if the model name contains `context`, the ingestion pipeline calls `embed_document_chunks()` which routes to the contextual endpoint. Source-level, node-level, and query embeddings always use the standard endpoint.

## Per-Cloud Quickstart

### OpenAI (simplest)

Embeddings + chat extraction from one provider.

```bash
OPENAI_API_KEY=sk-...
COVALENCE_EMBED_MODEL=text-embedding-3-large
COVALENCE_EMBED_DIM=1024
COVALENCE_CHAT_MODEL=gpt-4o
# OPENAI_BASE_URL not needed — defaults to https://api.openai.com/v1
```

### Voyage AI (recommended — embeddings + reranking) + OpenAI (extraction)

Voyage for high-quality embeddings with automatic reranking, OpenAI for chat extraction.

```bash
VOYAGE_API_KEY=pa-...              # Voyage API key (native provider)
COVALENCE_EMBED_MODEL=voyage-3-large
# COVALENCE_EMBED_PROVIDER=voyage  # Auto-detected when VOYAGE_API_KEY is set

COVALENCE_CHAT_MODEL=gpt-4o
COVALENCE_CHAT_API_KEY=sk-...      # Separate OpenAI key for chat
COVALENCE_CHAT_BASE_URL=https://api.openai.com/v1
```

When `VOYAGE_API_KEY` is set (or `COVALENCE_EMBED_PROVIDER=voyage`), Covalence automatically:
- Uses the native Voyage AI embedder with `input_type` hints
- Activates the Voyage `rerank-2.5` reranker in the search pipeline

### AWS Bedrock

Use an OpenAI-compatible proxy (e.g. LiteLLM) in front of Bedrock.

```bash
OPENAI_BASE_URL=http://localhost:4000/v1  # LiteLLM proxy
OPENAI_API_KEY=dummy                       # Proxy handles auth
COVALENCE_EMBED_MODEL=amazon.titan-embed-text-v2:0
COVALENCE_EMBED_DIM=1024

COVALENCE_CHAT_MODEL=anthropic.claude-3-5-sonnet-20241022-v2:0
COVALENCE_CHAT_API_KEY=dummy
COVALENCE_CHAT_BASE_URL=http://localhost:4000/v1
```

### Google Vertex AI

Use an OpenAI-compatible proxy (e.g. LiteLLM) in front of Vertex.

```bash
OPENAI_BASE_URL=http://localhost:4000/v1
OPENAI_API_KEY=dummy
COVALENCE_EMBED_MODEL=text-embedding-005
COVALENCE_EMBED_DIM=768

COVALENCE_CHAT_MODEL=gemini-2.0-flash
COVALENCE_CHAT_API_KEY=dummy
COVALENCE_CHAT_BASE_URL=http://localhost:4000/v1
```

### Self-Hosted (Ollama)

```bash
OPENAI_BASE_URL=http://localhost:11434/v1
OPENAI_API_KEY=ollama                      # Ollama ignores this but it must be set
COVALENCE_EMBED_MODEL=nomic-embed-text
COVALENCE_EMBED_DIM=768

COVALENCE_CHAT_MODEL=llama3.1:8b
COVALENCE_CHAT_API_KEY=ollama
COVALENCE_CHAT_BASE_URL=http://localhost:11434/v1
```

**Limitations:** No `dimensions` param support (set `COVALENCE_EMBED_DIM` to the model's native output). Embedding quality varies significantly by model. No batch API — embeddings are processed sequentially.

### GLiNER2 Sidecar (local entity extraction)

Use instead of LLM extraction for faster, cheaper entity extraction without API calls.

```bash
COVALENCE_ENTITY_EXTRACTOR=gliner2
COVALENCE_EXTRACT_URL=http://localhost:8432
COVALENCE_GLINER_THRESHOLD=0.5
COVALENCE_CHAT_MODEL=               # Leave empty to disable LLM extraction
```

## Full Configuration Reference

### Required

| Variable | Description | Default |
|---|---|---|
| `DATABASE_URL` | PostgreSQL connection string | _(none — required)_ |

### Server

| Variable | Description | Default |
|---|---|---|
| `BIND_ADDR` | HTTP server bind address | `0.0.0.0:8431` |
| `COVALENCE_API_KEY` | API key for request authentication | _(none — no auth)_ |

### Embedding Provider

| Variable | Description | Default |
|---|---|---|
| `COVALENCE_EMBED_PROVIDER` | Embedding provider: `openai` or `voyage` | `openai` (auto-detected if `VOYAGE_API_KEY` set) |
| `OPENAI_API_KEY` | API key for embedding provider (OpenAI mode) | _(none — embeddings disabled)_ |
| `OPENAI_BASE_URL` | Base URL for embedding API (OpenAI mode) | `https://api.openai.com/v1` |
| `COVALENCE_EMBED_MODEL` | Embedding model name | `text-embedding-3-large` |
| `COVALENCE_EMBED_DIM` | Legacy: fallback dimension for chunk/article | `1024` |
| `COVALENCE_EMBED_DIM_SOURCE` | Source-level embedding dimensions | `2048` |
| `COVALENCE_EMBED_DIM_CHUNK` | Chunk-level embedding dimensions | `1024` (falls back to `COVALENCE_EMBED_DIM`) |
| `COVALENCE_EMBED_DIM_ARTICLE` | Article-level embedding dimensions | `1024` (falls back to `COVALENCE_EMBED_DIM`) |
| `COVALENCE_EMBED_DIM_NODE` | Node-level embedding dimensions | `256` (falls back to `COVALENCE_NODE_EMBED_DIM`) |
| `COVALENCE_EMBED_DIM_ALIAS` | Alias-level embedding dimensions | `256` (falls back to `COVALENCE_NODE_EMBED_DIM`) |
| `COVALENCE_EMBED_BATCH` | Max texts per API call | `64` |
| `COVALENCE_NODE_EMBED_DIM` | Legacy: fallback dimension for node/alias | `256` |

### Chat / Entity Extraction

| Variable | Description | Default |
|---|---|---|
| `COVALENCE_CHAT_MODEL` | LLM model for entity extraction | `gpt-4o` |
| `COVALENCE_CHAT_API_KEY` | API key for chat (falls back to `OPENAI_API_KEY`) | _(fallback)_ |
| `COVALENCE_CHAT_BASE_URL` | Base URL for chat (does **not** fall back to `OPENAI_BASE_URL`) | _(none — uses OpenAI default)_ |
| `COVALENCE_ENTITY_EXTRACTOR` | Extractor backend: `llm` or `gliner2` | `llm` |
| `COVALENCE_EXTRACT_URL` | GLiNER2 sidecar URL | `http://localhost:8432` |
| `COVALENCE_GLINER_THRESHOLD` | GLiNER2 confidence threshold (0.0–1.0) | `0.5` |
| `COVALENCE_EXTRACT_CONCURRENCY` | Max concurrent extraction calls | `8` |

### Entity Resolution

| Variable | Description | Default |
|---|---|---|
| `COVALENCE_RESOLVE_TRIGRAM_THRESHOLD` | Trigram similarity threshold (0.0–1.0) | `0.4` |
| `COVALENCE_RESOLVE_VECTOR_THRESHOLD` | Vector cosine similarity threshold (0.0–1.0) | `0.85` |

### Ingestion

| Variable | Description | Default |
|---|---|---|
| `COVALENCE_CHUNK_SIZE` | Max chunk size in bytes | `1000` |
| `COVALENCE_CHUNK_OVERLAP` | Overlap between chunks in characters | `200` |

### Consolidation

| Variable | Description | Default |
|---|---|---|
| `COVALENCE_BATCH_INTERVAL` | Batch consolidation interval (seconds) | `300` |
| `COVALENCE_DEEP_INTERVAL` | Deep consolidation interval (seconds) | `86400` |
| `COVALENCE_DELTA_THRESHOLD` | Epistemic delta to trigger recompilation | `0.1` |

### Search

| Variable | Description | Default |
|---|---|---|
| `COVALENCE_RRF_K` | Reciprocal Rank Fusion k parameter | `60.0` |
| `COVALENCE_DEFAULT_LIMIT` | Default search result limit | `10` |

### Voyage AI

| Variable | Description | Default |
|---|---|---|
| `VOYAGE_API_KEY` | Voyage API key (auto-activates Voyage provider + reranker) | _(none)_ |
| `VOYAGE_BASE_URL` | Voyage base URL | `https://api.voyageai.com/v1` |

## End-to-End Test Flow

Reset the database, ingest a document, and verify search results:

```bash
# 1. Reset the dev database (drops + recreates + runs migrations)
make reset-db

# 2. Start the engine
make run

# 3. In another terminal — ingest a test document
cove source add --title "Test" --content "Albert Einstein developed the theory of general relativity. The theory describes gravity as the curvature of spacetime caused by mass and energy."

# 4. Verify chunks were created with embeddings
cove source list --json | jq '.[0].id'
# Use the source ID to check chunks:
curl -s http://localhost:8431/sources/<id>/chunks | jq '.[].embedding | length'

# 5. Search and verify results
cove search "general relativity"
cove search "who developed the theory of gravity"

# 6. Check node extraction
cove node resolve "Albert Einstein"
```

When using Voyage `voyage-context-3`, chunk embeddings are context-aware — a search for "the theory" should rank chunks about relativity higher than it would with independent chunk embeddings.
