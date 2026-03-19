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

GET /api/v1/sources/{id}/statements          (planned)
  → Statement tree for a source (or AST chunks if source_type=code)
  → Each statement includes canonical source byte offsets

GET /api/v1/sources/{id}/chunks
  → Chunks belonging to a source
  → Returns: [{ id, source_id, level, ordinal, content, token_count }]

POST /api/v1/sources/{id}/reprocess
  → Synchronous reprocessing: re-runs the full pipeline on existing content
  → Supersedes previous extractions, preserves source identity
  → Returns: { source_id, extractions_superseded, chunks_deleted, chunks_created, content_version }

POST /api/v1/sources/{id}/queue-reprocess
  → Asynchronous reprocessing via the persistent retry queue
  → Enqueues the job and returns immediately; the background worker processes it
    with automatic retries on transient failures
  → Returns: { enqueued: bool, job_id?: UUID }

DELETE /api/v1/sources/{id}
  → Cascading deletion: extractions → aliases → statements/chunks → orphaned edges → orphaned nodes
  → TMS cascade: surviving nodes/edges have epistemic opinions recalculated
  → Returns: { deleted, chunks_deleted, extractions_deleted, statements_deleted,
               sections_deleted, nodes_deleted, edges_deleted,
               nodes_recalculated, edges_recalculated }
```

### Search

```
POST /api/v1/search
  Body: {
    query: String,
    strategy: "auto" | "balanced" | "precise" | "exploratory" | "recent" | "graph_first" | "global" | "custom",
    weights: { vector, lexical, temporal, graph, structural, global }?,
    limit: Int?,
    min_confidence: Float?,               // epistemic confidence threshold (0.0–1.0)
    node_types: [String]?,                // restrict to specific node types
    entity_classes: [String]?,            // restrict to entity classes: code, domain, actor, analysis
    source_types: [String]?,              // restrict to source types (e.g. "document", "code")
    source_layers: [String]?,             // restrict by source domain layer: spec, design, code, research, external
    date_range_start: String?,            // ISO 8601 start of date range
    date_range_end: String?,              // ISO 8601 end of date range
    mode: "results" | "context",          // results (default) or assembled context
    granularity: "section" | "paragraph" | "source",  // section (default), paragraph, or full source
    hierarchical: bool?,                  // coarse-to-fine: find sources first, then chunks (default false)
    graph_view: String?,                  // orthogonal graph view for BFS: causal, temporal, entity, structural, all
  }
  → Returns: [SearchResult] or ContextResponse depending on mode
  → strategy "auto" (default) uses SkewRoute for adaptive strategy selection based on
    score distribution analysis (min-max normalized Gini coefficient)
  → granularity controls content resolution:
      "section"   — walk up to the parent section chunk (default)
      "paragraph" — use the matched chunk content as-is
      "source"    — use the full source normalized_content
  → source_layers filters results by source domain label (code, spec, design, research, external)
  → entity_classes filters by the graph type system classification (code, domain, actor, analysis)
  → graph_view restricts which edges the graph search dimension traverses during BFS;
    orthogonal views (ADR-0018) isolate causal, temporal, entity, or structural edge subsets
  → hierarchical mode first identifies relevant sources, then retrieves chunks only from those
    sources — useful for broad exploratory queries
  → Post-retrieval quality filter removes bibliography, boilerplate, metadata-only, and
    title-only chunks from results

POST /api/v1/search/feedback
  Body: { query, result_id, relevance: Float, comment? }
  → Records relevance feedback for a search result (relevance is 0.0–1.0)
  → Used to build eval datasets and tune retrieval quality
```

### Ask (LLM Synthesis)

```
POST /api/v1/ask
  Body: {
    question: String,              // natural language question
    max_context: Int?,             // max search results to use as context (default 15)
    strategy: String?,             // search strategy: auto, balanced, precise, etc.
    model: String?,                // LLM model override: haiku, sonnet, opus, gemini, copilot
  }
  → Searches the knowledge graph across all dimensions, enriches context with
    provenance and confidence metadata, and sends to an LLM for grounded synthesis
  → Requires a chat backend to be configured (COVALENCE_CHAT_CLI_COMMAND or HTTP backend)
  → Returns: {
      answer: String,              // synthesized answer grounded in graph evidence
      citations: [{
        source: String,            // source name or URI
        snippet: String,           // relevant excerpt from the source
        result_type: String,       // chunk, statement, section, node, etc.
        confidence: Float,         // epistemic confidence score
      }],
      context_used: Int,           // number of search results used as context
    }
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
  → Returns: { node_count, edge_count, semantic_edge_count, synthetic_edge_count, density, component_count }

GET /api/v1/admin/graph/invalidated-stats
  Query: { type_limit?, node_limit? }
  → Statistics about invalidated (bi-temporally deleted) edges
  → Surfaces controversy indicators: nodes with high invalidated-edge counts
  → Returns: { total_invalidated, total_valid, top_types: [...], top_nodes: [...] }

POST /api/v1/admin/publish/{source_id}
  Query: { clearance_level: 1 | 2 }
  → Promote source + derivatives to federated clearance level

POST /api/v1/admin/consolidate
  Query: { tier: "batch" | "deep" }
  → Trigger manual consolidation run

POST /api/v1/admin/gc
  → Provenance-based garbage collection: remove nodes with zero active extractions

POST /api/v1/admin/ontology/cluster
  Body: { level?: "entity" | "entity_type" | "rel_type", min_cluster_size?: Int, dry_run?: bool }
  → Run ontology clustering (HDBSCAN density-based)
  → Default dry_run=true: report clusters without writing
  → Returns: { applied, cluster_count, clusters: [...], noise_labels: [...] }

POST /api/v1/admin/config-audit
  → Config drift detection: sidecar health checks + configuration warnings
  → Returns: { current_config, sidecars: [{ name, configured, reachable, fallback? }], warnings: [...] }

POST /api/v1/admin/tier5/resolve
  Body: { min_cluster_size?: Int }
  → Trigger Tier 5 HDBSCAN batch entity resolution
  → Returns: { entities_processed, clusters_formed, clustered_resolved, noise_promoted, skipped_no_embedding }

POST /api/v1/admin/nodes/cleanup
  Body: { dry_run?: bool }
  → Retroactively clean noise entities from the graph
  → Scans all nodes through the noise entity filter; default dry_run=true (report only)
  → Returns: { nodes_identified, nodes_deleted, edges_removed, aliases_removed, dry_run, entities: [...] }

POST /api/v1/admin/nodes/backfill-embeddings
  → Backfill embeddings for nodes that are missing them
  → Returns: { total_missing, embedded, failed }

POST /api/v1/admin/nodes/summarize-code
  → Generate LLM semantic summaries for code nodes without existing summaries
  → Returns: { nodes_found, summarized, failed }

POST /api/v1/admin/opinions/seed
  → Seed epistemic opinions on all nodes and edges from extraction evidence
  → Returns: { nodes_seeded, nodes_vacuous, edges_seeded, edges_vacuous }

POST /api/v1/admin/edges/bridge
  Body: { min_similarity?: Float, max_edges_per_node?: Int }
  → Create cross-domain bridge edges between code entities and concept nodes
  → min_similarity: cosine similarity threshold (default 0.6)
  → max_edges_per_node: cap on bridge edges per code node (default 3)
  → Returns: { code_nodes_checked, edges_created, skipped_existing }

POST /api/v1/admin/raptor
  → Trigger RAPTOR recursive summarization across all sources
  → Builds hierarchical summary chunks enabling multi-resolution retrieval
  → Returns: { sources_processed, sources_skipped, summaries_created, llm_calls, embed_calls, errors }

GET /api/v1/admin/traces
  Query: { limit? }
  → Recent query traces for debugging retrieval quality

POST /api/v1/admin/traces/{id}/replay
  → Replay a traced query with different parameters for A/B comparison
  → Returns: { trace_id, results: [SearchResult] }

POST /api/v1/admin/cache/clear
  → Clear the semantic query cache
  → Returns: { entries_cleared }

POST /api/v1/admin/edges/synthesize
  Body: { min_cooccurrences?: Int, max_degree?: Int }
  → Run co-occurrence edge synthesis: creates synthetic edges between nodes that
    frequently appear in the same chunks (UNION of chunk-level + statement-level co-occurrences)
  → min_cooccurrences: minimum co-occurrence count to create an edge (default 1)
  → max_degree: only create edges for nodes with degree ≤ this value (default 2)
  → Returns: { edges_created, candidates_evaluated }

GET /api/v1/admin/knowledge-gaps
  Query: { min_in_degree?, min_label_length?, exclude_types?, limit? }
  → Identify knowledge gaps: entities with high in-degree but low out-degree
  → exclude_types defaults to "person,organization,event,location,publication,other"
    to filter bibliographic noise; pass empty string for all types
  → Returns: { gap_count, gaps: [{ node_id, canonical_name, node_type, in_degree, out_degree, gap_score, referenced_by }] }

GET /api/v1/admin/health-report
  → Comprehensive meta-loop health report aggregating metrics, coverage, pipeline
    progress, and queue status into a single response
  → Designed to be the first call in every meta-loop iteration
  → Returns: {
      graph: { nodes, edges, components },
      sources: { total, domains: { code: N, spec: N, ... } },
      entities: { total, classes: { code: N, domain: N, ... } },
      pipeline: { entity_summaries, total_code_entities, entity_summary_pct,
                  file_summaries, total_code_files, file_summary_pct },
      queue: [{ kind, status, count }]
    }

GET /api/v1/admin/data-health
  → Data hygiene preview — read-only report of stale, orphaned, and duplicated data
  → Surfaces: superseded sources, orphan nodes (no extractions), duplicate entities,
    unembedded nodes, unsummarized code entities
  → Returns: JSON report (schema varies by data condition)
```

### Queue Management

```
GET /api/v1/admin/queue/status
  → Summary of the persistent retry queue grouped by job kind and status
  → Returns: { rows: [{ kind, status, count }] }

POST /api/v1/admin/queue/retry
  Body: { kind?: String }
  → Retry all failed/dead jobs, optionally filtered by kind
  → Returns: { retried: Int }

GET /api/v1/admin/queue/dead
  Query: { limit? }
  → List dead-letter jobs (permanently failed after max attempts)
  → Returns: { jobs: [{ id, kind, attempt, max_attempts, last_error, dead_reason,
                         payload, created_at, updated_at }] }

POST /api/v1/admin/queue/dead/clear
  Body: { kind?: String }
  → Delete dead-letter jobs, optionally filtered by kind
  → Returns: { deleted: Int }

POST /api/v1/admin/queue/dead/resurrect
  Body: { kind?: String }
  → Resurrect dead jobs — reset to pending so they retry from scratch
  → Returns: { resurrected: Int }

POST /api/v1/admin/queue/summarize-all
  → Enqueue semantic summary jobs for all unsummarized code entities
  → Returns: { enqueued: Int }

POST /api/v1/admin/queue/compose-all
  → Enqueue source summary composition for all code sources with entity summaries
  → Returns: { enqueued: Int }
```

### Analysis (Cross-Domain)

```
POST /api/v1/analysis/bootstrap
  → Bootstrap Component bridge nodes for the 9 known subsystems
  → Idempotent: skips already-existing components
  → Returns: { components_created, components_existing, components_embedded }

POST /api/v1/analysis/link
  Body: { min_similarity?: Float, max_edges_per_component?: Int }
  → Create cross-domain bridge edges between Components and code/spec/research entities
  → Edge types: PART_OF_COMPONENT, IMPLEMENTS_INTENT, THEORETICAL_BASIS
  → Returns: { part_of_edges, intent_edges, basis_edges, skipped_existing }

POST /api/v1/analysis/coverage
  → Detect orphaned code entities and unimplemented spec concepts
  → Returns: { orphan_code: [...], unimplemented_specs: [...], coverage_score }

POST /api/v1/analysis/erosion
  Body: { threshold?: Float }
  → Measures semantic drift between Component descriptions and their code entities
  → drift(component) = 1 - mean(cosine(component.embedding, code_node.embedding))
  → threshold: report components above this drift score (default 0.3)
  → Returns: { eroded_components: [{ component_id, component_name, spec_intent, drift_score,
                divergent_nodes: [{ node_id, name, summary?, distance }] }], total_components }

POST /api/v1/analysis/blast-radius
  Body: { target: String, max_hops?: Int, include_invalidated?: bool }
  → Traverses structural and semantic edges to compute full impact of modifying an entity
  → target can be a node name or UUID
  → Returns: { target_node_id, target_name, target_node_type, component?,
               affected_by_hop: [{ hop_distance, nodes: [...] }], total_affected }

POST /api/v1/analysis/whitespace
  Body: { min_cluster_size?: Int, domain?: String }
  → Finds research sources with no corresponding Component or spec topic bridges
  → Returns: { gaps: [{ source_id, title, uri?, node_count, representative_nodes,
               connected_components, connected_spec_topics, assessment }],
               total_research_sources, unbridged_sources, whitespace_score }

POST /api/v1/analysis/verify
  Body: { research_query: String, component?: String }
  → Verify research-to-execution alignment through the Component bridge layer
  → Searches for research-domain and code-domain nodes matching the query
  → Returns: { research_query, research_matches: [...], code_matches: [...],
               alignment_score?, component? }

POST /api/v1/analysis/critique
  Body: { proposal: String }
  → Generate a dialectical critique of a design proposal using graph evidence
  → Searches research, spec, and code domains for supporting/opposing evidence
  → If a chat backend is available, synthesizes counter-arguments and recommendations
  → Returns: { proposal, research_evidence: [...], spec_evidence: [...],
               code_evidence: [...], synthesis?: { counter_arguments, supporting_arguments,
               recommendation } }

POST /api/v1/analysis/alignment
  Body: {
    checks: [String]?,          // which checks to run: code_ahead, spec_ahead,
                                //   design_contradicted, stale_design (empty = all)
    min_similarity: Float?,     // embedding similarity threshold for matching (default 0.4)
    limit: Int?,                // max items per check (default 20)
  }
  → Cross-domain alignment report comparing entities across spec, design, code,
    and research domains to surface misalignments
  → Returns: {
      code_ahead: [...],            // code entities with no matching spec concept
      spec_ahead: [...],            // spec concepts with no implementing code
      design_contradicted: [...],   // design decisions potentially contradicted by research
      stale_design: [...],          // design docs diverging from code reality
    }
  → Each item: { check, name, domain, node_type, closest_match_score?, closest_match_name?,
                  closest_match_domain?, reason }
```

### Components (planned)

Component CRUD endpoints are planned but not yet implemented as dedicated routes.
Component creation is currently handled via `POST /api/v1/analysis/bootstrap` (which
creates the 9 known subsystem Components) and Component linking via
`POST /api/v1/analysis/link` (which creates bridge edges automatically).

```
GET /api/v1/components                   (planned)
  Query: { limit?, offset? }
  → Paginated list of Component bridge nodes

GET /api/v1/components/{id}              (planned)
  → Component with linked spec topics, code entities, and research concepts

POST /api/v1/components                  (planned)
  Body: { name, description, source_id?, metadata? }
  → Create a Component bridge node
  → Returns: { id }

POST /api/v1/components/{id}/link        (planned)
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

For Claude/agent integration via MCP protocol. Available via `POST /api/v1/mcp/tools/call`.

### Implemented Tools

```
search(query, strategy?, limit?)
  → Fused search across all dimensions

get_node(id)
  → Node details with neighborhood summary

get_provenance(node_id)
  → Provenance chain for a node (extraction, chunk, source counts)

ingest_source(content, source_type, mime?)
  → Submit a source for ingestion (raw text, not base64)

traverse(start_node_id, hops?)
  → Graph neighborhood exploration

resolve_entity(name)
  → Find an existing entity by name, or null if not found

list_communities()
  → Community overview with top nodes per community

get_contradictions(node_id?)
  → Active contradictions in the knowledge base

memory_store(content, topic?)
  → Store a memory (creates an observation source)

memory_recall(query, limit?, topic?, min_confidence?)
  → Recall relevant memories using semantic search

memory_forget(id)
  → Delete a memory by its source ID
```

### Analysis Tools (REST only, not yet wired as MCP dispatch targets)

The following tools are available as REST endpoints under `/api/v1/analysis/` but
are not yet exposed through the MCP tool dispatch. They can be accessed directly
via their HTTP endpoints.

```
verify_implementation          → POST /api/v1/analysis/verify
detect_erosion                 → POST /api/v1/analysis/erosion
find_whitespace                → POST /api/v1/analysis/whitespace
blast_radius                   → POST /api/v1/analysis/blast-radius
critique_proposal              → POST /api/v1/analysis/critique
coverage_analysis              → POST /api/v1/analysis/coverage
alignment_report               → POST /api/v1/analysis/alignment
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
