# Lifecycle Hooks

## Overview

Lifecycle hooks are HTTP POST callbacks that fire at specific points in the `/ask` pipeline. They allow extensions to inject context, enrich queries, filter results, or observe pipeline activity without modifying the core engine.

Hooks are **fail-open by default**: if a hook times out or returns an error, the pipeline logs a warning and continues. This can be overridden per hook by setting `fail_open: false`, which causes hook errors to propagate and abort the pipeline.

## Phase Reference

| Phase | Direction | When | Payload | Response |
|-------|-----------|------|---------|----------|
| `pre_search` | sync | Before search execution in `/ask` | query, adapter_id | boost_terms, metadata_filters |
| `post_search` | sync | After search results, before LLM synthesis | query, results_summary, adapter_id | additional_context |
| `post_synthesis` | async | After LLM synthesis completes | query, answer, citations, adapter_id | (fire-and-forget) |

**Sync** hooks block the pipeline until all hooks for that phase complete. The engine merges responses from multiple hooks before proceeding.

**Async** hooks are fire-and-forget. The engine spawns a background task and returns immediately. Errors are logged but never propagated.

## Payload Schemas

### pre_search

Sent before the search call. Hooks can enrich the query with boost terms or add metadata filters.

**Request payload:**
```json
{
  "query": "How does entity resolution work?",
  "adapter_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `query` | string | The user's search query |
| `adapter_id` | string (UUID) or null | Adapter ID that scopes the hook. Null for global hooks |

**Response:**
```json
{
  "boost_terms": ["coreference", "deduplication", "fuzzy matching"],
  "metadata_filters": {"domain": "code", "min_confidence": 0.7}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `boost_terms` | array of strings or null | Additional terms to enrich the search query |
| `metadata_filters` | object or null | Opaque metadata filters passed to the search layer |

All response fields are optional. Return `{}` to make no modifications.

### post_search

Sent after search results are retrieved but before LLM synthesis. Hooks can inject additional context for the LLM.

**Request payload:**
```json
{
  "query": "How does entity resolution work?",
  "results_summary": "Found 8 results across 3 domains (code, spec, research). Top result: 'Entity Resolution Pipeline' (confidence 0.92).",
  "adapter_id": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `query` | string | The user's search query |
| `results_summary` | string | Human-readable summary of search results |
| `adapter_id` | string (UUID) or null | Adapter scope |

**Response:**
```json
{
  "additional_context": [
    "Note: entity resolution was refactored in Wave 7. See ADR-0018.",
    "The HDBSCAN tier is currently disabled in production."
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `additional_context` | array of strings or null | Context strings injected into the LLM prompt before synthesis |

### post_synthesis

Sent after LLM synthesis completes. This is fire-and-forget -- the response is ignored.

**Request payload:**
```json
{
  "query": "How does entity resolution work?",
  "answer": "Entity resolution in Covalence uses a 5-tier pipeline...",
  "citations": [
    {"source_id": "abc-123", "title": "spec/05-ingestion.md", "chunk_id": "def-456"}
  ],
  "adapter_id": null
}
```

| Field | Type | Description |
|-------|------|-------------|
| `query` | string | The user's original query |
| `answer` | string | The synthesized answer text |
| `citations` | array of objects | Citation objects from the synthesis |
| `adapter_id` | string (UUID) or null | Adapter scope |

**Response:** Ignored. Any valid HTTP response is accepted.

## Behavior

### fail_open

- **`true` (default):** Timeout or error is logged as a warning. The pipeline continues without the hook's contribution.
- **`false`:** Errors propagate. A hook failure aborts the pipeline and returns an error to the caller.

### timeout_ms

- Default: **2000** milliseconds.
- Configurable per hook in the extension manifest or via the admin API.
- The engine uses the per-hook timeout for the HTTP POST request. If the hook does not respond within this window, it is treated as a failure (subject to `fail_open`).

### Multiple Hooks Per Phase

When multiple hooks are registered for the same phase, they execute **concurrently** via `join_all`. Results are merged in registration order:

- `pre_search`: boost terms are concatenated. The last non-null `metadata_filters` wins.
- `post_search`: additional context strings are concatenated.
- `post_synthesis`: all hooks fire independently.

### Scope

Hooks can be **global** or **domain-scoped**:

- **Global** (`adapter_id = null`): fires for every `/ask` request regardless of adapter.
- **Domain-scoped** (`adapter_id` set): fires only when the request's `adapter_id` matches, or when the request has no adapter_id (global requests trigger all hooks).

The matching logic: a hook fires if the hook's `adapter_id` is null, or the request's `adapter_id` is null, or both match.

## Registration

### Via Extension Manifest

Add hooks to the `hooks:` section of your `extension.yaml`:

```yaml
hooks:
  - phase: pre_search
    url: "http://localhost:9090/enrich"
    timeout_ms: 3000
    fail_open: true
  - phase: post_synthesis
    url: "http://localhost:9090/observe"
    timeout_ms: 5000
    fail_open: true
```

Hooks declared in manifests are inserted into the `lifecycle_hooks` table at startup. Duplicate `(phase, hook_url)` pairs are skipped.

### Via Admin API

Register hooks at runtime:

```bash
curl -X POST http://localhost:8431/api/v1/admin/hooks \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-enricher",
    "phase": "pre_search",
    "hook_url": "http://localhost:9090/enrich",
    "timeout_ms": 3000,
    "fail_open": true
  }'
```

## Example: Testing a Hook

Start a simple hook server and test it end-to-end.

### 1. Create a hook endpoint

Using any HTTP framework, create a POST endpoint that returns boost terms:

```python
# hook_server.py (Flask example)
from flask import Flask, request, jsonify

app = Flask(__name__)

@app.route("/enrich", methods=["POST"])
def enrich():
    data = request.json
    query = data.get("query", "")
    # Add domain-specific boost terms
    return jsonify({"boost_terms": ["graph", "knowledge base"]})

app.run(port=9090)
```

### 2. Register the hook

```yaml
# In your extension.yaml
hooks:
  - phase: pre_search
    url: "http://localhost:9090/enrich"
    timeout_ms: 2000
    fail_open: true
```

### 3. Verify with curl

```bash
# Simulate what the engine sends:
curl -X POST http://localhost:9090/enrich \
  -H "Content-Type: application/json" \
  -d '{"query": "entity resolution", "adapter_id": null}'

# Expected: {"boost_terms": ["graph", "knowledge base"]}
```

### 4. Test via /ask

```bash
curl -X POST http://localhost:8431/api/v1/ask \
  -H "Content-Type: application/json" \
  -d '{"query": "How does entity resolution work?"}'
```

The engine will call your hook before searching. Check the engine logs for hook execution:

```
INFO pre_search hook succeeded hook="my-enricher"
```
