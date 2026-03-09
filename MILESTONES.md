# Milestones

Phased roadmap for Covalence development. Each milestone is self-contained and delivers testable functionality.

## M0 — Project Skeleton *(complete)*

- [x] Git repo initialized
- [x] CLAUDE.md with full architectural context
- [x] Rust workspace: core, api, migrations crates
- [x] Go CLI skeleton (Cobra)
- [x] Initial migration (001_initial_schema.sql)
- [x] OpenAPI setup (utoipa + Swagger UI)
- [x] Makefile
- [x] CI pipeline
- [x] ADRs for all pre-decided architectural choices
- [x] This milestones document

## M1 — Storage Foundation *(complete)*

- [x] Domain model types (Source, Chunk, Node, Edge, Article, Extraction, NodeAlias)
- [x] Newtype IDs (SourceId, NodeId, EdgeId, etc.) with sqlx Type/Encode/Decode
- [x] Subjective Logic opinion type (full: fusion, discount, deduction, json roundtrip)
- [x] Repository traits (SourceRepo, ChunkRepo, NodeRepo, EdgeRepo, ArticleRepo, ExtractionRepo, NodeAliasRepo, AuditLogRepo)
- [x] PostgreSQL repository implementations (all 8 repos with full CRUD)
- [x] Database connection pool setup (PgRepo::new with PgPoolOptions)
- [x] Config loading from environment (Config::from_env with all subsystem configs)
- [x] Integration tests against real PG (11 tests covering all repos)

**Note:** Clearance levels, causal hierarchy, and audit log model also implemented here.

## M2 — Graph Sidecar *(complete)*

- [x] GraphSidecar struct (DiGraph + UUID->NodeIndex index)
- [x] Initial load from PG (full_reload in sync module)
- [x] Outbox sync loop (LISTEN/NOTIFY + 5s polling fallback)
- [x] BFS/DFS traversal with hop-decay scoring
- [x] Shortest path (BFS-based)
- [x] Filtered views (clearance-based NodeFiltered + edge-type EdgeFiltered)
- [x] Thread safety (Arc<RwLock<GraphSidecar>> as SharedGraph)
- [x] Admin endpoint: force reload

## M3 — Search *(complete)*

- [x] Vector search dimension (pgvector cosine distance via halfvec)
- [x] Lexical search dimension (tsvector + websearch + trigram ILIKE fallback)
- [x] Temporal search dimension (recency decay + time-range filter)
- [x] RRF fusion with configurable weights (rrf_fuse implementation)
- [x] Query strategies (balanced, precise, exploratory, recent, graph_first, custom)
- [x] SearchQuery, SearchDimension trait, DimensionKind enum
- [x] Graph search dimension (graph traversal as SearchDimension)
- [x] Structural search dimension (structural importance as SearchDimension)
- [x] Search API endpoint (POST /search)
- [x] Result shape with confidence and provenance

**Note:** All 5 search dimensions implemented and wired to SearchService with RRF fusion. Search is functional end-to-end. FusedResult includes confidence, entity_type, name, and snippet fields.

## M4 — Basic Ingestion *(complete)*

- [x] Source acceptance (SHA-256 hash dedup via compute_content_hash)
- [x] Markdown parser (title extraction, text/plain support)
- [x] Hierarchical chunker (document -> section -> paragraph splitting)
- [x] Text normalization (Unicode NFC, whitespace collapse, control char strip)
- [x] Embedder trait + MockEmbedder
- [x] Embedding via OpenAI API (reqwest-based HTTP client with batching)
- [x] Store chunks + embeddings in PG (wired end-to-end: parse -> normalize -> chunk -> embed -> store)
- [x] POST /sources endpoint
- [x] GET /sources/:id endpoint

**Note:** SourceService.ingest() runs the full pipeline: parse, normalize, chunk, embed, store in PG. Real OpenAI embedder with batching support; SourceService creates embedder from config when OPENAI_API_KEY is set.

## M5 — Full Search Fusion *(complete)*

- [x] PageRank computation (power iteration with dangling node handling)
- [x] Personalized PageRank
- [x] Topological confidence scoring (alpha*PR + beta*path_diversity)
- [x] Graph traversal dimension (BFS traversal as SearchDimension impl)
- [x] Structural search dimension (structural importance as SearchDimension impl)
- [x] Confidence integration in search results (FusedResult with confidence, entity_type, name, snippet; SearchFilters with min_confidence, node_types, date_range)
- [x] Parent-child context injection (paragraph chunks enriched with truncated parent section context)
- [x] Multi-granularity search (vector + lexical dimensions search chunks, nodes, and articles; result_type tagging)

## M6 — LLM Extraction + Entity Resolution *(complete)*

- [x] Extractor trait + MockExtractor (extraction types defined)
- [x] EntityResolver trait + MockResolver (resolution types defined)
- [x] ExtractedEntity, ExtractedRelationship, ExtractionResult types
- [x] MatchType enum (Exact, Fuzzy, Vector, New)
- [x] LLM-driven entity/relationship extraction (OpenAI-compatible HTTP client)
- [x] Structured output parsing
- [x] Entity resolution (exact + alias + fuzzy trigram matching via PgResolver)
- [x] Extraction provenance records (wired end-to-end: extract -> resolve -> create node/edge -> Extraction record)
- [x] Advisory lock-based concurrency safety (pg_advisory_lock with SHA-256 entity name hash)
- [x] Node alias management (auto-alias on fuzzy match — ensure_alias in SourceService)
- [x] Co-reference resolution (CorefResolver: abbreviation/mention → canonical name mapping across chunks)

## M7 — Epistemic Model Phase 1 *(complete)*

- [x] Subjective Logic opinion operations (cumulative fusion, average fusion, discount, deduction, projection)
- [x] Source trust via Beta-binomial model (initial_trust per SourceType)
- [x] Dempster-Shafer evidence fusion (MassFunction + combination rule)
- [x] Cumulative fusion (both in Opinion and epistemic::fusion module)
- [x] CONTRADICTS/CONTENDS edges: DF-QuAD attack + circular attack resolution
- [x] SUPERSEDES/CORRECTS edges: temporal decay (apply_supersedes, apply_corrects, apply_append)
- [x] Convergence guard (compute_epistemic_closure with damped fixed-point iteration)
- [x] Composite confidence computation (opinion projected * topo adjustment)
- [x] Epistemic delta tracking (compute_delta, EpistemicDelta with significance threshold)
- [x] Extraction confidence x source reliability (wired: extraction confidence stored in provenance, source reliability in Source model)

## M8 — Batch Consolidation + Articles *(complete)*

- [x] BatchConsolidator trait + BatchJob/BatchStatus types
- [x] DeepConsolidator trait + DeepConfig/DeepReport types
- [x] ConsolidationScheduler (batch/deep timing + delta-triggered runs)
- [x] GraphBatchConsolidator (real BatchConsolidator impl wiring topic clustering + contention detection)
- [x] Topic clustering (community-based — cluster_sources_by_community via graph communities)
- [x] LLM-driven article compilation (ArticleCompiler trait + LlmCompiler + ConcatCompiler fallback)
- [x] Article storage with embeddings (GraphBatchConsolidator embeds article summaries via Embedder)
- [x] Epistemic delta tracking (report_delta + accumulated_delta + significance threshold in scheduler)
- [x] Delta-triggered re-compilation (scheduler.should_run_batch checks accumulated delta against threshold)
- [x] Contention detection and queuing (detect_contentions via CONTRADICTS/CONTENDS edges)
- [x] Bayesian confidence aggregation (Beta conjugate updating: bayesian_aggregate with prior + weighted observations)

## M9 — API Completion *(complete)*

- [x] Health endpoint (GET /health)
- [x] Swagger UI at /docs
- [x] Service layer: SourceService, NodeService, EdgeService, ArticleService, AdminService, SearchService
- [x] NodeService.provenance() — full extraction->chunk->source chain
- [x] NodeService.neighborhood() — BFS via graph sidecar
- [x] SourceService.ingest() — dedup + source creation
- [x] Go CLI: source (add/list/get/delete/chunks), search (with filters), node (list/get/neighborhood/provenance), graph (stats/communities/topology), admin (health/reload/consolidate/metrics/publish), audit (list)
- [x] Go CLI internal HTTP client + table/JSON output
- [x] HTTP routes for source/node/edge/article/search CRUD
- [x] Graph endpoints (neighborhood, provenance, communities, stats)
- [x] Admin endpoints (reload, health, consolidate)
- [x] Admin endpoint: publish (clearance promotion), metrics (graph + source counts)
- [x] MCP tool interface (8 tools: search, get_node, get_provenance, ingest_source, traverse, resolve_entity, list_communities, get_contradictions)
- [x] Audit log endpoint
- [x] Node merge/split operations (edge retargeting, alias copying, soft-delete, audit logging)
- [x] OpenAPI spec fully generated (all 22 paths + 28 schemas registered in utoipa)

## M10 — Deep Consolidation + Advanced Epistemic *(complete)*

- [x] TrustRank computation (confidence-weighted trust propagation from seeds)
- [x] Community detection (Louvain with modularity optimization + coherence scoring)
- [x] Landmark node identification per community (via betweenness centrality)
- [x] Spreading activation for query expansion (ACT-R inspired, hop-decay)
- [x] Structural importance / betweenness centrality (Brandes' algorithm)
- [x] Domain topology map generation (build_topology with domains, links, landmarks + GET /graph/topology endpoint)
- [x] Landmark article identification (identify_landmark_articles via structural importance)
- [x] Bayesian Model Reduction (forgetting) — BMR keep_score, ACT-R base level, 3-tier eviction
- [x] Cross-domain bridge discovery (multi-community connector identification + scoring)
- [x] GraphDeepConsolidator (real DeepConsolidator impl wiring TrustRank + community + BMR + bridges)

**Note:** All graph algorithms implemented and tested. GraphDeepConsolidator wires them together via the DeepConsolidator trait. Domain topology and landmark articles now implemented.

## M11 — Spec Update: v2 Architecture *(complete)*

Schema, model, and pipeline updates to align with the updated spec (graphrag-spec):

- [x] Migration 002: bi-temporal edge columns (valid_from, valid_until, invalid_at, invalidated_by)
- [x] Migration 002: embedding dimension 768 → 2048 (chunks, nodes, articles, node_aliases)
- [x] Migration 002: chunk landscape fields (parent_alignment, extraction_method, landscape_metrics)
- [x] Migration 002: model_calibrations table
- [x] Migration 002: query_cache table
- [x] Edge model: bi-temporal fields + is_invalidated() + is_valid_at() helpers
- [x] Chunk model: landscape fields + ExtractionMethod enum
- [x] Edge repo: invalidate() + list_active() methods
- [x] Chunk repo: update_landscape() method
- [x] k-core community detection replacing Louvain (Matula-Beck algorithm)
- [x] Connected component splitting within same core level
- [x] Search strategy: 6th dimension (Global/Community Summary)
- [x] Search strategy: updated weight tables matching spec (6 dimensions)
- [x] SkewRoute adaptive strategy selection (Gini coefficient)
- [x] Landscape analysis pipeline (Stage 6: parent-child alignment, adjacent similarity, extraction gating)
- [x] cosine_similarity utility function
- [x] Context assembly pipeline (dedup, diversify, order, budget, annotate)
- [x] Semantic query cache types (CacheConfig, CachedResponse)
- [x] Abstention detection (score gate + insufficient context check)
- [x] Memory API types (store/recall/forget/status)
- [x] Memory API endpoints (POST /memory, POST /memory/recall, DELETE /memory/:id, GET /memory/status)
- [x] Voyage embedder (VoyageConfig + VoyageEmbedder implementing Embedder trait)
- [x] Community summary infrastructure (SummaryGenerator trait + ConcatSummaryGenerator)
- [x] Bi-temporal edge invalidation logic (detect_conflicts, temporal_overlap)
- [x] Source takedown support (UpdateClass enum + TakedownResult)
- [x] Voyage config in Config (VOYAGE_API_KEY, VOYAGE_BASE_URL env vars)
- [x] MCP tools expansion (memory_store, memory_recall, memory_forget)
- [x] CLI memory commands (store, recall, forget, status)

**Note:** 286+ tests passing, clippy clean. All changes are backward-compatible with existing M0-M10 functionality.

## Post-M11 — Bug Fixes (Issue #10) *(complete)*

Bring-up bugs found during initial deployment:

- [x] `env_or()` empty string fallthrough — now filters empty strings like `optional_env()`
- [x] `env_parse` / `env_parse_f64` same fix applied
- [x] Separate chat API config (`COVALENCE_CHAT_API_KEY`, `COVALENCE_CHAT_BASE_URL`) to avoid sharing Voyage embedder config with LLM extractor
- [x] Extraction column names fixed (`extraction_method`, `is_superseded`) to match migration 002 schema
- [x] Source URI populated in search results (`source_uri` field on `FusedResult` + `SearchResultResponse`)
- [x] Noisy year entities filtered during extraction (bare numbers/year ranges rejected)
- [x] Migration 002 dimension note added (documents 2048 dependency on `COVALENCE_EMBED_DIM`)

**Note:** 301+ tests passing, clippy clean, fmt clean.

## Future

- Federation protocol (clearance-based egress, ZK edges)
- Multi-tenant support
- Cross-encoder reranking (ColBERT via BGE-M3)
- Topology-derived embeddings (spectral, Node2Vec)
- Multi-lingual support (BGE-M3)
- SSE streaming for LLM synthesis
- Webhook notifications
- Batch ingestion API
