# Covalence

**A knowledge engine for AI agents** — PostgreSQL-backed persistent memory with pgvector semantic search, Apache AGE graph traversal, and a full epistemic model (confidence scoring, contention detection, provenance tracking). Covalence stores what agents learn as a living knowledge graph: raw **sources** are compiled into curated **articles**, relationships are typed **edges**, and every claim carries lineage back to its origin. Successor to Valence.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│           AI Agents / LLM Orchestrators          │
└──────────────────────┬──────────────────────────┘
                       │  MCP tools / function calls
┌──────────────────────▼──────────────────────────┐
│           OpenClaw Plugin  (TypeScript)          │
│   auto-recall · auto-capture · session ingest   │
└──────────────────────┬──────────────────────────┘
                       │  HTTP REST
┌──────────────────────▼──────────────────────────┐
│        Covalence Engine  (Rust / Actix-web)      │
│  sources · articles · search · memory · admin   │
│  slow-path queue → compile / embed / infer      │
└──────────────────────┬──────────────────────────┘
                       │  sqlx / pgvector / AGE
┌──────────────────────▼──────────────────────────┐
│            PostgreSQL 17  (custom image)         │
│   pgvector (HNSW halfvec)  ·  Apache AGE graph  │
│   full-text search  ·  JSONB metadata            │
└─────────────────────────────────────────────────┘
```

---

## Quick Start

### One-command setup (Docker)

```bash
git clone https://github.com/ourochronos/covalence.git
cd covalence
docker compose up -d
curl http://localhost:8430/health
```

`docker compose up` builds and starts **both** the PostgreSQL 17 database (with pgvector + Apache AGE) and the Covalence engine.  Migrations in `sql/` are applied automatically on first boot before the engine accepts traffic.

> **Optional LLM support** — create a `.env` from the example and add your
> OpenAI key to enable embeddings and article compilation:
> ```bash
> cp .env.example .env
> # edit .env — set OPENAI_API_KEY (and optionally OPENAI_BASE_URL)
> docker compose up -d
> ```

### Manual / local development setup

#### 1 — Start the database

```bash
docker compose up -d postgres
```

The custom image (`./docker/Dockerfile`) bundles PostgreSQL 17, pgvector, and Apache AGE. Data is persisted in the `covalence-data` volume. The DB is exposed on **port 5434** (`localhost:5434/covalence`, user/pass: `covalence`).

#### 2 — Apply migrations

```bash
for f in sql/*.sql; do
    psql postgres://covalence:covalence@localhost:5434/covalence -f "$f"
done
```

#### 3 — Register historical migrations with SQLx (existing instances only)

The engine uses `sqlx::migrate!()` to auto-apply new migrations on startup
(tracking#106).  Migrations 001–017 were applied before this feature existed,
so they must be registered in the `_sqlx_migrations` tracking table once:

```bash
DATABASE_URL=postgres://covalence:covalence@localhost:5434/covalence \
  ./scripts/seed-sqlx-migrations.sh
```

This script is **idempotent** — safe to re-run.  Skip this step on a **fresh**
database (the engine handles everything automatically from a clean slate).

#### 4 — Build and run the engine

```bash
cd engine
cargo build --release
cargo run --release
```

The engine auto-applies any unapplied migrations in `engine/migrations/` on
every startup before accepting traffic.

The engine listens on **http://localhost:8430** by default.

### Adding new database migrations

New migrations live in **`engine/migrations/`** and are managed by
[SQLx](https://github.com/launchbadger/sqlx).

**Naming convention** — files must follow the pattern:

```
{version}_{description}.sql
```

where `{version}` is an integer **greater than 17** (the last manually-applied
migration).  Example:

```
engine/migrations/018_add_tags_table.sql
```

The migration is automatically applied the next time the engine starts.  No
manual `psql` invocation is needed.

**Keeping `sql/` as the historical record** — files in `sql/001_*.sql` through
`sql/017_*.sql` are the immutable audit trail of the pre-SQLx era.  Do **not**
delete or modify them.  New migrations go only in `engine/migrations/`.

### Environment variables

See **`.env.example`** for the full reference. Key variables:

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | ✅ | `postgres://covalence:covalence@localhost:5434/covalence` | PostgreSQL connection string |
| `OPENAI_API_KEY` | optional | — | Enables embeddings (`text-embedding-3-small`) and LLM compilation |
| `OPENAI_BASE_URL` | optional | `https://api.openai.com/v1` | Override for compatible proxies / local models |
| `COVALENCE_EMBED_MODEL` | optional | `text-embedding-3-small` | Embedding model name |
| `INFERENCE_URL` | optional | — | OpenAI-compatible proxy URL for the OpenClaw plugin |
| `COVALENCE_API_KEY` | optional | — | When set, all requests must include `Authorization: Bearer <key>` or `X-Api-Key: <key>`. Leave unset for unauthenticated dev mode. `GET /health` is always exempt. |
| `BIND_ADDR` | optional | `0.0.0.0:8430` | HTTP listen address |
| `RUST_LOG` | optional | `covalence_engine=debug` | Tracing log filter |

---

## API Reference

All endpoints return `{ "data": … }` JSON envelopes.

### Sources

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sources` | Ingest a new raw source |
| `GET` | `/sources` | List sources |
| `GET` | `/sources/{id}` | Get source by ID |
| `DELETE` | `/sources/{id}` | Delete source (cascades to article provenance) |

### Articles

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/articles` | Create an article manually |
| `POST` | `/articles/compile` | Compile sources → article via LLM (async, `202`) |
| `POST` | `/articles/merge` | Merge two articles into one |
| `GET` | `/articles` | List articles (`?limit=`, `?cursor=`, `?status=`) |
| `GET` | `/articles/{id}` | Get article by ID |
| `PATCH` | `/articles/{id}` | Update article content |
| `DELETE` | `/articles/{id}` | Archive / delete article |
| `POST` | `/articles/{id}/split` | Split oversized article into two |
| `GET` | `/articles/{id}/provenance` | Trace provenance graph for an article |

### Search

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/search` | Hybrid search: vector + full-text + graph, RRF-fused |

### Graph (Nodes & Edges)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/edges` | Create a typed edge between two nodes |
| `DELETE` | `/edges/{id}` | Remove an edge |
| `GET` | `/nodes/{id}/edges` | List edges for a node (`?direction=`, `?labels=`) |
| `GET` | `/nodes/{id}/neighborhood` | Walk the graph neighborhood |

### Contentions

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/contentions` | List contentions (`?node_id=`, `?status=`) |
| `GET` | `/contentions/{id}` | Get contention detail |
| `POST` | `/contentions/{id}/resolve` | Resolve (`supersede_a`, `supersede_b`, `accept_both`, `dismiss`) |

### Memory

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/memory` | Store a tagged observation memory |
| `POST` | `/memory/search` | Recall memories by natural-language query |
| `GET` | `/memory/status` | Memory system statistics |
| `PATCH` | `/memory/{id}/forget` | Soft-delete a memory |

### Sessions

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/sessions` | Create a session |
| `GET` | `/sessions` | List sessions |
| `GET` | `/sessions/{id}` | Get session |
| `POST` | `/sessions/{id}/close` | Close a session |

### Admin

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/admin/stats` | Knowledge base health and capacity stats |
| `POST` | `/admin/maintenance` | Trigger maintenance (recompile, decay, evict) |
| `GET` | `/admin/queue` | List slow-path queue entries |
| `GET` | `/admin/queue/{id}` | Get queue entry |
| `POST` | `/admin/queue/{id}/retry` | Retry a failed queue entry |
| `DELETE` | `/admin/queue/{id}` | Remove a queue entry |
| `POST` | `/admin/embed-all` | Queue embeddings for all un-embedded nodes |
| `POST` | `/admin/tree-index-all` | Queue hierarchical tree-index for eligible nodes |

---

## Python Client (auto-generated)

A typed Python client is auto-generated from the OpenAPI spec and committed to
`clients/python/`. It covers all five API tag groups: **sources**, **articles**,
**search**, **memory**, and **admin**.

### Install

```bash
pip install -e clients/python
```

### Quick usage

```python
import covalence
from covalence.api import SourcesApi, SearchApi, MemoryApi
from covalence.models import IngestRequest, SearchRequest, StoreMemoryRequest

cfg = covalence.Configuration(host="http://localhost:8430")

with covalence.ApiClient(cfg) as client:
    # Ingest a source
    src = SourcesApi(client).ingest_source(
        IngestRequest(
            content="Covalence stores epistemic knowledge.",
            metadata={},
            source_type="observation",
        )
    )
    print(src.id)

    # Semantic search
    results = SearchApi(client).search(
        SearchRequest(query="epistemic knowledge", limit=5)
    )
    for r in results:
        print(r.score, r.content_preview)
```

### Regenerate after spec changes

The spec lives at `openapi.json` (repo root) and is exported from the live engine.
Regeneration requires Docker (no Java needed on the host):

```bash
# Pull a fresh spec from the running engine and regenerate
make client-fetch

# …or regenerate from the committed spec without starting the engine
make client

# Validate that the committed spec parses as JSON (no engine needed)
make spec
```

**CI gate**: `make client-check` (or the `openapi-client` GitHub Actions job) re-generates
the client and fails the build if any `.py` file differs from what's committed.
This ensures the client always reflects the current API. See [`scripts/generate-client.sh`](scripts/generate-client.sh)
and [`.github/workflows/ci.yml`](.github/workflows/ci.yml) for details.

---

## OpenClaw Plugin

Covalence ships a first-party [OpenClaw](https://openclaw.ai) plugin that exposes all engine tools to agents and handles automatic recall/capture lifecycle.

### Install

```bash
cd plugin
npm install
npm run build
```

Point OpenClaw at the built plugin or reference the `plugin/` directory directly. The plugin registers as `memory-covalence`.

### Key configuration options

| Option | Default | Description |
|---|---|---|
| `serverUrl` | `http://localhost:8430` | Covalence engine URL |
| `authToken` | — | Bearer token (or `$COVALENCE_AUTH_TOKEN`) |
| `autoRecall` | `true` | Inject relevant memories before each agent run |
| `autoCapture` | `true` | Extract insights from conversations |
| `sessionIngestion` | `true` | Persist conversation transcripts as sources |
| `autoCompileOnFlush` | `true` | Trigger queue processing on session flush |
| `inferenceEnabled` | `true` | Proxy OpenAI-compatible `/chat/completions` + `/embeddings` |
| `inferenceModel` | — | Provider/model for compilation (e.g. `github-copilot/gpt-4.1-mini`) |

---

## Epistemic Model

Covalence models knowledge quality explicitly rather than treating all stored text as equally trustworthy.

### Confidence

Every node carries a `confidence` score (0–1), initialized by source type and decayed as knowledge ages or is superseded. Search ranking:

```
score = relevance × 0.5 + confidence × 0.35 + freshness × 0.15
```

### Provenance

Articles are compiled **from** sources. Each link carries a relationship type:

| Relationship | Meaning |
|---|---|
| `originates` | Source is the primary origin of the article |
| `confirms` | Source corroborates an existing claim |
| `supersedes` | Source replaces prior content |
| `contradicts` | Source directly opposes the article |
| `contends` | Source partially challenges the article |

The `/articles/{id}/provenance` endpoint traverses this graph. Claim-level attribution uses TF-IDF similarity to rank which sources most contributed a specific sentence.

### Contentions

When an incoming source contradicts an existing article, a **contention** is automatically detected with severity and materiality scores. Contentions surface for explicit resolution — knowledge is never silently overwritten. Resolution options: `supersede_a` (article wins), `supersede_b` (source wins), `accept_both`, `dismiss`.

### Node lifecycle

```
active → superseded | archived | disputed | tombstone
```

Tombstones preserve audit continuity on delete. The async slow-path queue handles: `compile`, `embed`, `split`, `merge`, `infer_edges`, `contention_check`, `tree_index`, `tree_embed`.

---

## Current Status

**Active development — core engine functional.**

| Component | Status |
|---|---|
| PostgreSQL schema (nodes, edges, contentions, embeddings, sessions, queue) | ✅ |
| Rust/Actix engine with full REST API | ✅ |
| pgvector HNSW embeddings (`halfvec(1536)`) | ✅ |
| Apache AGE graph backend + neighborhood traversal | ✅ |
| Hybrid search (vector + FTS + graph, RRF fusion) | ✅ |
| Slow-path async queue | ✅ |
| OpenClaw plugin (auto-recall, capture, session ingestion, inference proxy) | ✅ |
| Epistemic model (confidence, provenance, contentions) | ✅ |
| Automatic edge inference | 🔄 in progress |
| Decay + organic eviction tuning | 🔄 in progress |
| OpenAPI spec / Swagger UI | 📋 planned |

---

## License

MIT
