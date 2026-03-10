# Design: API & Context Assembly

## Status: implemented (core), partial (MCP, feedback)

## Spec Sections: 08-api.md, 06-search.md

## Architecture Overview

The API layer (axum) exposes Covalence's capabilities as REST endpoints. The context assembly pipeline transforms raw search results into coherent, budgeted context windows for LLM generation. MCP (Model Context Protocol) integration exposes Covalence as an AI tool provider.

## API Surface (43 endpoints)

### Fully Implemented ✅

| Group | Endpoints | Notes |
|-------|-----------|-------|
| **Sources** | `POST /sources`, `GET /sources`, `GET /sources/{id}`, `DELETE /sources/{id}`, `GET /sources/{id}/chunks` | Full CRUD + chunk inspection |
| **Search** | `POST /search`, `POST /search/feedback` | Multi-dimension fused search + user feedback endpoint |
| **Nodes** | `GET /nodes/{id}`, `GET /nodes/{id}/neighborhood`, `GET /nodes/{id}/provenance`, `POST /nodes/resolve`, `POST /nodes/merge`, `GET /nodes/landmarks`, `POST /nodes/{id}/split`, `POST /nodes/{id}/correct`, `POST /nodes/{id}/annotate` | Full graph entity management |
| **Edges** | `GET /edges/{id}`, `DELETE /edges/{id}`, `POST /edges/{id}/correct` | Edge inspection and correction |
| **Graph** | `GET /graph/stats`, `GET /graph/communities`, `GET /graph/topology` | Graph analytics |
| **Admin** | `POST /admin/graph/reload`, `POST /admin/publish/{source_id}`, `POST /admin/consolidate`, `POST /admin/ontology/cluster`, `GET /audit`, `GET /health` | Operational management |
| **MCP** | `POST /mcp/tools/list`, `POST /mcp/tools/call` | Model Context Protocol for AI tool integration |

### Context Assembly Pipeline

| Stage | Status | Description |
|-------|--------|-------------|
| **Deduplicate** | ✅ | Cosine similarity threshold (0.95) removes near-duplicate chunks |
| **Diversify** | ✅ | `max_per_source: 3` ensures source diversity |
| **Budget** | ✅ | `max_tokens: 8000` caps context window |
| **Expand** | 🟡 | Neighborhood expansion exists in search, not fully in context assembly |
| **Order** | 🟡 | Results ordered by relevance score, not by semantic coherence |
| **Annotate** | 🟡 | Confidence metadata exists but not formatted into context |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **MCP integration** | Tools listed and callable | Limited tool set exposed; no streaming |
| **Search feedback** | Endpoint exists | Not wired to learning — feedback stored but not used to adjust ranking |
| **Node correction** | Endpoint exists | Manual entity correction works, but doesn't trigger re-embedding or re-extraction |
| **Source metadata** | `title`, `author`, `created_date` in model | Never populated from API — `CreateSourceRequest` doesn't accept title/author |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **URL-based ingestion** | #28: fetch from URL, extract metadata | High |
| **Streaming responses** | Spec 08: SSE for long operations | Medium |
| **Batch ingestion** | Spec 08: bulk source upload | Medium |
| **Rate limiting** | Spec 08: per-client rate limits | Medium |
| **Auth** | Spec 08: API key / JWT authentication | Medium — currently open |
| **Webhooks** | Spec 08: event notifications | Low |
| **GraphQL** | Spec 08: alternative query interface | Low |
| **Source title/author in create** | #28: metadata on ingestion | High — easy fix |

## Key Design Decisions

### Why REST over GraphQL
REST is simpler for the ingestion (create/read/delete) use case. Graph queries are better served by dedicated endpoints (`/nodes/{id}/neighborhood`) than a generic GraphQL resolver. MCP provides the AI-integration story.

### Why MCP
MCP (Model Context Protocol) makes Covalence callable by any MCP-compatible AI system (Claude, GPT, etc.). Instead of building a chat UI, Covalence becomes a "knowledge backend" that any AI can query.

### Why context budgeting matters
LLMs have finite context windows. Shoving all search results in produces worse answers than a curated, deduplicated, diversified subset. The `max_tokens: 8000` default leaves room for the system prompt + user query + generation.

### Why source diversity (max_per_source: 3)
Without diversity, a single long document dominates the context. Capping per-source results ensures the LLM sees multiple perspectives. This is especially important for epistemic quality — multiple sources corroborating a claim produces higher confidence.

## Gaps Identified

1. **No auth** — the API is completely open. Fine for local development, but federation requires peer authentication and clearance-level enforcement.

2. **Source metadata gap** — the `Source` model has `title`, `author`, `created_date` fields that are never populated. The create endpoint doesn't accept them. Easy fix.

3. **Search feedback is dead-end** — feedback is stored but never used. Should feed into: (a) relevance model training, (b) epistemic confidence adjustment, (c) abstention threshold tuning.

4. **Context ordering is naive** — results are ordered by score, but "Lost in the Middle" (Liu et al. 2023, in KB) showed LLMs attend to first and last positions most. Context should place high-confidence results at the boundaries.

5. **No context provenance in response** — search results include scores but not the source chain (which document → which chunk → which entity). The trace exists internally but isn't exposed.

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| Lost in the Middle | Liu et al. 2023 | ✅ Ingested |
| Fusion-in-Decoder | Izacard & Grave 2020 | ✅ Ingested |
| RETRO | Borgeaud et al. 2022 | ✅ Ingested |
| Information foraging | Pirolli & Card 1999 | 🔄 Worker ingesting |
| Semantic caching | Dar et al. 1996 | 🔄 Worker ingesting |
| MCP specification | Anthropic 2024 | ❌ Not ingested (evolving spec) |

## Next Actions

1. Add `title`, `author` fields to `CreateSourceRequest`
2. Wire search feedback to epistemic confidence adjustment
3. Implement "Lost in the Middle" reordering in context assembly
4. Expose provenance chain in search responses
5. Add auth layer (API key minimum, JWT for federation)
