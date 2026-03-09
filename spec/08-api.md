# 08 — API Surface

**Status:** Implemented

## Overview

The API layer is a thin routing shell over the engine. Two interfaces: HTTP REST (for general clients) and MCP (for Claude/agent integration). Both expose the same underlying operations.

## HTTP Endpoints

### Ingestion

```
POST /sources
  Body: { source_type, content, metadata, clearance_level?, budget? }
  → Accepts a source and kicks off the ingestion pipeline
  → budget: optional cost cap for LLM calls (prevents runaway API costs)
  → Returns: { source_id, status: "accepted" }

GET /sources/:id
  → Source metadata + ingestion status + trust scores

GET /sources/:id/chunks
  → Chunk tree for a source, including landscape analysis metrics
  → Each chunk includes: parent_alignment, extraction_method, landscape_metrics, flags

GET /sources/:id/landscape
  → Embedding landscape analysis summary for a source
  → Returns: similarity curves, valleys, plateaus, extraction method distribution,
    cross-document novelty stats, model calibration used
  → Useful for debugging ingestion decisions and tuning thresholds

DELETE /sources/:id
  → Takedown: TMS cascade, soft-delete source, purge sole-provenance entities
  → Returns: { affected_nodes, affected_edges, audit_log_id }
```

### Search

```
POST /search
  Body: {
    query: String,
    strategy: "balanced" | "precise" | "exploratory" | "recent" | "graph_first" | "custom",
    weights: { vector, lexical, temporal, graph, structural }?,  // for custom strategy
    limit: Int?,
    filters: {
      source_types: [String]?,
      date_range: { start, end }?,
      node_types: [String]?,
      min_confidence: Float?,
    }?
  }
  → Returns: [SearchResult]
```

### Graph

```
GET /nodes/:id
  → Node with properties, aliases, and connected edges

GET /nodes/:id/neighborhood
  Query: { hops, edge_types, limit }
  → k-hop neighborhood around a node

GET /nodes/:id/provenance
  → Full provenance chain: extractions → chunks → sources

POST /nodes/resolve
  Body: { name, description?, context? }
  → Entity resolution: find existing node or suggest creation

GET /edges/:id
  → Edge with properties and provenance

GET /graph/communities
  → Current community structure

POST /nodes/merge
  Body: { source_nodes: [UUID], target_node: UUID }
  → Merge multiple nodes into one. Creates SUPERSEDES edges from old to new.
  → Old nodes set to clearance_level -1 (inactive). Sidecar follows SUPERSEDES dynamically.
  → Returns: { merged_node, audit_log_id }

POST /nodes/:id/split
  Body: { new_nodes: [{ name, type, description, edge_ids }] }
  → Split a wrongly-merged node into distinct entities
  → Returns: { new_nodes: [UUID], audit_log_id }

GET /graph/stats
  → Node count, edge count, density, component count

GET /audit
  Query: { action?, target_type?, target_id?, since?, limit? }
  → Paginated audit log of system decisions
```

### Memory (Agent-Friendly Wrapper)

```
POST /memory
  Body: { content, importance?, tags?, context?, supersedes_id? }
  → Store a memory (creates an observation source with memory metadata)
  → Returns: { memory_id }

POST /memory/recall
  Body: { query, tags?, limit?, min_confidence? }
  → Search memories ranked by relevance, confidence, freshness
  → Returns: [MemoryResult]

DELETE /memory/:id
  Body: { reason? }
  → Soft-forget a memory (filtered from future recall, audit trail preserved)

GET /memory/status
  → Memory count, top tags, last memory timestamp
```

### Admin

### Knowledge Curation (Human-in-the-Loop)

User corrections and annotations feed back into the graph. Every curation action is recorded as a new source (type: `user_correction`, reliability: 0.9) with full provenance.

```
POST /nodes/:id/correct
  Body: { description: "corrected description", reason: "why" }
  → Updates node description, re-embeds, creates correction source
  → Increments node confidence (human-verified)

POST /edges/:id/correct
  Body: { rel_type: "new_type", properties: {...}, reason: "why" }
  → Updates edge, creates correction source
  → For factual corrections: invalidates old edge (bi-temporal),
    creates new corrected edge

DELETE /edges/:id
  Body: { reason: "why" }
  → Bi-temporal invalidation (sets invalid_at, not physical delete)
  → Creates correction source documenting the removal

POST /nodes/:id/annotate
  Body: { annotation: "additional context", tags: ["verified", "disputed"] }
  → Adds annotation to node properties, creates annotation source

POST /search/feedback
  Body: { trace_id: "...", result_id: "...", relevant: true|false, comment: "..." }
  → Records relevance feedback for a search result
  → Used to tune retrieval quality over time (eval dataset building)
```

All curation actions are:
- Attributed to a user identity
- Stored as sources for provenance tracking
- Auditable (who changed what, when, why)
- Reversible (corrections create new edges, don't destroy old ones)

```

```
POST /admin/graph/reload
  → Force full reload of graph sidecar from PG

POST /admin/publish/:source_id
  Query: { clearance_level: 1 | 2 }
  → Promote source + derivatives to federated clearance level
  → Recursively updates chunks, extractions, nodes/edges
  → Returns: { promoted_count, audit_log_id }

POST /admin/consolidate
  Query: { tier: "batch" | "deep" }
  → Trigger manual consolidation run

GET /admin/health
  → System health, PG connectivity, sidecar status, outbox lag

GET /admin/metrics
  → Node/edge/chunk/source counts, index sizes, query latencies, LLM cost tracking

GET /admin/traces?limit=50
  → Recent query traces for debugging retrieval quality

POST /admin/trace/{trace_id}/replay
  → Replay a traced query with different parameters for A/B comparison
```

### Query Tracing

Every search request produces a trace record capturing the full pipeline execution:

```json
{
  "trace_id": "uuid",
  "query": "original query text",
  "timestamp": "...",
  "strategy_selected": "balanced",
  "strategy_auto": true,
  "score_distribution": {"entropy": 0.72, "gini": 0.35},
  "dimension_results": {
    "vector": {"count": 20, "top_score": 0.94, "latency_ms": 12},
    "lexical": {"count": 15, "top_score": 0.87, "latency_ms": 3},
    "graph": {"count": 8, "top_score": 0.76, "latency_ms": 45},
    "temporal": {"count": 3, "top_score": 0.92, "latency_ms": 2},
    "structural": {"count": 5, "top_score": 0.65, "latency_ms": 1},
    "global": {"count": 2, "top_score": 0.71, "latency_ms": 8}
  },
  "fusion_results": 20,
  "rerank_results": 10,
  "context_assembly": {"items": 7, "tokens": 6200, "deduped": 2, "budget_trimmed": 1},
  "generation": {"model": "...", "input_tokens": 7100, "output_tokens": 450, "latency_ms": 1200},
  "total_latency_ms": 1320
}
```

Traces enable:
- **Debugging**: "Why didn't this query find the right answer?" → check dimension_results to see if relevant docs were retrieved but dropped during fusion/reranking
- **A/B testing**: Replay the same query with different strategies/weights
- **Cost tracking**: Per-query LLM token usage and embedding API calls
- **Regression detection**: Compare trace distributions before/after changes

## MCP Tools

For Claude/agent integration via MCP protocol:

```
search(query, strategy?, limit?)
  → Fused search across all dimensions

get_node(id)
  → Node details with neighborhood summary

get_provenance(node_id | edge_id)
  → Provenance chain for a fact

ingest_source(content, source_type, metadata)
  → Submit a source for ingestion

traverse(start_node_id, hops?, edge_types?)
  → Graph neighborhood exploration

resolve_entity(name, description?)
  → Find or create an entity

list_communities()
  → Community overview with top nodes per community

get_contradictions(node_id?)
  → Active contradictions in the knowledge base

memory_store(content, importance?, tags?, context?)
  → Remember something for later recall

memory_recall(query, tags?, limit?)
  → Recall relevant memories

memory_forget(memory_id, reason?)
  → Soft-forget a memory
```

## Authentication / Authorization

TBD — initial implementation is single-user, no auth. Multi-user/tenant support is a future concern.

## Error Responses

Standard HTTP status codes with structured error bodies:

```json
{
  "error": {
    "code": "ENTITY_NOT_FOUND",
    "message": "Node with id 550e8400-... not found",
    "details": {}
  }
}
```

## Rate Limiting

No rate limiting in initial implementation. LLM calls during ingestion are the bottleneck; those have their own backoff logic.

## Open Questions

- [x] SSE streaming → Defer. Results are typically small (top-20). SSE useful when adding LLM synthesis streaming in v2.
- [x] MCP raw graph algorithms → No. Expose high-level operations (search, traverse, communities). Raw PageRank output isn't useful to agents.
- [x] GraphQL → Defer. REST is sufficient for v1. GraphQL adds complexity without proportional benefit.
- [x] Webhooks → Defer. Polling via `GET /sources/:id` status is sufficient for v1.
- [x] Batch operations → v2 feature. For v1, clients POST /sources sequentially.
