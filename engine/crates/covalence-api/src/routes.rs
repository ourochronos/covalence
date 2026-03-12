//! Route definitions — thin handlers that delegate to services.

use axum::Router;
use axum::routing::{delete, get, post};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use utoipa::OpenApi;

use crate::handlers::{admin, edges, mcp, memory, nodes, search, sources};
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
        .route("/sources/{id}/chunks", get(sources::get_source_chunks))
        // Search
        .route("/search", post(search::search))
        .route("/search/feedback", post(search::search_feedback))
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
        .route("/admin/publish/{source_id}", post(admin::publish_source))
        .route("/admin/consolidate", post(admin::trigger_consolidation))
        .route("/admin/gc", post(admin::garbage_collect))
        .route("/admin/ontology/cluster", post(admin::cluster_ontology))
        .route("/admin/health", get(admin::health))
        .route("/admin/metrics", get(admin::metrics))
        .route("/admin/traces", get(admin::list_traces))
        .route("/admin/traces/{id}/replay", post(admin::replay_trace))
        .route("/admin/cache/clear", post(admin::clear_cache))
        .route("/admin/knowledge-gaps", get(admin::knowledge_gaps))
        .route("/admin/config-audit", post(admin::config_audit))
        .route(
            "/admin/edges/synthesize",
            post(admin::synthesize_cooccurrence),
        );

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
        // Versioned API
        .nest("/api/v1", api_v1)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .with_state(state)
}

/// Memory API route group.
fn memory_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(memory::store_memory))
        .route("/recall", post(memory::recall_memory))
        .route("/status", get(memory::memory_status))
        .route("/{id}", delete(memory::forget_memory))
}
