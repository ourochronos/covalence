# Covalence Engine — API Reference

> **Base URL:** `http://localhost:3000` (default)  
> **Content-Type:** `application/json` for all request/response bodies  
> **Auth:** None at the engine layer (managed at the proxy/plugin layer)

All successful responses are wrapped in a `{"data": ...}` envelope. List responses additionally include `{"data": [...], "meta": {"count": N}}`. Errors follow the shape `{"error": "<message>"}` with an appropriate HTTP status code.

---

## Table of Contents

1. [Sources](#1-sources)
2. [Articles](#2-articles)
3. [Search](#3-search)
4. [Edges / Graph](#4-edges--graph)
5. [Contentions](#5-contentions)
6. [Memory](#6-memory)
7. [Sessions](#7-sessions)
8. [Admin](#8-admin)
9. [Shared Types](#9-shared-types)
10. [Error Reference](#10-error-reference)

---

## 1. Sources

Sources are raw, immutable input material ingested into the knowledge substrate. Each source is hashed (SHA-256) on ingest, making the operation **idempotent** — re-ingesting identical content returns the existing record without creating a duplicate.

After ingest, two slow-path background tasks are automatically enqueued: `embed` (generate a vector embedding) and `contention_check` (detect conflicts with existing articles).

---

### `POST /sources` — Ingest a source

**Description:** Ingest a new source document. Returns `201 Created` with the source object. If a source with the same SHA-256 content fingerprint already exists, returns the existing record (also `201`).

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `content` | `string` | ✅ | Raw text content of the source |
| `source_type` | `string` | ❌ | Type hint affecting default reliability score. One of: `document`, `code`, `tool_output`, `user_input`, `web`, `conversation`, `observation`. Defaults to `document` |
| `title` | `string` | ❌ | Human-readable title |
| `metadata` | `object` | ❌ | Arbitrary JSON metadata |
| `session_id` | `uuid` | ❌ | If provided, a `CAPTURED_IN` edge is created from this source to the session node |
| `reliability` | `float` | ❌ | Override the default reliability score (0.0–1.0). Defaults by `source_type` |

**Default reliability by source_type:**

| source_type | default reliability |
|-------------|---------------------|
| `document`, `code` | 0.80 |
| `tool_output` | 0.70 |
| `user_input` | 0.75 |
| `web` | 0.60 |
| `conversation` | 0.50 |
| `observation` | 0.40 |
| _(unrecognized)_ | 0.50 |

**Example request:**
```json
{
  "content": "Rust's ownership system guarantees memory safety without a garbage collector.",
  "source_type": "document",
  "title": "Rust memory model overview",
  "metadata": { "url": "https://doc.rust-lang.org/book/", "author": "Steve Klabnik" },
  "reliability": 0.85
}
```

**Response (`201 Created`):**
```json
{
  "data": {
    "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
    "node_type": "source",
    "title": "Rust memory model overview",
    "content": "Rust's ownership system guarantees memory safety without a garbage collector.",
    "source_type": "document",
    "status": "active",
    "confidence": 0.85,
    "reliability": 0.85,
    "fingerprint": "a3f5e9...",
    "metadata": { "url": "https://doc.rust-lang.org/book/", "author": "Steve Klabnik" },
    "version": 1,
    "created_at": "2026-03-01T10:00:00Z",
    "modified_at": "2026-03-01T10:00:00Z"
  }
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `400` | Invalid request body |
| `500` | Database or internal error |

---

### `GET /sources` — List sources

**Description:** Returns a paginated list of sources, newest first.

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `limit` | `integer` | Max results (1–100, default 20) |
| `cursor` | `uuid` | Cursor-based pagination: return sources with `id > cursor` |
| `source_type` | `string` | Filter by source type |
| `status` | `string` | Filter by status (`active`, `archived`, `tombstone`) |
| `q` | `string` | Full-text search query (PostgreSQL `websearch_to_tsquery`) |

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
      "node_type": "source",
      "title": "Rust memory model overview",
      "content": "Rust's ownership system...",
      "source_type": "document",
      "status": "active",
      "confidence": 0.85,
      "reliability": 0.85,
      "fingerprint": "a3f5e9...",
      "metadata": {},
      "version": 1,
      "created_at": "2026-03-01T10:00:00Z",
      "modified_at": "2026-03-01T10:00:00Z"
    }
  ],
  "meta": { "count": 1 }
}
```

---

### `GET /sources/{id}` — Get a source

**Description:** Fetch a single source by UUID.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Source ID |

**Response (`200 OK`):** Single source object (same shape as list item above).

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | No source found with the given ID |

---

### `DELETE /sources/{id}` — Delete a source

**Description:** Permanently hard-deletes a source and its embedding. Also removes the corresponding AGE graph vertex (cascading to incident edges). This is **irreversible**.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Source ID |

**Response (`204 No Content`):** _(empty body)_

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | No source found with the given ID |

---

## 2. Articles

Articles are compiled knowledge nodes — synthesized from one or more sources. Unlike sources, articles are **mutable** (they have a `version` counter) and can be split, merged, or compiled asynchronously by an LLM.

---

### `POST /articles` — Create an article

**Description:** Directly create a new article (agent-authored). Creates an AGE vertex and enqueues an `embed` slow-path task. If `source_ids` are provided, `ORIGINATES` edges are created from each source to the new article.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `content` | `string` | ✅ | Article body text |
| `title` | `string` | ❌ | Human-readable title |
| `domain_path` | `string[]` | ❌ | Hierarchical domain tags, e.g. `["python", "stdlib"]` |
| `epistemic_type` | `string` | ❌ | One of `semantic`, `episodic`, `procedural`, `declarative`. Defaults to `semantic` |
| `source_ids` | `uuid[]` | ❌ | UUIDs of source nodes this article originates from |
| `metadata` | `object` | ❌ | Arbitrary JSON metadata |

**Example request:**
```json
{
  "content": "Rust guarantees memory safety through its ownership and borrowing system.",
  "title": "Rust Memory Safety",
  "domain_path": ["rust", "memory", "safety"],
  "epistemic_type": "semantic",
  "source_ids": ["3fa85f64-5717-4562-b3fc-2c963f66afa6"],
  "metadata": { "author_type": "agent" }
}
```

**Response (`201 Created`):**
```json
{
  "data": {
    "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "node_type": "article",
    "title": "Rust Memory Safety",
    "content": "Rust guarantees memory safety...",
    "status": "active",
    "confidence": 0.5,
    "epistemic_type": "semantic",
    "domain_path": ["rust", "memory", "safety"],
    "metadata": { "author_type": "agent" },
    "version": 1,
    "pinned": false,
    "usage_score": 0.0,
    "contention_count": 0,
    "created_at": "2026-03-01T10:00:00Z",
    "modified_at": "2026-03-01T10:00:00Z"
  }
}
```

---

### `POST /articles/compile` — Compile sources into an article (async)

**Description:** Enqueues an asynchronous LLM compilation job that synthesizes one or more sources into a new article. Returns `202 Accepted` immediately with a `job_id`. Poll `GET /admin/queue/{job_id}` to check status; once `completed` the compiled article will exist as a regular article node.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `source_ids` | `uuid[]` | ✅ | Non-empty list of source UUIDs to compile |
| `title_hint` | `string` | ❌ | Optional title suggestion for the LLM |

**Example request:**
```json
{
  "source_ids": [
    "3fa85f64-5717-4562-b3fc-2c963f66afa6",
    "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
  ],
  "title_hint": "Rust memory management patterns"
}
```

**Response (`202 Accepted`):**
```json
{
  "data": {
    "job_id": "550e8400-e29b-41d4-a716-446655440000",
    "status": "pending"
  }
}
```

---

### `POST /articles/merge` — Merge two articles

**Description:** Merges two articles into a new article. The merged content is the concatenation of both (separated by `---`). `MERGED_FROM` edges are created from the new article to each parent. Provenance edges from both parents are inherited. Both originals are **archived**.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `article_id_a` | `uuid` | ✅ | First article to merge |
| `article_id_b` | `uuid` | ✅ | Second article to merge |

**Example request:**
```json
{
  "article_id_a": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "article_id_b": "550e8400-e29b-41d4-a716-446655440000"
}
```

**Response (`201 Created`):** Returns the newly created merged article (same shape as Create Article response).

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Either article not found |

---

### `GET /articles` — List articles

**Description:** Returns a paginated list of articles, newest first.

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `limit` | `integer` | Max results (1–100, default 20) |
| `cursor` | `uuid` | Pagination cursor: return articles with `id > cursor` |
| `status` | `string` | Filter by status. Default: `active`. Options: `active`, `archived`, `tombstone` |

**Response (`200 OK`):**
```json
{
  "data": [ /* array of article objects */ ],
  "meta": { "count": 5 }
}
```

---

### `GET /articles/{id}` — Get an article

**Description:** Fetch a single article by UUID. Includes a live `contention_count` of active CONTRADICTS/CONTENDS edges.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Article ID |

**Response (`200 OK`):**
```json
{
  "data": {
    "id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "node_type": "article",
    "title": "Rust Memory Safety",
    "content": "Rust guarantees memory safety...",
    "status": "active",
    "confidence": 0.72,
    "epistemic_type": "semantic",
    "domain_path": ["rust", "memory", "safety"],
    "metadata": {},
    "version": 3,
    "pinned": false,
    "usage_score": 4.2,
    "contention_count": 1,
    "created_at": "2026-03-01T10:00:00Z",
    "modified_at": "2026-03-01T12:30:00Z"
  }
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | No article found with the given ID |

---

### `PATCH /articles/{id}` — Update an article

**Description:** Partially update an article. Each provided field is updated independently. Increments `version` and updates `modified_at`. If `content` is changed, a new `embed` task is enqueued.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Article ID |

**Request body** (all fields optional):

| Field | Type | Description |
|-------|------|-------------|
| `content` | `string` | New article body |
| `title` | `string` | New title |
| `domain_path` | `string[]` | Replacement domain path array |
| `pinned` | `boolean` | Pin/unpin the article (pinned articles are exempt from capacity eviction) |

**Example request:**
```json
{
  "title": "Rust Memory Safety (Updated)",
  "pinned": true
}
```

**Response (`200 OK`):** Returns the updated article object.

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Article not found |

---

### `DELETE /articles/{id}` — Archive an article

**Description:** Soft-deletes an article by setting `status = 'archived'` and recording `archived_at`. Only `active` articles can be archived. The node is **not** removed from the database or graph.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Article ID |

**Response (`204 No Content`):** _(empty body)_

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Article not found or already archived |

---

### `POST /articles/{id}/split` — Split an article

**Description:** Splits an article into two roughly equal parts at a paragraph boundary near the midpoint. Creates two new articles (Part 1, Part 2) with `SPLIT_INTO` edges from the original and inherited provenance edges. The original is **archived**.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Article ID to split |

**Response (`201 Created`):**
```json
{
  "data": {
    "original_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "part_a": { "id": "...", "title": "Rust Memory Safety (Part 1)", "..." : "..." },
    "part_b": { "id": "...", "title": "Rust Memory Safety (Part 2)", "..." : "..." }
  }
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Article not found |

---

### `GET /articles/{id}/provenance` — Get provenance chain

**Description:** Walks the graph backward from an article following provenance edge types (`ORIGINATES`, `CONFIRMS`, `SUPERSEDES`, `DERIVES_FROM`, `MERGED_FROM`, `SPLIT_INTO`, `SPLIT_FROM`, `EXTENDS`, `COMPILED_FROM`, `ELABORATES`) up to `max_depth` hops.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Article ID |

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `max_depth` | `integer` | Maximum traversal depth (default 5) |

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "source_node": { "id": "3fa85f64-...", "node_type": "source", "..." : "..." },
      "edge_type": "ORIGINATES",
      "confidence": 1.0,
      "depth": 1
    },
    {
      "source_node": { "id": "6ba7b810-...", "node_type": "source", "..." : "..." },
      "edge_type": "CONFIRMS",
      "confidence": 0.9,
      "depth": 2
    }
  ]
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Article not found |

---

### `POST /articles/{id}/trace` — Trace a claim to sources

**Description:** Given a specific claim text, ranks the article's linked sources by TF-IDF cosine similarity to identify which sources most likely contributed the claim. Only considers sources connected via `ORIGINATES`, `CONFIRMS`, or `SUPERSEDES` edges.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Article ID |

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `claim_text` | `string` | ✅ | The specific claim or sentence to trace |

**Example request:**
```json
{
  "claim_text": "ownership eliminates dangling pointer bugs at compile time"
}
```

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "source_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
      "title": "Rust memory model overview",
      "score": 0.847,
      "snippet": "...ownership system guarantees memory safety without a garbage collector..."
    },
    {
      "source_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
      "title": "Rust book chapter 4",
      "score": 0.312,
      "snippet": "...borrowing rules prevent data races..."
    }
  ],
  "meta": { "count": 2 }
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Article not found or not active |

---

## 3. Search

Covalence uses a three-dimensional retrieval cascade:

1. **Vector** (semantic similarity via pgvector) and **Lexical** (BM25 or `ts_rank`) run in **parallel**
2. **Graph** dimension runs on the candidate set produced by step 1
3. Scores are fused: `final_score = dim_score×0.85 + confidence×0.10 + freshness×0.05`

If no `embedding` is provided, the engine auto-embeds `query` using the configured LLM. Freshness decay is computed as `exp(-0.01 × days_since_modified)`.

---

### `POST /search` — Unified search

**Description:** Search across all active nodes using the three-dimensional retrieval cascade.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | `string` | ✅ | Natural language query string |
| `embedding` | `float[]` | ❌ | Pre-computed query embedding. If omitted, auto-embedded from `query` |
| `intent` | `string` | ❌ | Routing hint for graph dimension. One of: `factual`, `temporal`, `causal`, `entity` |
| `session_id` | `uuid` | ❌ | Session context for graph-aware ranking |
| `node_types` | `string[]` | ❌ | Filter to specific node types, e.g. `["article"]` |
| `limit` | `integer` | ❌ | Max results (default 10) |
| `weights` | `object` | ❌ | Custom dimension weights (auto-normalized to sum 1.0) |
| `weights.vector` | `float` | ❌ | Vector weight (default 0.65) |
| `weights.lexical` | `float` | ❌ | Lexical weight (default 0.25) |
| `weights.graph` | `float` | ❌ | Graph weight (default 0.10) |

**Intent → prioritized edge types for graph dimension:**

| intent | prioritized edges |
|--------|-------------------|
| `factual` | `CONFIRMS`, `ORIGINATES` |
| `temporal` | `PRECEDES`, `FOLLOWS`, `CONCURRENT_WITH` |
| `causal` | `CAUSES`, `MOTIVATED_BY`, `IMPLEMENTS` |
| `entity` | `INVOLVES`, `CAPTURED_IN` |

**Example request:**
```json
{
  "query": "how does Rust prevent memory leaks",
  "intent": "factual",
  "node_types": ["article"],
  "limit": 5,
  "weights": { "vector": 0.7, "lexical": 0.2, "graph": 0.1 }
}
```

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "node_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
      "score": 0.913,
      "vector_score": 0.95,
      "lexical_score": 0.82,
      "graph_score": 0.60,
      "confidence": 0.72,
      "node_type": "article",
      "title": "Rust Memory Safety",
      "content_preview": "Rust guarantees memory safety through its ownership and borrowing system..."
    }
  ],
  "meta": {
    "total_results": 1,
    "lexical_backend": "bm25",
    "dimensions_used": ["vector", "lexical", "graph"],
    "elapsed_ms": 42
  }
}
```

---

## 4. Edges / Graph

Edges represent typed relationships between nodes. The graph is stored in both PostgreSQL (`covalence.edges`) and Apache AGE for Cypher-based traversals.

### Edge type vocabulary

| Label | Category | Description |
|-------|----------|-------------|
| `ORIGINATES` | Provenance | Source directly contributed to article compilation |
| `CONFIRMS` | Provenance | Source corroborates an existing article |
| `SUPERSEDES` | Provenance | New node replaces old (new→old) |
| `CONTRADICTS` | Provenance | Conflicting claims |
| `CONTENDS` | Provenance | Softer disagreement / alternative interpretation |
| `EXTENDS` | Provenance | Elaborates without superseding |
| `DERIVES_FROM` | Provenance | Article derived from another article |
| `MERGED_FROM` | Provenance | Article produced by merging parents |
| `SPLIT_INTO` | Provenance | Original was divided into this fragment |
| `SPLIT_FROM` | Provenance | Fragment produced by splitting a parent |
| `PRECEDES` | Temporal | Temporally before |
| `FOLLOWS` | Temporal | Temporally after |
| `CONCURRENT_WITH` | Temporal | Overlapping time periods |
| `CAUSES` | Causal | LLM-inferred causal relationship |
| `MOTIVATED_BY` | Causal | Decision motivated by this knowledge |
| `IMPLEMENTS` | Causal | Concrete artifact implements abstract concept |
| `RELATES_TO` | Semantic | Generic semantic relatedness |
| `GENERALIZES` | Semantic | Abstracts a more specific node |
| `CAPTURED_IN` | Session | Source captured during session |
| `INVOLVES` | Entity | Node references a named entity |
| `COMPILED_FROM` | Legacy | Alias for `ORIGINATES` |
| `ELABORATES` | Legacy | Alias for `EXTENDS` |

---

### `POST /edges` — Create an edge

**Description:** Create a typed edge between any two nodes.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `from_node_id` | `uuid` | ✅ | Source (origin) node UUID |
| `to_node_id` | `uuid` | ✅ | Target (destination) node UUID |
| `label` | `string` | ✅ | Edge type label (see vocabulary above) |
| `confidence` | `float` | ❌ | Edge confidence (0.0–1.0, default 1.0) |
| `method` | `string` | ❌ | Creation method (default `agent_explicit`) |
| `notes` | `string` | ❌ | Free-text annotation stored in edge metadata |

**Example request:**
```json
{
  "from_node_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
  "to_node_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "label": "CONFIRMS",
  "confidence": 0.9,
  "notes": "Corroborates section on ownership rules"
}
```

**Response (`201 Created`):**
```json
{
  "data": {
    "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    "age_id": 12345,
    "source_node_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
    "target_node_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    "edge_type": "CONFIRMS",
    "weight": 1.0,
    "confidence": 0.9,
    "metadata": { "notes": "Corroborates section on ownership rules" },
    "created_at": "2026-03-01T10:00:00Z",
    "created_by": "agent_explicit"
  }
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `400` | Unknown `label` value |

---

### `DELETE /edges/{id}` — Delete an edge

**Description:** Removes an edge from both PostgreSQL and Apache AGE.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Edge ID |

**Response (`204 No Content`):** _(empty body)_

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Edge not found |

---

### `GET /nodes/{id}/edges` — List edges for a node

**Description:** List edges connected to a node with optional direction and type filters.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Node UUID (source, article, or session) |

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `direction` | `string` | `outbound`, `inbound`, or omit for both (default) |
| `labels` | `string` | Comma-separated edge type filter, e.g. `CONFIRMS,ORIGINATES` |
| `limit` | `integer` | Max results (default 50) |

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
      "source_node_id": "3fa85f64-...",
      "target_node_id": "7c9e6679-...",
      "edge_type": "CONFIRMS",
      "weight": 1.0,
      "confidence": 0.9,
      "metadata": {},
      "created_at": "2026-03-01T10:00:00Z",
      "created_by": "agent_explicit"
    }
  ],
  "meta": { "count": 1 }
}
```

---

### `GET /nodes/{id}/neighborhood` — Graph neighborhood traversal

**Description:** Traverses the graph outward from a starting node up to `depth` hops, returning all reachable neighbors with their connecting edges.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Starting node UUID |

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `depth` | `integer` | Max traversal depth (1–5, default 2) |
| `direction` | `string` | `outbound`, `inbound`, or omit for both (default) |
| `labels` | `string` | Comma-separated edge type filter |
| `limit` | `integer` | Max total neighbors returned (1–200, default 50) |

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "node": { "id": "...", "node_type": "source", "title": "...", "..." : "..." },
      "edge": { "id": "...", "edge_type": "ORIGINATES", "..." : "..." },
      "depth": 1
    },
    {
      "node": { "id": "...", "node_type": "article", "..." : "..." },
      "edge": { "id": "...", "edge_type": "CONFIRMS", "..." : "..." },
      "depth": 2
    }
  ],
  "meta": { "count": 2 }
}
```

---

## 5. Contentions

Contentions are automatically detected when an incoming source appears to contradict or dispute an existing article. They track the conflict lifecycle from `detected` through `resolved` or `dismissed`.

---

### `GET /contentions` — List contentions

**Description:** Returns all contentions, optionally filtered by article node or status. Ordered by `detected_at` descending.

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `node_id` | `uuid` | Filter to contentions involving this article node |
| `status` | `string` | Filter by status: `detected`, `resolved`, `dismissed` |

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "id": "b6d2f3a1-0000-4000-8000-000000000001",
      "node_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
      "source_node_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
      "description": "Source claims Rust uses GC; article states it does not",
      "status": "detected",
      "resolution": null,
      "severity": "high",
      "detected_at": "2026-03-01T11:00:00Z",
      "resolved_at": null
    }
  ]
}
```

---

### `GET /contentions/{id}` — Get a contention

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Contention ID |

**Response (`200 OK`):** Single contention object (same shape as list item).

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Contention not found |

---

### `POST /contentions/{id}/resolve` — Resolve a contention

**Description:** Mark a contention as resolved. The resolution and rationale are stored as `"<type>: <rationale>"` in the `resolution` field and `status` is set to `resolved`.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Contention ID |

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `resolution` | `string` | ✅ | One of: `supersede_a` (article wins), `supersede_b` (source wins), `accept_both` (both valid), `dismiss` (not material) |
| `rationale` | `string` | ✅ | Free-text explanation of the decision |

**Example request:**
```json
{
  "resolution": "supersede_a",
  "rationale": "The source is an uncited blog post; the article references the official Rust book."
}
```

**Response (`200 OK`):**
```json
{
  "data": {
    "id": "b6d2f3a1-0000-4000-8000-000000000001",
    "node_id": "7c9e6679-...",
    "source_node_id": "3fa85f64-...",
    "description": "Source claims Rust uses GC; article states it does not",
    "status": "resolved",
    "resolution": "supersede_a: The source is an uncited blog post...",
    "severity": "high",
    "detected_at": "2026-03-01T11:00:00Z",
    "resolved_at": "2026-03-01T11:30:00Z"
  }
}
```

**Errors:**

| Status | Condition |
|--------|-----------|
| `400` | Invalid `resolution` value |
| `404` | Contention not found |

---

## 6. Memory

The Memory API is a purpose-built wrapper over the Source API for agent-authored observations. Memories are stored as `source_type = 'observation'` nodes tagged with `metadata.memory = true`. Confidence is derived from importance: `confidence = 0.4 + (importance × 0.4)`.

---

### `POST /memory` — Store a memory

**Description:** Store a new memory observation. Enqueues an `embed` task. If `supersedes_id` is provided, the old memory is soft-forgotten and a `SUPERSEDES` edge is created.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `content` | `string` | ✅ | Memory content text |
| `tags` | `string[]` | ❌ | Categorization tags (default `[]`) |
| `importance` | `float` | ❌ | 0.0–1.0; drives confidence score (default 0.5) |
| `context` | `string` | ❌ | Provenance hint, e.g. `"session:main"` or `"observation:system"` |
| `supersedes_id` | `uuid` | ❌ | UUID of a prior memory this replaces |

**Example request:**
```json
{
  "content": "The user prefers dark mode in all UI contexts.",
  "tags": ["preferences", "ui"],
  "importance": 0.8,
  "context": "conversation:user"
}
```

**Response (`201 Created`):**
```json
{
  "data": {
    "id": "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
    "content": "The user prefers dark mode in all UI contexts.",
    "tags": ["preferences", "ui"],
    "importance": 0.8,
    "context": "conversation:user",
    "confidence": 0.72,
    "created_at": "2026-03-01T10:00:00Z",
    "forgotten": false
  }
}
```

---

### `POST /memory/search` — Recall memories

**Description:** Search active (non-forgotten) memories using full-text search (`websearch_to_tsquery`), ranked by `ts_rank`. Tag filtering uses JSONB containment — all listed tags must be present.

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | `string` | ✅ | Natural language recall query |
| `limit` | `integer` | ❌ | Max results (default 5) |
| `tags` | `string[]` | ❌ | Return only memories containing ALL listed tags |
| `min_confidence` | `float` | ❌ | Minimum confidence threshold (default 0.0) |

**Example request:**
```json
{
  "query": "user interface preferences",
  "tags": ["ui"],
  "min_confidence": 0.5,
  "limit": 3
}
```

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "id": "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
      "content": "The user prefers dark mode in all UI contexts.",
      "tags": ["preferences", "ui"],
      "importance": 0.8,
      "context": "conversation:user",
      "confidence": 0.72,
      "created_at": "2026-03-01T10:00:00Z",
      "forgotten": false
    }
  ]
}
```

---

### `GET /memory/status` — Memory system stats

**Description:** Returns aggregate counts for the memory subsystem.

**Response (`200 OK`):**
```json
{
  "data": {
    "total_memories": 42,
    "active_memories": 38,
    "forgotten_memories": 4
  }
}
```

---

### `PATCH /memory/{id}/forget` — Forget a memory

**Description:** Soft-deletes a memory by setting `metadata.forgotten = true`. The underlying node is **not** deleted; it is excluded from future recall results.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Memory (source node) UUID |

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `reason` | `string` | ❌ | Optional reason stored as `metadata.forget_reason` |

**Example request:**
```json
{ "reason": "User preference changed; superseded by newer memory." }
```

**Response (`204 No Content`):** _(empty body)_

---

## 7. Sessions

Sessions track agent interaction contexts. Sources ingested with a `session_id` automatically receive `CAPTURED_IN` edges linking them to the session node.

---

### `POST /sessions` — Create a session

**Request body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `label` | `string` | ❌ | Human-readable session label |
| `metadata` | `object` | ❌ | Arbitrary JSON metadata (default `{}`) |

**Example request:**
```json
{
  "label": "research-session-2026-03-01",
  "metadata": { "agent": "claude-code", "task": "api-documentation" }
}
```

**Response (`201 Created`):**
```json
{
  "data": {
    "id": "c2fe46a9-0000-4000-8000-000000000099",
    "label": "research-session-2026-03-01",
    "status": "open",
    "created_at": "2026-03-01T10:00:00Z",
    "last_active_at": "2026-03-01T10:00:00Z",
    "metadata": { "agent": "claude-code", "task": "api-documentation" }
  }
}
```

---

### `GET /sessions` — List sessions

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | `string` | Filter by status: `open`, `closed` |
| `limit` | `integer` | Max results (default 50) |

**Response (`200 OK`):** Array of session objects ordered by `last_active_at` descending.

```json
{
  "data": [
    {
      "id": "c2fe46a9-0000-4000-8000-000000000099",
      "label": "research-session-2026-03-01",
      "status": "open",
      "created_at": "2026-03-01T10:00:00Z",
      "last_active_at": "2026-03-01T10:15:00Z",
      "metadata": {}
    }
  ]
}
```

---

### `GET /sessions/{id}` — Get a session

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Session ID |

**Response (`200 OK`):** Single session object.

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Session not found |

---

### `POST /sessions/{id}/close` — Close a session

**Description:** Sets session `status` to `closed`.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Session ID |

**Response (`204 No Content`):** _(empty body)_

---

## 8. Admin

Admin endpoints provide operational control over background tasks, maintenance routines, and system visibility.

---

### `GET /admin/stats` — System statistics

**Description:** Returns a system health snapshot including node counts, edge sync status, queue depth, and embedding coverage.

**Response (`200 OK`):**
```json
{
  "data": {
    "nodes": {
      "total": 1420,
      "sources": 980,
      "articles": 430,
      "sessions": 10,
      "active": 1350,
      "archived": 70,
      "pinned": 5
    },
    "edges": {
      "sql_count": 3200,
      "age_count": 3200,
      "in_sync": true
    },
    "queue": {
      "pending": 12,
      "processing": 2,
      "failed": 1,
      "completed_24h": 348
    },
    "embeddings": {
      "total": 1300,
      "nodes_without": 50
    }
  }
}
```

> `edges.in_sync` is `true` when `sql_count == age_count`. Divergence indicates a partial graph write failure.

---

### `POST /admin/maintenance` — Run maintenance operations

**Description:** Trigger one or more maintenance operations. Any field omitted or `false` is skipped.

**Request body:**

| Field | Type | Description |
|-------|------|-------------|
| `recompute_scores` | `boolean` | Recompute `usage_score` for all active nodes from usage trace data |
| `process_queue` | `boolean` | Time out stale `processing` queue jobs (stuck > 10 min) → mark `failed` |
| `evict_if_over_capacity` | `boolean` | Archive lowest-scoring non-pinned articles when count > 1000 |
| `evict_count` | `integer` | Max articles to evict per run (default 10) |

**Example request:**
```json
{
  "recompute_scores": true,
  "evict_if_over_capacity": true,
  "evict_count": 20
}
```

**Response (`200 OK`):**
```json
{
  "data": {
    "actions_taken": [
      "recomputed usage scores",
      "evicted 12 low-usage articles"
    ]
  }
}
```

---

### `GET /admin/queue` — List queue entries

**Description:** List slow-path background task queue entries. Results are ordered by `priority DESC, created_at ASC`.

**Query parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | `string` | Filter by: `pending`, `processing`, `failed`, `completed` |
| `limit` | `integer` | Max results (default 50) |

**Task types:**

| task_type | Triggered by | Description |
|-----------|-------------|-------------|
| `embed` | Source/Article ingest or update | Generate a vector embedding for a node |
| `contention_check` | Source ingest | Detect contradictions with existing articles |
| `compile` | `POST /articles/compile` | LLM synthesis of sources into an article |
| `tree_index` | `POST /admin/tree-index-all` | Build tree-of-thought index chunks for a node |
| `tree_embed` | After `tree_index` | Embed the tree-index chunks |

**Response (`200 OK`):**
```json
{
  "data": [
    {
      "id": "d290f1ee-6c54-4b01-90e6-d701748f0851",
      "task_type": "embed",
      "node_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
      "status": "pending",
      "priority": 3,
      "created_at": "2026-03-01T10:00:00Z",
      "started_at": null,
      "completed_at": null
    }
  ]
}
```

---

### `GET /admin/queue/{id}` — Get a queue entry

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Queue entry ID |

**Response (`200 OK`):** Single queue entry object.

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Queue entry not found |

---

### `POST /admin/queue/{id}/retry` — Retry a failed queue entry

**Description:** Resets a `failed` entry back to `pending`. Only entries with `status = 'failed'` can be retried.

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Queue entry ID |

**Response (`200 OK`):** The updated queue entry with `status: "pending"`.

**Errors:**

| Status | Condition |
|--------|-----------|
| `400` | Entry is not in `failed` status |
| `404` | Queue entry not found |

---

### `DELETE /admin/queue/{id}` — Delete a queue entry

**Path parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | `uuid` | Queue entry ID |

**Response (`204 No Content`):** _(empty body)_

**Errors:**

| Status | Condition |
|--------|-----------|
| `404` | Queue entry not found |

---

### `POST /admin/embed-all` — Queue embeddings for all unembedded nodes

**Description:** Enqueues `embed` tasks for every active node that lacks an embedding and does not already have a pending/processing embed task. Useful after bulk import.

**Request body:** None

**Response (`200 OK`):**
```json
{ "queued": 47 }
```

---

### `POST /admin/tree-index-all` — Queue tree indexing for all eligible nodes

**Description:** Enqueues `tree_index` (plus a follow-up `tree_embed`) task for active nodes whose content meets or exceeds `min_chars`. By default, skips already-indexed nodes.

**Request body:**

| Field | Type | Description |
|-------|------|-------------|
| `overlap` | `float` | Chunk overlap ratio (default 0.20) |
| `force` | `boolean` | Re-index already-indexed nodes if `true` (default `false`) |
| `min_chars` | `integer` | Minimum content length in characters to qualify (default 700) |

**Example request:**
```json
{ "overlap": 0.15, "force": false, "min_chars": 500 }
```

**Response (`200 OK`):**
```json
{
  "queued": 23,
  "overlap": 0.15,
  "force": false,
  "min_chars": 500
}
```

---

## 9. Shared Types

### Node object

Full node representation used in provenance, neighborhood, and detailed responses.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `uuid` | Unique identifier |
| `node_type` | `string` | `source`, `article`, `session`, or `entity` |
| `title` | `string\|null` | Human-readable title |
| `content` | `string\|null` | Node body text |
| `status` | `string` | `active`, `archived`, or `tombstone` |
| `confidence` | `float` | Overall confidence score (0.0–1.0) |
| `domain_path` | `string[]` | Hierarchical topic tags |
| `metadata` | `object` | Arbitrary JSON metadata |
| `version` | `integer` | Mutation counter (starts at 1) |
| `pinned` | `boolean` | Exempt from capacity eviction when `true` |
| `usage_score` | `float` | Recency-decayed retrieval frequency |
| `created_at` | `datetime` | ISO 8601 UTC |
| `modified_at` | `datetime` | ISO 8601 UTC |

### Edge object

| Field | Type | Description |
|-------|------|-------------|
| `id` | `uuid` | Unique identifier |
| `source_node_id` | `uuid` | Origin node |
| `target_node_id` | `uuid` | Destination node |
| `edge_type` | `string` | Edge label (SCREAMING_SNAKE_CASE) |
| `weight` | `float` | Edge weight (default 1.0) |
| `confidence` | `float` | Edge confidence (0.0–1.0) |
| `metadata` | `object` | Arbitrary JSON metadata |
| `created_at` | `datetime` | ISO 8601 UTC |
| `created_by` | `string\|null` | Creator method/agent identifier |

### Confidence sub-scores

When returned in expanded form, confidence components are:

| Component | Weight | Description |
|-----------|--------|-------------|
| `source` | 0.30 | Source reliability |
| `method` | 0.15 | Method quality |
| `consistency` | 0.20 | Internal consistency |
| `freshness` | 0.10 | Recency |
| `corroboration` | 0.15 | Number of confirming sources |
| `applicability` | 0.10 | Relevance to context |
| `overall` | — | Weighted composite |

Formula: `overall = source×0.30 + method×0.15 + consistency×0.20 + freshness×0.10 + corroboration×0.15 + applicability×0.10`

### Node status values

| Value | Description |
|-------|-------------|
| `active` | Normal operating state; included in search results |
| `archived` | Soft-deleted; excluded from search but retained in DB |
| `tombstone` | Permanently removed reference; cascade of hard deletes |

### Epistemic types

| Value | Description |
|-------|-------------|
| `semantic` | General factual/conceptual knowledge |
| `episodic` | Event or interaction-specific memory |
| `procedural` | How-to or process knowledge |
| `declarative` | Explicitly stated facts |

---

## 10. Error Reference

All error responses follow:

```json
{ "error": "human-readable description" }
```

| HTTP Status | Meaning |
|-------------|---------|
| `400 Bad Request` | Invalid request body, missing required field, unknown enum value, or business-rule violation (e.g. retrying a non-failed queue entry) |
| `404 Not Found` | The requested resource does not exist |
| `500 Internal Server Error` | Unexpected database or internal engine error |

> The engine does not return `401`/`403`; access control is enforced at the plugin/proxy layer.

---

_Generated from `engine/src/api/routes.rs` and `engine/src/services/*.rs` — Covalence engine. For schema details see `docs/schema-dump.md`._
