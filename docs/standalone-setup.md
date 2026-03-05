# Covalence Standalone Setup

> **Goal:** Run Covalence in your own project without OpenClaw — direct API key, your own
> Docker stack, your own agent code.

This guide is for the **"I just want a persistent knowledge engine for my agent"** use case.
No OpenClaw plugin, no inference proxy, no MCP — just a Rust HTTP server backed by
PostgreSQL and your OpenAI key.

---

## Table of Contents

1. [Quick Start](#1-quick-start)
2. [Environment Variables](#2-environment-variables)
3. [Docker Setup](#3-docker-setup)
4. [Running Migrations Manually](#4-running-migrations-manually)
5. [Python Client](#5-python-client)
6. [CLI Usage](#6-cli-usage)
7. [Basic Workflow](#7-basic-workflow)
8. [What You Give Up Without OpenClaw](#8-what-you-give-up-without-openclaw)

---

## 1. Quick Start

```bash
git clone https://github.com/ourochronos/covalence.git
cd covalence

cp .env.example .env
# Set your API key — the only required edit for LLM features:
echo 'OPENAI_API_KEY=sk-...' >> .env

docker compose up -d
curl http://localhost:8430/health
```

Engine → `http://localhost:8430` | PostgreSQL → `localhost:5434`

Migrations are applied automatically on first boot by the engine container's entrypoint.

---

## 2. Environment Variables

The engine reads its config from environment variables (or a `.env` file via `dotenvy`).
This table covers everything relevant to standalone operation.

### Core variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | ✓ | — | PostgreSQL connection string |
| `OPENAI_API_KEY` | ✗* | — | Enables embeddings and LLM compilation |
| `BIND_ADDR` | ✗ | `0.0.0.0:8430` | Interface and port to bind |
| `RUST_LOG` | ✗ | `covalence_engine=debug,tower_http=debug` | Log filter |

*Without `OPENAI_API_KEY`, the engine starts fine but vector search and article compilation
are disabled — only lexical (BM25) search is available.

### LLM / embeddings

| Variable | Default | Description |
|---|---|---|
| `OPENAI_API_KEY` | — | Your OpenAI secret key |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | Override to use a compatible local server (Ollama, LM Studio, Azure OpenAI, etc.) |
| `COVALENCE_EMBED_MODEL` | `text-embedding-3-small` | Embedding model name |

> **Using a local model?** Point `OPENAI_BASE_URL` at your Ollama or LM Studio instance
> and set `COVALENCE_EMBED_MODEL` to your local embedding model name. The engine uses the
> OpenAI-compatible `/embeddings` endpoint, so any compatible server works.

### Database (migration runner only)

Used by `docker/run-migrations.sh` and set automatically by `docker-compose.yml`. Only
needed if you run migrations manually: `DB_HOST`, `DB_PORT`, `DB_NAME`, `DB_USER`,
`DB_PASSWORD`, `SQL_DIR` (`/app/sql`), `ENGINE_BIN`, `MAX_WAIT` (60 s).

### What `INFERENCE_URL` is (and why you don't need it standalone)

The `.env.example` mentions `INFERENCE_URL`. This is consumed by the **OpenClaw plugin**,
not the engine itself. When running standalone, ignore it — set `OPENAI_API_KEY` directly
and the engine will call OpenAI on its own.

---

## 3. Docker Setup

The provided `docker-compose.yml` runs everything: a PostgreSQL 17 image (with `pgvector`
pre-installed) and the Covalence engine.

### Standalone `.env`

Create a `.env` file in the repo root:

```bash
# .env — standalone Covalence (no OpenClaw)

# --- Database -----------------------------------------------------------
# Default matches docker-compose.yml — change only if using an external DB.
DATABASE_URL=postgres://covalence:covalence@postgres:5432/covalence

# --- LLM / Embeddings ---------------------------------------------------
# Set this to enable vector search and article compilation.
OPENAI_API_KEY=sk-...

# Optional overrides:
# OPENAI_BASE_URL=https://api.openai.com/v1
# COVALENCE_EMBED_MODEL=text-embedding-3-small

# --- Engine server ------------------------------------------------------
# BIND_ADDR=0.0.0.0:8430
# RUST_LOG=covalence_engine=info,tower_http=info
```

### Starting the stack

```bash
docker compose up -d           # start postgres + engine
docker compose logs -f engine  # tail engine logs
docker compose down            # stop (data persists in covalence-data volume)
docker compose down -v         # stop AND delete all data
```

### Using an external PostgreSQL

Set `DATABASE_URL` in `.env` to your external DB and run only the engine:

```bash
# .env
DATABASE_URL=postgres://myuser:mypassword@db.example.com:5432/covalence
```

```bash
docker compose up -d engine
```

Your Postgres must have the `pgvector` and `age` extensions available. Schema is created
on first boot via `sql/*.sql`.

**Ports:** PostgreSQL → `localhost:5434` | Engine → `localhost:8430`. Override in
`docker-compose.yml` if these conflict with other services.

---

## 4. Running Migrations Manually

The engine container entrypoint (`docker/run-migrations.sh`) handles migrations
automatically. If you need to run them outside Docker — in CI, against a managed cloud DB,
or during local development — you have two options.

### Option A — `psql` directly

```bash
export PGPASSWORD=covalence

for f in sql/*.sql; do
    echo "Applying $f ..."
    psql -h localhost -p 5434 -U covalence -d covalence -f "$f"
done
```

The migration script uses a `_migration_log` tracking table so already-applied files are
skipped. The table is created automatically by Option A; if you manage migrations with a
different tool, seed it yourself before running your first file.

### Option B — `sqlx-cli` / any psql-compatible tool

```bash
export DATABASE_URL=postgres://covalence:covalence@localhost:5434/covalence
for f in sql/*.sql; do psql "$DATABASE_URL" -f "$f"; done
```

---

## 5. Python Client

### Installation

```bash
pip install covalence-client
```

**Requirements:** Python 3.10+

### Connecting

```python
from covalence import CovalenceClient

# Default: http://localhost:8430
client = CovalenceClient()

# Custom host/port
client = CovalenceClient("http://my-server:8430")
```

Both sync (`CovalenceClient`) and async (`AsyncCovalenceClient`) clients are available —
use the sync client for scripts and notebooks, async for `asyncio`-based agents:

```python
# Sync
with CovalenceClient() as client:
    stats = client.get_admin_stats()
    print(f"Articles: {stats.nodes.articles}")

# Async
import asyncio
from covalence import AsyncCovalenceClient

async def main():
    async with AsyncCovalenceClient() as client:
        stats = await client.get_admin_stats()

asyncio.run(main())
```

---

## 6. CLI Usage

Installing `covalence-client` also provides a `covalence` command-line tool.

### Health and stats

```bash
covalence health        # verify engine is up
covalence stats         # knowledge base counts, queue health, embedding coverage
```

### Ingesting sources

```bash
# From a file
covalence ingest --file ./notes.md --type document --title "My Notes"

# From stdin
cat page.txt | covalence ingest --type web --title "Example Page"

# With reliability override
covalence ingest --file spec.md --type document --title "API Spec" --reliability 0.95
```

### Searching

```bash
# Basic search (all node types)
covalence search "Python asyncio event loop"

# Articles only, with weight override
covalence search "memory safety" --type article --limit 5
covalence search "ownership" --weights '{"vector":0.8,"lexical":0.1,"graph":0.1}'
```

### Article compilation

```bash
# Compile sources into an article — returns a job ID
covalence compile --sources src-id-1,src-id-2,src-id-3 --title "My Topic"

# Check job status
covalence queue --job <job-id>
```

### Maintenance

```bash
covalence maintain --recompute-scores --process-queue --evict
covalence embed-all          # queue embeddings for any nodes missing them
covalence contentions        # list open (unresolved) contentions
```

---

## 7. Basic Workflow

A complete standalone integration — ingest raw material, compile it into a curated article,
search, review contentions, and run maintenance.

```python
import time
from covalence import CovalenceClient

client = CovalenceClient()   # http://localhost:8430

# ── 1. Ingest sources ──────────────────────────────────────────────────────

doc = client.ingest_source(
    "Rust's ownership system guarantees memory safety at compile time "
    "without a garbage collector. Each value has a single owner; when "
    "the owner goes out of scope, the value is dropped automatically.",
    source_type="document",
    title="Rust Ownership — Official Book",
    reliability=0.95,
)

blog = client.ingest_source(
    "Rust uses a modern reference-counted GC to manage memory.",
    source_type="web",
    title="Random Blog Post",
    reliability=0.35,
)

print(f"Ingested: {doc.id}  confidence={doc.confidence:.2f}")

# ── 2. Compile (requires OPENAI_API_KEY, runs async — poll until done) ─────

job = client.compile_article(
    [doc.id],
    title_hint="Rust memory safety and ownership",
)

while True:
    entry = client.get_queue_entry(job.job_id)
    if entry.status == "completed":
        break
    if entry.status == "failed":
        raise RuntimeError(f"Compilation failed: {entry}")
    time.sleep(2)

print(f"Article compiled (job {job.job_id})")

# ── 3. Search ──────────────────────────────────────────────────────────────

results = client.search("how does Rust manage memory", node_types=["article"], limit=5)
for r in results.data:
    print(f"[{r.score:.3f}] {r.title}: {r.content_preview[:100]}")

# ── 4. Review contentions ──────────────────────────────────────────────────
# The low-reliability blog post likely triggered a contention against the article.

from covalence.models import ContentionResolution

for c in client.list_contentions(status="detected").data:
    print(f"[{c.severity}] {c.description}")
    client.resolve_contention(
        c.id,
        resolution=ContentionResolution.supersede_a,   # article wins
        rationale="Blog post has reliability 0.35; official source takes precedence.",
    )

# ── 5. Maintenance (run periodically) ─────────────────────────────────────

client.run_maintenance(recompute_scores=True, process_queue=True, evict_if_over_capacity=True)
```

### Memory shortcut (no LLM needed)

For lightweight agent self-observations, use the Memory API — a thin wrapper around
sources with importance scoring and tag filtering:

```python
mem = client.store_memory(
    "User prefers concise answers.",
    tags=["preferences"],
    importance=0.8,
)

# Recall before each agent turn
results = client.recall_memories("user formatting preferences", tags=["preferences"])
for m in results.data:
    print(f"[{m.confidence:.2f}] {m.content}")

# Supersede stale memory
new_mem = client.store_memory(
    "User prefers bullet points for technical topics.",
    tags=["preferences"],
    importance=0.85,
    supersedes_id=mem.id,
)
client.forget_memory(mem.id, reason="Superseded.")
```

---

## 8. What You Give Up Without OpenClaw

Running standalone gives you the full knowledge engine. The only things that require
OpenClaw are convenience automations built into the plugin:

| Feature | OpenClaw plugin | Standalone equivalent |
|---|---|---|
| Auto-recall before agent runs | ✓ plugin | Call `client.search()` or `client.recall_memories()` yourself |
| Auto-capture from conversation transcripts | ✓ plugin | `client.ingest_source(..., source_type="conversation")` |
| Inference proxy (`INFERENCE_URL`) | ✓ plugin | Set `OPENAI_API_KEY` directly; engine calls OpenAI on its own |
| MCP tool interface | ✓ plugin | Use the Python client or direct HTTP REST to `:8430` |
| Auto-compile on session flush | ✓ plugin | `client.compile_article(...)` + `client.run_maintenance(process_queue=True)` |

Everything else — sources, articles, the graph, search, contentions, provenance, memories,
maintenance — is identical with or without OpenClaw.

---

## Further Reading

- **[Adoption Guide](./adoption-guide.md)** — Full integration patterns, search tuning, best practices
- **[API Reference](./api-reference.md)** — All 38 endpoints with full request/response shapes
- **[README](../README.md)** — Architecture overview and Docker setup
- **[covalence-py](https://github.com/ourochronos/covalence-py)** — Python client full method reference
