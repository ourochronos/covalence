# 08 — API Surface

**Status:** Implemented

## Overview

The API layer is a thin routing shell over the engine. Two interfaces: HTTP REST (for general clients) and MCP (for Claude/agent integration). Both expose the same underlying operations.

All API routes are versioned under `/api/v1`. Swagger UI is available at `/docs`. A root `/health` endpoint is provided for convenience without auth.

## HTTP Endpoints

### Sources

```
POST /api/v1/sources
  Body: {
    content: String?,        // base64-encoded content
    url: String?,            // OR a URL to fetch
    source_type: String?,    // document, web_page, conversation, code, api, manual, tool_output, observation
    mime: String?,           // e.g., "text/markdown", "text/x-rust"
    uri: String?,            // original material URI
    title: String?,          // override auto-extracted title
    author: String?,         // override auto-extracted author
    authors: [String]?,      // list of authors
    metadata: Object?,       // arbitrary metadata
    format_origin: String?,  // original format (e.g., "pdf", "html")
  }
  → Accepts a source and kicks off the ingestion pipeline
  → Provide either content (base64) or url — at least one required
  → source_type required when using content directly
  → Returns: { id }

GET /api/v1/sources
  Query: { limit?, offset? }
  → Paginated list of sources, ordered by ingested_at descending

GET /api/v1/sources/{id}
  → Source metadata + ingestion status + reliability score

GET /api/v1/sources/{id}/statements
  → Statement tree for a source (or AST chunks if source_type=code)
  → Each statement includes canonical source byte offsets

POST /api/v1/sources/{id}/reprocess
  → Idempotent reprocessing: re-runs the full pipeline on existing content
  → Supersedes previous extractions, preserves source identity
  → Returns: { id, chunks_created, entities_extracted, edges_created }

DELETE /api/v1/sources/{id}
  → Cascading deletion: extractions → aliases → statements/chunks → orphaned edges → orphaned nodes
  → Returns: { deleted, extractions_deleted, nodes_deleted, edges_deleted }
```

### Search

```
POST /api/v1/search
  Body: {
    query: String,
    strategy: "auto" | "balanced" | "precise" | "exploratory" | "recent" | "graph_first" | "global" | "custom",
    weights: { vector, lexical, temporal, graph, structural, global }?,
    limit: Int?,
    mode: "results" | "context",                  // results (default) or assembled context
    granularity: "section" | "paragraph" | "source",  // section (default), paragraph, or full source
    source_layers: ["spec" | "design" | "code" | "research"]?,  // filter by source URI prefix layer
    filters: {
      source_types: [String]?,
      date_range: { start, end }?,
      node_types: [String]?,
      min_confidence: Float?,
    }?
  }
  → Returns: [SearchResult] or ContextResponse depending on mode
  → strategy "auto" (default) uses SkewRoute for adaptive strategy selection based on
    score distribution analysis (min-max normalized Gini coefficient)
  → granularity controls content resolution:
      "section"   — walk up to the parent section chunk (default)
      "paragraph" — use the matched chunk content as-is
      "source"    — use the full source normalized_content
  → source_layers filters results by URI prefix layer (e.g. spec/, docs/adr/, *.rs, research papers)

POST /api/v1/search/feedback
  Body: { trace_id, result_id, relevant: bool, comment? }
  → Records relevance feedback for a search result
  → Used to build eval datasets and tune retrieval quality
```

### Nodes

```
GET /api/v1/nodes/{id}
  Query: { include_edges?, include_aliases? }
  → Node with properties, aliases, and connected edges

GET /api/v1/nodes/{id}/neighborhood
  Query: { hops?, edge_types?, limit? }
  → k-hop neighborhood around a node

GET /api/v1/nodes/{id}/provenance
  → Full provenance chain: extractions → chunks → sources

GET /api/v1/nodes/landmarks
  → Nodes ordered by mention count (proxy for betweenness centrality)

POST /api/v1/nodes/resolve
  Body: { name, description?, context? }
  → Entity resolution: find existing node or suggest creation

POST /api/v1/nodes/merge
  Body: { source_nodes: [UUID], target_node: UUID }
  → Merge multiple nodes into one. Creates SUPERSEDES edges.
  → Returns: { merged_node, audit_log_id }

POST /api/v1/nodes/{id}/split
  Body: { new_nodes: [{ name, type, description, edge_ids }] }
  → Split a wrongly-merged node into distinct entities
  → Returns: { new_nodes: [UUID], audit_log_id }

POST /api/v1/nodes/{id}/correct
  Body: { description, reason }
  → Updates node description, re-embeds, creates correction source

POST /api/v1/nodes/{id}/annotate
  Body: { annotation, tags? }
  → Adds annotation to node properties, creates annotation source
```

### Edges

```
GET /api/v1/edges/{id}
  → Edge with properties and provenance

POST /api/v1/edges/{id}/correct
  Body: { rel_type?, properties?, reason }
  → Updates edge, creates correction source

DELETE /api/v1/edges/{id}
  → Bi-temporal invalidation (sets invalid_at, not physical delete)
```

### Graph

```
GET /api/v1/graph/stats
  → Node count (by type, including code entity types), edge count (by type), density,
    component count, statement count, section count

GET /api/v1/graph/communities
  → Current community structure (k-core decomposition)

GET /api/v1/graph/topology
  → Topological statistics: degree distribution, clustering coefficient
```

### Audit

```
GET /api/v1/audit
  Query: { action?, target_type?, target_id?, since?, limit? }
  → Paginated audit log of system decisions
```

### Memory (Agent-Friendly Wrapper)

```
POST /api/v1/memory
  Body: { content, importance?, tags?, context?, supersedes_id? }
  → Store a memory (creates an observation source with memory metadata)
  → Returns: { memory_id }

POST /api/v1/memory/recall
  Body: { query, tags?, limit?, min_confidence? }
  → Search memories ranked by relevance, confidence, freshness
  → Returns: [MemoryResult]

GET /api/v1/memory/status
  → Memory count, top tags, last memory timestamp

DELETE /api/v1/memory/{id}
  Body: { reason? }
  → Soft-forget a memory (filtered from future recall, audit trail preserved)
```

### Knowledge Curation (Human-in-the-Loop)

User corrections and annotations feed back into the graph. Every curation action is recorded as a new source (type: `user_correction`, reliability: 0.9) with full provenance.

All curation actions are:
- Attributed to a user identity
- Stored as sources for provenance tracking
- Auditable (who changed what, when, why)
- Reversible (corrections create new edges, don't destroy old ones)

See nodes `correct`/`annotate` and edges `correct`/`delete` endpoints above.

### Admin

```
GET /api/v1/admin/health
  → System health, PG connectivity, sidecar status

GET /api/v1/admin/metrics
  → Node/edge/chunk/source counts, index sizes, query latencies

POST /api/v1/admin/graph/reload
  → Force full reload of graph sidecar from PG

POST /api/v1/admin/publish/{source_id}
  Query: { clearance_level: 1 | 2 }
  → Promote source + derivatives to federated clearance level

POST /api/v1/admin/consolidate
  Query: { tier: "batch" | "deep" }
  → Trigger manual consolidation run

POST /api/v1/admin/gc
  → Provenance-based garbage collection: remove nodes with zero active extractions

POST /api/v1/admin/ontology/cluster
  → Run ontology clustering (greedy agglomerative by cosine similarity)

POST /api/v1/admin/config-audit
  → Config drift detection: sidecar health checks + configuration warnings

GET /api/v1/admin/traces
  Query: { limit? }
  → Recent query traces for debugging retrieval quality

POST /api/v1/admin/traces/{id}/replay
  → Replay a traced query with different parameters for A/B comparison

POST /api/v1/admin/cache/clear
  → Clear the semantic query cache
  → Returns: { entries_cleared }

POST /api/v1/admin/edges/synthesize
  Body: { min_cooccurrences?: Int, max_degree?: Int }
  → Run co-occurrence edge synthesis: creates synthetic edges between nodes that
    frequently appear in the same chunks
  → min_cooccurrences: minimum co-occurrence count to create an edge (default 1)
  → max_degree: only create edges for nodes with degree ≤ this value (default 2)
  → Returns: { edges_created, candidates_evaluated }

GET /api/v1/admin/knowledge-gaps
  Query: { min_mentions?, max_extractions? }
  → Identify knowledge gaps: entities with high mention count but low extraction coverage
```

### Analysis (Cross-Domain)

```
POST /api/v1/analysis/verify-implementation
  Body: { concept: String, max_depth?: Int }
  → Traces from a research concept through spec topics and components to code entities
  → Returns: { concept, spec_topics: [...], components: [...], code_entities: [...], gaps: [...] }

POST /api/v1/analysis/erosion
  Body: { component_id?: UUID, threshold?: Float }
  → Measures semantic drift between Component descriptions and their code entities
  → drift(component) = 1 - mean(cosine(component.embedding, code_node.embedding))
  → Returns: { components: [{ id, name, drift_score, code_entities: [...] }] }

POST /api/v1/analysis/whitespace
  Body: { min_cluster_size?: Int }
  → Finds dense research clusters with no corresponding Component or spec topic links
  → Returns: { gaps: [{ research_cluster, concepts: [...], density, nearest_component }] }

POST /api/v1/analysis/blast-radius
  Body: { entity_id: UUID, max_hops?: Int }
  → Traverses structural and semantic edges to compute full impact of modifying an entity
  → Returns: { entity, direct_deps: [...], transitive_deps: [...], affected_components: [...], affected_specs: [...] }

POST /api/v1/analysis/critique
  Body: { proposal: String }
  → Searches the graph for competing approaches, contradicting claims, and conflicting implementations
  → Returns: { arguments_for: [...], arguments_against: [...], conflicts: [...], alternatives: [...] }

POST /api/v1/analysis/coverage
  Body: { layer?: "spec" | "code" | "research" }
  → Measures coverage between knowledge domains
  → Returns: { coverage_score, covered_topics: [...], uncovered_topics: [...], orphan_code: [...] }
```

### Components

```
GET /api/v1/components
  Query: { limit?, offset? }
  → Paginated list of Component bridge nodes

GET /api/v1/components/{id}
  → Component with linked spec topics, code entities, and research concepts

POST /api/v1/components
  Body: { name, description, source_id?, metadata? }
  → Create a Component bridge node
  → Returns: { id }

POST /api/v1/components/{id}/link
  Body: { entity_id: UUID, edge_type: "IMPLEMENTS_INTENT" | "PART_OF_COMPONENT" | "THEORETICAL_BASIS" }
  → Link a code entity, spec topic, or research concept to this component
```

### MCP (Model Context Protocol)

```
POST /api/v1/mcp/tools/list
  → List available MCP tools

POST /api/v1/mcp/tools/call
  Body: { name, arguments }
  → Call an MCP tool by name
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

verify_implementation(concept)
  → Trace research concept through spec to code

detect_erosion(component_id?, threshold?)
  → Measure semantic drift between spec and code

find_whitespace()
  → Find research areas with no implementation

blast_radius(entity_id, max_hops?)
  → Compute impact of modifying an entity

critique_proposal(proposal)
  → Find counterarguments from the knowledge graph

coverage_analysis(layer?)
  → Measure cross-domain coverage
```

## Authentication / Authorization

Optional API key authentication via the `COVALENCE_API_KEY` environment variable.

- **When `COVALENCE_API_KEY` is not set:** All requests pass through without authentication (development mode).
- **When `COVALENCE_API_KEY` is set:** All requests to non-public paths must include an `Authorization: Bearer <key>` header with a matching key.
  - Returns `401 Unauthorized` with `{"error": {"code": "auth_error", "message": "..."}}` if:
    - The header is missing entirely
    - The authorization scheme is not `Bearer`
    - The token does not match the configured key
- **Public paths (exempt from auth):** `/health`, `/openapi.json`, `/docs/*`, `/dashboard/*`

Multi-user/tenant support is a future concern.

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
