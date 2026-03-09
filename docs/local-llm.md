# Local LLM Quickstart

Run Covalence entirely on local hardware with no cloud API
calls. This guide covers Ollama and vLLM as the two most common
self-hosted backends.

See [docs/providers.md](providers.md) for the full configuration
reference.

## Ollama

### 1. Install and pull models

```bash
# macOS
brew install ollama
ollama serve  # runs on :11434 by default

# Pull an embedding model
ollama pull nomic-embed-text     # 768 dims, good baseline

# Pull an extraction/chat model (pick one)
ollama pull llama3.1:8b          # best quality/speed tradeoff
ollama pull mistral:7b           # lighter alternative
ollama pull gemma2:9b            # Google's option
```

### 2. Configure `.env`

```bash
DATABASE_URL=postgres://covalence:covalence@localhost:5435/covalence_dev
BIND_ADDR=0.0.0.0:8431

# --- Embeddings (Ollama) ---
OPENAI_API_KEY=ollama
OPENAI_BASE_URL=http://localhost:11434/v1
COVALENCE_EMBED_MODEL=nomic-embed-text

# Ollama does NOT support the `dimensions` API parameter.
# Set every dimension to the model's native output size.
COVALENCE_EMBED_DIM_SOURCE=768
COVALENCE_EMBED_DIM_CHUNK=768
COVALENCE_EMBED_DIM_ARTICLE=768
COVALENCE_EMBED_DIM_NODE=768
COVALENCE_EMBED_DIM_ALIAS=768

# --- Chat / Extraction (Ollama) ---
COVALENCE_CHAT_MODEL=llama3.1:8b
COVALENCE_CHAT_API_KEY=ollama
COVALENCE_CHAT_BASE_URL=http://localhost:11434/v1

# Lower concurrency — local inference is slower
COVALENCE_EXTRACT_CONCURRENCY=2
```

> **Why set all `DIM_*` vars to 768?** Cloud models like OpenAI
> `text-embedding-3-large` support Matryoshka embeddings: Covalence
> requests the max dimension and truncates per table. Local models
> output a fixed-size vector and ignore the `dimensions` parameter
> entirely. Setting every table to the model's native dim avoids a
> dimension mismatch at insert time.

### 3. Migrate and run

```bash
make dev-db       # create PG on port 5435
make migrate      # run migrations (sets vector column sizes)
make run          # start the engine on port 8431
```

If you previously ran with different dimensions, you need to
resize the vector columns. Migration 007 handles this based on
the `COVALENCE_EMBED_DIM_*` values, or you can manually alter:

```sql
ALTER TABLE chunks
  ALTER COLUMN embedding TYPE halfvec(768);
```

### Model recommendations

| Use case | Model | Notes |
|---|---|---|
| Embeddings | `nomic-embed-text` | 768 dims, solid recall |
| Embeddings | `mxbai-embed-large` | 1024 dims, higher quality |
| Extraction | `llama3.1:8b` | Best structured output |
| Extraction | `mistral:7b` | Faster, slightly less accurate |

When using `mxbai-embed-large`, update all `DIM_*` vars to 1024.

---

## vLLM

[vLLM](https://docs.vllm.ai/) serves HuggingFace models behind
an OpenAI-compatible API. It is faster than Ollama for
throughput-heavy workloads (batch ingestion) thanks to continuous
batching and PagedAttention.

### 1. Serve a chat model

```bash
pip install vllm

# Serve a chat/extraction model
vllm serve meta-llama/Meta-Llama-3.1-8B-Instruct \
  --host 0.0.0.0 \
  --port 8000 \
  --max-model-len 8192
```

### 2. Serve an embedding model

vLLM can also serve embedding models. Run it on a separate port:

```bash
vllm serve nomic-ai/nomic-embed-text-v1.5 \
  --host 0.0.0.0 \
  --port 8001 \
  --task embedding \
  --max-model-len 8192
```

### 3. Configure `.env`

```bash
DATABASE_URL=postgres://covalence:covalence@localhost:5435/covalence_dev
BIND_ADDR=0.0.0.0:8431

# --- Embeddings (vLLM) ---
OPENAI_API_KEY=unused
OPENAI_BASE_URL=http://localhost:8001/v1
COVALENCE_EMBED_MODEL=nomic-ai/nomic-embed-text-v1.5

# Must match model's native output dims
COVALENCE_EMBED_DIM_SOURCE=768
COVALENCE_EMBED_DIM_CHUNK=768
COVALENCE_EMBED_DIM_ARTICLE=768
COVALENCE_EMBED_DIM_NODE=768
COVALENCE_EMBED_DIM_ALIAS=768

# --- Chat / Extraction (vLLM) ---
COVALENCE_CHAT_MODEL=meta-llama/Meta-Llama-3.1-8B-Instruct
COVALENCE_CHAT_API_KEY=unused
COVALENCE_CHAT_BASE_URL=http://localhost:8000/v1

# vLLM handles batching internally; higher concurrency is fine
COVALENCE_EXTRACT_CONCURRENCY=8
```

### Multi-GPU / Tensor Parallel

```bash
vllm serve meta-llama/Meta-Llama-3.1-70B-Instruct \
  --tensor-parallel-size 4 \
  --host 0.0.0.0 \
  --port 8000
```

---

## Hybrid: local embeddings + cloud extraction

You can mix providers. A common setup uses Ollama for embeddings
(free, fast for small batches) and a cloud LLM for extraction
(better structured output quality):

```bash
# Embeddings via Ollama
OPENAI_API_KEY=ollama
OPENAI_BASE_URL=http://localhost:11434/v1
COVALENCE_EMBED_MODEL=nomic-embed-text
COVALENCE_EMBED_DIM_SOURCE=768
COVALENCE_EMBED_DIM_CHUNK=768
COVALENCE_EMBED_DIM_ARTICLE=768
COVALENCE_EMBED_DIM_NODE=768
COVALENCE_EMBED_DIM_ALIAS=768

# Extraction via OpenAI
COVALENCE_CHAT_MODEL=gpt-4o
COVALENCE_CHAT_API_KEY=sk-...
COVALENCE_CHAT_BASE_URL=https://api.openai.com/v1
```

---

## Limitations

**No `dimensions` parameter.** Most local embedding models
output a fixed-size vector and silently ignore the `dimensions`
field in the API request. You must set all `COVALENCE_EMBED_DIM_*`
variables to the model's native output size. Using Covalence's
default per-table dimensions (2048/1024/256) with a 768-dim
model will cause insertion failures.

**No batch API.** Neither Ollama nor vLLM supports the OpenAI
batch API. Embeddings are processed via the standard
`/v1/embeddings` endpoint. For large ingestion jobs, vLLM's
continuous batching provides significantly better throughput than
Ollama.

**Embedding quality varies.** Local embedding models
(nomic-embed-text, mxbai-embed-large) produce lower-quality
vectors than cloud models like `text-embedding-3-large` or
`voyage-3-large`. Expect lower recall on semantic search,
especially for short queries. Graph-based and keyword search
dimensions are unaffected.

**Extraction quality varies.** Smaller chat models (7B-8B) may
produce less reliable structured JSON output during entity
extraction. If extraction quality is poor, consider:
- Using the GLiNER2 sidecar instead (`COVALENCE_ENTITY_EXTRACTOR=gliner2`)
- Using a cloud LLM for extraction only (hybrid setup above)
- Using a larger local model (70B) if hardware permits

**Memory requirements.** Rough GPU VRAM estimates:
- Embedding (nomic-embed-text): ~1 GB
- Chat 7-8B (q4 quantized): ~5 GB
- Chat 70B (q4 quantized): ~40 GB (multi-GPU recommended)
