//! Route definitions — thin handlers that delegate to services.

use axum::Router;
use axum::routing::{delete, get, post, put};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use utoipa::OpenApi;

use crate::handlers::{
    adapters, admin, analysis, ask, config, edges, extensions, hooks, mcp, memory, metrics, nodes,
    ontology, search, sessions, sources,
};
use crate::middleware::require_api_key;
use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Maximum request body size (50 MiB).
const MAX_BODY_SIZE: usize = 50 * 1024 * 1024;

/// Build the application router with all routes.
pub fn router(state: AppState) -> Router {
    // Versioned API routes under /api/v1
    let api_v1 = Router::new()
        // Sources
        .route("/sources", post(sources::create_source))
        .route("/sources", get(sources::list_sources))
        .route("/sources/{id}", get(sources::get_source))
        .route("/sources/{id}", delete(sources::delete_source))
        .route("/sources/{id}/reprocess", post(sources::reprocess_source))
        .route(
            "/sources/{id}/queue-reprocess",
            post(sources::queue_reprocess_source),
        )
        .route("/sources/{id}/chunks", get(sources::get_source_chunks))
        // Search
        .route("/search", post(search::search))
        .route("/search/feedback", post(search::search_feedback))
        // Ask (LLM synthesis)
        .route("/ask", post(ask::ask))
        .route("/ask/stream", post(ask::ask_stream))
        // Sessions (conversation context)
        .route("/sessions", post(sessions::create_session))
        .route("/sessions", get(sessions::list_sessions))
        .route("/sessions/{id}", get(sessions::get_session))
        .route("/sessions/{id}", delete(sessions::close_session))
        .route("/sessions/{id}/turns", get(sessions::get_turns))
        .route("/sessions/{id}/turns", post(sessions::add_turn))
        // Nodes
        .route("/nodes/resolve", post(nodes::resolve_node))
        .route("/nodes/merge", post(nodes::merge_nodes))
        .route("/nodes/landmarks", get(nodes::list_landmarks))
        .route("/nodes/{id}", get(nodes::get_node))
        .route("/nodes/{id}/neighborhood", get(nodes::get_neighborhood))
        .route("/nodes/{id}/provenance", get(nodes::get_provenance))
        .route("/nodes/{id}/split", post(nodes::split_node))
        .route("/nodes/{id}/correct", post(nodes::correct_node))
        .route("/nodes/{id}/annotate", post(nodes::annotate_node))
        // Edges
        .route("/edges/{id}", get(edges::get_edge))
        .route("/edges/{id}", delete(edges::delete_edge))
        .route("/edges/{id}/correct", post(edges::correct_edge))
        // Graph
        .route("/graph/stats", get(admin::graph_stats))
        .route("/graph/communities", get(admin::graph_communities))
        .route("/graph/topology", get(admin::graph_topology))
        // Audit
        .route("/audit", get(admin::audit_log))
        // MCP
        .route("/mcp/tools/list", post(mcp::list_tools_handler))
        .route("/mcp/tools/call", post(mcp::call_tool))
        // Memory
        .nest("/memory", memory_routes())
        // Admin
        .route("/admin/graph/reload", post(admin::reload_graph))
        .route(
            "/admin/graph/invalidated-stats",
            get(admin::invalidated_edge_stats),
        )
        .route("/admin/publish/{source_id}", post(admin::publish_source))
        .route("/admin/consolidate", post(admin::trigger_consolidation))
        .route("/admin/gc", post(admin::garbage_collect))
        .route("/admin/raptor", post(admin::trigger_raptor))
        .route("/admin/ontology/cluster", post(admin::cluster_ontology))
        .route("/admin/health", get(admin::health))
        .route("/admin/metrics", get(admin::metrics))
        .route("/admin/traces", get(admin::list_traces))
        .route("/admin/traces/{id}/replay", post(admin::replay_trace))
        .route("/admin/cache/clear", post(admin::clear_cache))
        .route("/admin/knowledge-gaps", get(admin::knowledge_gaps))
        .route("/admin/config-audit", post(admin::config_audit))
        .route("/admin/tier5/resolve", post(admin::resolve_tier5))
        .route(
            "/admin/edges/synthesize",
            post(admin::synthesize_cooccurrence),
        )
        .route("/admin/nodes/cleanup", post(admin::cleanup_noise_entities))
        .route(
            "/admin/nodes/backfill-embeddings",
            post(admin::backfill_node_embeddings),
        )
        .route("/admin/opinions/seed", post(admin::seed_opinions))
        .route(
            "/admin/nodes/summarize-code",
            post(admin::summarize_code_nodes),
        )
        .route("/admin/edges/bridge", post(admin::bridge_code_to_concepts))
        // Services
        .route("/admin/services", get(admin::list_services))
        // Extensions
        .route("/admin/extensions", get(extensions::list_extensions))
        .route(
            "/admin/extensions/reload",
            post(extensions::reload_extensions),
        )
        // Config + Adapters
        .route("/admin/config", get(config::list_config))
        .route("/admin/config/{key}", put(config::update_config))
        .route("/admin/adapters", get(adapters::list_adapters))
        .route("/admin/ontology", get(ontology::get_ontology))
        // Hooks
        .route("/admin/hooks", post(hooks::create_hook))
        .route("/admin/hooks", get(hooks::list_hooks))
        .route("/admin/hooks/{id}", delete(hooks::delete_hook))
        // Queue
        .route("/admin/health-report", get(admin::health_report))
        .route("/admin/data-health", get(admin::data_health_handler))
        .route("/admin/queue/status", get(admin::queue_status_handler))
        .route("/admin/queue/retry", post(admin::retry_failed_handler))
        .route("/admin/queue/dead", get(admin::list_dead_handler))
        .route("/admin/queue/dead/clear", post(admin::clear_dead_handler))
        .route(
            "/admin/queue/dead/resurrect",
            post(admin::resurrect_dead_handler),
        )
        .route(
            "/admin/queue/summarize-all",
            post(admin::enqueue_summarize_all),
        )
        .route("/admin/queue/compose-all", post(admin::enqueue_compose_all))
        // Analysis
        .route("/analysis/bootstrap", post(analysis::bootstrap_components))
        .route("/analysis/link", post(analysis::link_domains))
        .route("/analysis/coverage", post(analysis::coverage_analysis))
        .route("/analysis/erosion", post(analysis::detect_erosion))
        .route("/analysis/blast-radius", post(analysis::blast_radius))
        .route("/analysis/whitespace", post(analysis::whitespace_roadmap))
        .route("/analysis/verify", post(analysis::verify_implementation))
        .route("/analysis/alignment", post(analysis::alignment_report))
        .route("/analysis/critique", post(analysis::critique));

    // Resolve the dashboard directory relative to the working
    // directory. The binary is typically run from the repo root,
    // so `dashboard/` is a sibling of `engine/`.
    let dashboard_dir =
        std::env::var("COVALENCE_DASHBOARD_DIR").unwrap_or_else(|_| "dashboard".to_string());

    Router::new()
        // Swagger UI + OpenAPI spec at root
        .merge(utoipa_swagger_ui::SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
        // Dashboard — static files (public, no auth)
        .nest_service("/dashboard", ServeDir::new(&dashboard_dir))
        // Root health check (convenience, no auth)
        .route("/health", get(admin::health))
        // Prometheus metrics (root level, no auth — standard scrape path)
        .route("/metrics", get(metrics::prometheus_metrics))
        // Versioned API
        .nest("/api/v1", api_v1)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_SIZE))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .with_state(state)
}

/// Memory API route group.
fn memory_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(memory::store_memory))
        .route("/recall", post(memory::recall_memory))
        .route("/status", get(memory::memory_status))
        .route("/consolidate", post(memory::consolidate_memory))
        .route("/reflect/{session_id}", post(memory::reflect_memory))
        .route("/forget-old", post(memory::apply_forgetting))
        .route("/{id}", delete(memory::forget_memory))
}
