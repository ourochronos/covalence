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
| Dimensionality control (`dimensions` param) | text-embedding-3-* only | No | v3 only | No |
| Late chunking | No | voyage-context-3 | jina-embeddings-v3 | No |
| Contextual embeddings | No | voyage-context-3 | No | No |
| Matryoshka embeddings | text-embedding-3-* | No | jina-embeddings-v3 | No |
| Batch API | Yes | Yes | Yes | No |
| Chat/extraction | Yes (gpt-4o, etc.) | No | No | Yes (llama3, etc.) |

### Dimensionality control

Models with Matryoshka embedding support (OpenAI `text-embedding-3-*`, Jina v3) allow truncating the output vector to fewer dimensions via the `dimensions` API parameter. Covalence always sends `COVALENCE_EMBED_DIM` as this parameter. For models that don't support it (Voyage, Ollama), set `COVALENCE_EMBED_DIM` to match the model's native output dimension.

| Model | Native dims | Recommended `COVALENCE_EMBED_DIM` |
|---|---|---|
| `text-embedding-3-large` | 3072 | 1024 (default) |
| `text-embedding-3-small` | 1536 | 512 or 1024 |
| `voyage-3` | 1024 | 1024 |
| `voyage-context-3` | 1024 | 1024 |
| `jina-embeddings-v3` | 1024 | 1024 |
| Ollama (nomic-embed-text) | 768 | 768 |

**Important:** `COVALENCE_EMBED_DIM` must match the DB column dimension. After changing it, run migration 004 or manually `ALTER TABLE ... ALTER COLUMN embedding TYPE halfvec(N)` on all embedding columns.

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

### Voyage AI (embeddings) + OpenAI (extraction)

Voyage for high-quality contextual embeddings, OpenAI for chat extraction.

```bash
OPENAI_API_KEY=voyage-...          # Voyage API key (uses OpenAI-compatible endpoint)
OPENAI_BASE_URL=https://api.voyageai.com/v1
COVALENCE_EMBED_MODEL=voyage-context-3
COVALENCE_EMBED_DIM=1024

COVALENCE_CHAT_MODEL=gpt-4o
COVALENCE_CHAT_API_KEY=sk-...      # Separate OpenAI key for chat
COVALENCE_CHAT_BASE_URL=https://api.openai.com/v1
```

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
| `OPENAI_API_KEY` | API key for embedding provider | _(none — embeddings disabled)_ |
| `OPENAI_BASE_URL` | Base URL for embedding API | `https://api.openai.com/v1` |
| `COVALENCE_EMBED_MODEL` | Embedding model name | `text-embedding-3-large` |
| `COVALENCE_EMBED_DIM` | Output vector dimensions (sent as `dimensions` param) | `1024` |
| `COVALENCE_EMBED_BATCH` | Max texts per API call | `64` |
| `COVALENCE_NODE_EMBED_DIM` | Node-level embedding dimensions | `256` |

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

### Voyage AI (reserved)

| Variable | Description | Default |
|---|---|---|
| `VOYAGE_API_KEY` | Voyage API key (reserved for future use) | _(none)_ |
| `VOYAGE_BASE_URL` | Voyage base URL (reserved for future use) | _(none)_ |
