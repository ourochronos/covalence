//! OpenAPI spec generation and Swagger UI endpoint.
//!
//! Exposes:
//!   GET /openapi.json  — full OpenAPI 3.x spec as JSON
//!   GET /docs          — Swagger UI (CDN-loaded, no build-time downloads)

#![allow(dead_code)]

use axum::response::Html;
use utoipa::OpenApi;

use crate::models::SearchIntent;
use crate::services::{
    admin_service::{
        EdgeStats, EmbeddingStats, MaintenanceRequest, MaintenanceResponse, NodeStats, QueueStats,
        StatsResponse,
    },
    article_service::{ArticleResponse, CompileJobResponse, CompileRequest, MergeRequest},
    memory_service::{Memory, RecallRequest, StoreMemoryRequest},
    search_service::{SearchMode, SearchRequest, SearchResult, SearchStrategy, WeightsInput},
    source_service::{IngestRequest, SourceResponse},
};

// ─── Path annotations ─────────────────────────────────────────────────────────
// These stub functions exist solely to carry `#[utoipa::path]` metadata.

/// POST /sources
#[utoipa::path(
    post,
    path = "/sources",
    request_body = IngestRequest,
    responses(
        (status = 201, description = "Source ingested successfully", body = SourceResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "sources"
)]
pub fn ingest_source() {}

/// GET /sources/{id}
#[utoipa::path(
    get,
    path = "/sources/{id}",
    params(
        ("id" = String, Path, description = "Source UUID")
    ),
    responses(
        (status = 200, description = "Source found", body = SourceResponse),
        (status = 404, description = "Source not found"),
    ),
    tag = "sources"
)]
pub fn get_source() {}

/// DELETE /sources/{id}
#[utoipa::path(
    delete,
    path = "/sources/{id}",
    params(
        ("id" = String, Path, description = "Source UUID")
    ),
    responses(
        (status = 204, description = "Source deleted"),
        (status = 404, description = "Source not found"),
    ),
    tag = "sources"
)]
pub fn delete_source() {}

/// POST /articles/compile
#[utoipa::path(
    post,
    path = "/articles/compile",
    request_body = CompileRequest,
    responses(
        (status = 202, description = "Compilation job accepted", body = CompileJobResponse),
        (status = 400, description = "Bad request"),
    ),
    tag = "articles"
)]
pub fn compile_article() {}

/// GET /articles/{id}
#[utoipa::path(
    get,
    path = "/articles/{id}",
    params(
        ("id" = String, Path, description = "Article UUID")
    ),
    responses(
        (status = 200, description = "Article found", body = ArticleResponse),
        (status = 404, description = "Article not found"),
    ),
    tag = "articles"
)]
pub fn get_article() {}

/// POST /articles/merge
#[utoipa::path(
    post,
    path = "/articles/merge",
    request_body = MergeRequest,
    responses(
        (status = 201, description = "Articles merged", body = ArticleResponse),
        (status = 400, description = "Bad request"),
        (status = 404, description = "One or both articles not found"),
    ),
    tag = "articles"
)]
pub fn merge_articles() {}

/// POST /search
#[utoipa::path(
    post,
    path = "/search",
    request_body = SearchRequest,
    responses(
        (status = 200, description = "Search results", body = Vec<SearchResult>),
        (status = 400, description = "Bad request"),
    ),
    tag = "search"
)]
pub fn search() {}

/// POST /memory
#[utoipa::path(
    post,
    path = "/memory",
    request_body = StoreMemoryRequest,
    responses(
        (status = 201, description = "Memory stored", body = Memory),
        (status = 400, description = "Bad request"),
    ),
    tag = "memory"
)]
pub fn store_memory() {}

/// POST /memory/search
#[utoipa::path(
    post,
    path = "/memory/search",
    request_body = RecallRequest,
    responses(
        (status = 200, description = "Memory recall results", body = Vec<Memory>),
        (status = 400, description = "Bad request"),
    ),
    tag = "memory"
)]
pub fn recall_memory() {}

/// GET /admin/stats
#[utoipa::path(
    get,
    path = "/admin/stats",
    responses(
        (status = 200, description = "System statistics", body = StatsResponse),
    ),
    tag = "admin"
)]
pub fn admin_stats() {}

/// POST /admin/maintenance
#[utoipa::path(
    post,
    path = "/admin/maintenance",
    request_body = MaintenanceRequest,
    responses(
        (status = 200, description = "Maintenance completed", body = MaintenanceResponse),
    ),
    tag = "admin"
)]
pub fn admin_maintenance() {}

// ─── OpenAPI aggregate struct ─────────────────────────────────────────────────

#[derive(OpenApi)]
#[openapi(
    paths(
        ingest_source,
        get_source,
        delete_source,
        compile_article,
        get_article,
        merge_articles,
        search,
        store_memory,
        recall_memory,
        admin_stats,
        admin_maintenance,
    ),
    components(schemas(
        IngestRequest,
        SourceResponse,
        CompileRequest,
        CompileJobResponse,
        ArticleResponse,
        MergeRequest,
        SearchRequest,
        SearchResult,
        SearchMode,
        SearchStrategy,
        SearchIntent,
        WeightsInput,
        StoreMemoryRequest,
        RecallRequest,
        Memory,
        StatsResponse,
        NodeStats,
        EdgeStats,
        QueueStats,
        EmbeddingStats,
        MaintenanceRequest,
        MaintenanceResponse,
    )),
    tags(
        (name = "sources", description = "Source management — ingest, retrieve, and delete raw sources"),
        (name = "articles", description = "Article management — compile, merge, split, and retrieve compiled knowledge"),
        (name = "search", description = "Three-dimensional knowledge search (vector + lexical + graph)"),
        (name = "memory", description = "Memory operations — store and recall agent observations"),
        (name = "admin", description = "Admin operations — system stats and maintenance"),
    ),
    info(
        title = "Covalence Knowledge Engine",
        version = "0.1.0",
        description = "Epistemic knowledge management API — sources, articles, search, and memory"
    )
)]
pub struct ApiDoc;

// ─── Route handlers ───────────────────────────────────────────────────────────

/// GET /openapi.json — serve the full OpenAPI spec as JSON.
pub async fn openapi_json() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(ApiDoc::openapi())
}

/// GET /docs — serve the Swagger UI (CDN-loaded, no build-time downloads).
pub async fn swagger_ui() -> Html<&'static str> {
    Html(
        r##"<!DOCTYPE html>
<html>
<head>
  <title>Covalence API</title>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script>
    SwaggerUIBundle({
      url: "/openapi.json",
      dom_id: "#swagger-ui",
      presets: [SwaggerUIBundle.presets.apis, SwaggerUIBundle.SwaggerUIStandalonePreset],
      layout: "BaseLayout"
    })
  </script>
</body>
</html>"##,
    )
}
