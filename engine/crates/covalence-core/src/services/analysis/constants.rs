//! Well-known component definitions and module path mappings.

/// Well-known logical components of the Covalence system.
///
/// Each entry: (canonical_name, description used for embedding + intent matching).
pub(super) const COMPONENT_DEFS: &[(&str, &str)] = &[
    (
        "Ingestion Pipeline",
        "Source ingestion pipeline: file reading, format conversion \
         (HTML/Markdown/plain text), overlapping-window chunking with \
         contextual embeddings (Voyage voyage-context-3), LLM entity \
         extraction with noise filtering, domain classification \
         (code/spec/design/research/external), and persistent retry \
         queue for failed ingestion jobs.",
    ),
    (
        "Statement Pipeline",
        "Statement-first extraction pipeline: windowed LLM statement \
         extraction via Claude Haiku with Gemini/Copilot fallback, \
         fastcoref coreference resolution, offset projection ledger \
         for byte-accurate provenance, HAC clustering into sections, \
         source summary compilation, and entity extraction from \
         statements with source-domain-aware entity classification.",
    ),
    (
        "Search Fusion",
        "Multi-dimensional fused search: vector, lexical, temporal, \
         graph, structural, and global community dimensions combined \
         via convex combination fusion with coverage multiplier. \
         SkewRoute adaptive strategy selection via Gini coefficient. \
         Self-referential domain boost (DDSS) for internal vs external \
         content. Voyage rerank-2.5 reranking blended 60/40 with \
         fusion scores. Semantic query cache. Abstention detection. \
         Post-fusion entity_class and source_layer filtering.",
    ),
    (
        "Entity Resolution",
        "Five-tier entity resolution cascade: exact canonical name \
         match, alias lookup (case-insensitive), vector cosine \
         similarity (Voyage embeddings, threshold 0.85), fuzzy \
         trigram matching (threshold 0.55), and HDBSCAN batch \
         clustering for deferred Tier 5 resolution. Graph context \
         disambiguation rejects type mismatches with no neighborhood \
         overlap. Source-domain-aware entity_class derivation.",
    ),
    (
        "Graph Sidecar",
        "Graph engine abstraction (GraphEngine trait) supporting both \
         in-memory petgraph StableDiGraph and Apache AGE PostgreSQL \
         backends. Node/edge index with entity_class metadata. \
         PageRank, TrustRank, k-core community detection, \
         neighborhood traversal, topology analysis. Outbox-based \
         sync with LISTEN/NOTIFY. Cross-domain analysis: coverage, \
         erosion detection, blast radius, whitespace roadmap, \
         dialectical critique.",
    ),
    (
        "Epistemic Model",
        "Subjective Logic epistemic framework: opinion tuples (b,d,u,a), \
         cumulative and average fusion, discount and deduction operators. \
         Dempster-Shafer evidence fusion. DF-QuAD argumentation for \
         CONTRADICTS/CONTENDS edges. Bayesian Model Reduction for \
         principled forgetting. Confidence propagation with damped \
         fixed-point convergence. Epistemic confidence boost wired \
         into search ranking.",
    ),
    (
        "Consolidation",
        "Knowledge consolidation: batch consolidation for merging \
         duplicate nodes via ontology clustering, deep consolidation \
         for ontology refinement and community summary generation, \
         RAPTOR recursive summarization producing articles. Contention \
         detection and resolution via CONTRADICTS edges. HDBSCAN-based \
         entity clustering for Tier 5 resolution.",
    ),
    (
        "API Layer",
        "Axum HTTP API server: RESTful endpoints under /api/v1 for \
         sources, nodes, edges, articles, search, ask (LLM synthesis), \
         memory, admin, and cross-domain analysis. OpenAPI spec via \
         utoipa with Swagger UI. MCP tool interface (11 tools). \
         Entity_class and source_domain filtering in search. \
         ChainChatBackend for multi-provider LLM failover.",
    ),
    (
        "CLI",
        "Go CLI tool (cove): Cobra subcommands for source management \
         (add, add-url, reprocess, list, delete), search (with strategy, \
         granularity, entity_class filters), node inspection, graph \
         analysis, admin operations, memory (store/recall/forget), \
         ask (grounded Q&A with citations), and llm (multi-provider \
         LLM prompts: haiku/sonnet/opus/gemini/copilot).",
    ),
];

/// Module path segments mapped to component names for PART_OF_COMPONENT linking.
///
/// Code nodes whose source URI contains one of these path segments
/// are linked to the corresponding Component.
pub(super) const MODULE_PATH_MAPPINGS: &[(&str, &str)] = &[
    // Ingestion Pipeline — specific submodules first, then catch-all
    ("src/ingestion/pipeline", "Ingestion Pipeline"),
    ("src/ingestion/embedder", "Ingestion Pipeline"),
    ("src/ingestion/converter", "Ingestion Pipeline"),
    ("src/ingestion/chunker", "Ingestion Pipeline"),
    ("src/ingestion/code_chunker", "Ingestion Pipeline"),
    ("src/ingestion/normalize", "Ingestion Pipeline"),
    ("src/ingestion/source_profile", "Ingestion Pipeline"),
    ("src/ingestion/extractor", "Ingestion Pipeline"),
    ("src/ingestion/llm_extractor", "Ingestion Pipeline"),
    ("src/ingestion/fingerprint", "Ingestion Pipeline"),
    ("src/ingestion/ast_extractor", "Ingestion Pipeline"),
    ("src/ingestion/sidecar_extractor", "Ingestion Pipeline"),
    ("src/ingestion/gliner_extractor", "Ingestion Pipeline"),
    ("src/ingestion/openai_embedder", "Ingestion Pipeline"),
    ("src/ingestion/voyage", "Ingestion Pipeline"),
    ("src/ingestion/landscape", "Ingestion Pipeline"),
    ("src/ingestion/projection", "Ingestion Pipeline"),
    ("src/ingestion/parser", "Ingestion Pipeline"),
    ("src/ingestion/accept", "Ingestion Pipeline"),
    ("src/ingestion/pii", "Ingestion Pipeline"),
    ("src/ingestion/takedown", "Ingestion Pipeline"),
    ("ingestion_helpers", "Ingestion Pipeline"),
    ("chunk_quality", "Ingestion Pipeline"),
    // Statement Pipeline
    ("src/ingestion/statement", "Statement Pipeline"),
    ("src/ingestion/chat_backend", "Statement Pipeline"),
    ("src/ingestion/section", "Statement Pipeline"),
    ("src/services/statement_pipeline", "Statement Pipeline"),
    // Search Fusion
    ("src/search", "Search Fusion"),
    ("src/services/search", "Search Fusion"),
    // Entity Resolution
    ("src/ingestion/resolver", "Entity Resolution"),
    // Graph Sidecar
    ("src/graph", "Graph Sidecar"),
    // Epistemic Model
    ("src/epistemic", "Epistemic Model"),
    ("types/opinion", "Epistemic Model"),
    // Consolidation
    ("src/consolidation", "Consolidation"),
    ("src/services/consolidation", "Consolidation"),
    ("src/services/article", "Consolidation"),
    // API Layer
    ("src/routes", "API Layer"),
    ("src/handlers", "API Layer"),
    ("src/middleware", "API Layer"),
    ("src/state", "API Layer"),
    ("src/openapi", "API Layer"),
    ("covalence-api", "API Layer"),
    // CLI
    ("cmd/", "CLI"),
    ("internal/", "CLI"),
    // Services (general)
    ("src/services/admin", "API Layer"),
    ("src/services/ask", "API Layer"),
    ("src/services/source", "Ingestion Pipeline"),
    ("src/services/pipeline", "Ingestion Pipeline"),
    ("src/services/node", "Graph Sidecar"),
    ("src/services/edge", "Graph Sidecar"),
    ("src/services/noise_filter", "Ingestion Pipeline"),
    ("src/services/analysis", "Graph Sidecar"),
    ("src/services/memory", "API Layer"),
    ("search_helpers", "Search Fusion"),
    // Core domain models & types — part of Graph Sidecar (data model)
    ("src/models/", "Graph Sidecar"),
    ("src/types/", "Graph Sidecar"),
    ("src/error", "Graph Sidecar"),
    ("src/config", "API Layer"),
    // Storage layer
    ("src/storage/", "Graph Sidecar"),
    // Eval harness
    ("covalence-eval", "Ingestion Pipeline"),
    // Migrations
    ("covalence-migrations", "API Layer"),
];
