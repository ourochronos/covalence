//! Well-known component definitions and module path mappings.

/// Well-known logical components of the Covalence system.
///
/// Each entry: (canonical_name, description used for embedding + intent matching).
pub(super) const COMPONENT_DEFS: &[(&str, &str)] = &[
    (
        "Ingestion Pipeline",
        "Source ingestion pipeline: file reading, HTML/PDF conversion, \
         chunking, embedding generation, entity extraction, and graph \
         construction from unstructured documents.",
    ),
    (
        "Statement Pipeline",
        "Statement-first extraction: windowed LLM statement extraction, \
         coreference resolution, offset projection, HAC clustering into \
         sections, source summary compilation.",
    ),
    (
        "Search Fusion",
        "Multi-dimensional fused search: vector, lexical, temporal, graph, \
         structural, and global search dimensions combined via convex \
         combination fusion with coverage multiplier and reranking.",
    ),
    (
        "Entity Resolution",
        "Five-tier entity resolution cascade: exact match, alias lookup, \
         vector cosine similarity, fuzzy trigram matching, and HDBSCAN \
         batch clustering for entity deduplication.",
    ),
    (
        "Graph Sidecar",
        "In-memory petgraph directed graph sidecar: node/edge index, \
         PageRank, TrustRank, community detection via label propagation, \
         neighborhood traversal, and topology analysis.",
    ),
    (
        "Epistemic Model",
        "Subjective Logic epistemic framework: opinion tuples (b,d,u,a), \
         Dempster-Shafer fusion, DF-QuAD argumentation, Bayesian memory \
         retention decay, confidence propagation and convergence.",
    ),
    (
        "Consolidation",
        "Knowledge consolidation: batch consolidation for merging duplicate \
         nodes, deep consolidation for ontology refinement, RAPTOR \
         recursive summarization, graph-batch consolidation.",
    ),
    (
        "API Layer",
        "Axum HTTP API server: RESTful endpoints under /api/v1, OpenAPI \
         spec via utoipa, Swagger UI, MCP tool interface, request routing \
         and middleware.",
    ),
    (
        "CLI",
        "Go CLI tool (cove): Cobra subcommands for source management, \
         search, node inspection, graph analysis, admin operations, \
         memory, and copilot integration.",
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
    ("src/services/source", "Ingestion Pipeline"),
    ("src/services/node", "Graph Sidecar"),
    ("src/services/edge", "Graph Sidecar"),
    ("src/services/noise_filter", "Ingestion Pipeline"),
    ("src/services/analysis", "Graph Sidecar"),
    ("src/services/memory", "API Layer"),
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
