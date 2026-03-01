//! Axum route handlers — thin layer mapping HTTP → service calls.

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::services::{
    source_service::*,
    article_service::*,
    edge_service::*,
    admin_service::*,
};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // Sources
        .route("/sources", post(source_ingest))
        .route("/sources", get(source_list))
        .route("/sources/{id}", get(source_get))
        .route("/sources/{id}", delete(source_delete))
        // Articles
        .route("/articles", post(article_create))
        .route("/articles/compile", post(article_compile))
        .route("/articles/merge", post(article_merge))
        .route("/articles", get(article_list))
        .route("/articles/{id}", get(article_get))
        .route("/articles/{id}", patch(article_update))
        .route("/articles/{id}", delete(article_delete))
        .route("/articles/{id}/split", post(article_split))
        .route("/articles/{id}/provenance", get(article_provenance))
        // Edges
        .route("/edges", post(edge_create))
        .route("/edges/{id}", delete(edge_delete))
        // Nodes (shared)
        .route("/nodes/{id}/edges", get(node_edges))
        .route("/nodes/{id}/neighborhood", get(node_neighborhood))
        // Admin
        .route("/admin/stats", get(admin_stats))
        .route("/admin/maintenance", post(admin_maintenance))
}

// ── Source handlers ─────────────────────────────────────────────

async fn source_ingest(
    State(state): State<AppState>,
    Json(req): Json<IngestRequest>,
) -> impl IntoResponse {
    let svc = SourceService::new(state.pool);
    match svc.ingest(req).await {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn source_get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let svc = SourceService::new(state.pool);
    match svc.get(id).await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn source_list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let svc = SourceService::new(state.pool);
    match svc.list(params).await {
        Ok(resp) => Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn source_delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let svc = SourceService::new(state.pool);
    match svc.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

// ── Article handlers ────────────────────────────────────────────

async fn article_create(
    State(state): State<AppState>,
    Json(req): Json<CreateArticleRequest>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.create(req).await {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.get(id).await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct ArticleListQuery {
    limit: Option<i64>,
    cursor: Option<Uuid>,
    status: Option<String>,
}

async fn article_list(
    State(state): State<AppState>,
    Query(params): Query<ArticleListQuery>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.list(params.limit.unwrap_or(20), params.cursor, params.status.as_deref()).await {
        Ok(resp) => Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateArticleRequest>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.update(id, req).await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_split(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.split(id).await {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_merge(
    State(state): State<AppState>,
    Json(req): Json<MergeRequest>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.merge(req).await {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_compile(
    State(state): State<AppState>,
    Json(req): Json<CompileRequest>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.compile(req).await {
        Ok(resp) => (StatusCode::ACCEPTED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct ProvenanceQuery {
    max_depth: Option<u32>,
    claim: Option<String>,
}

async fn article_provenance(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<ProvenanceQuery>,
) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.provenance(id, params.max_depth).await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

// ── Edge handlers ───────────────────────────────────────────────

async fn edge_create(
    State(state): State<AppState>,
    Json(req): Json<CreateEdgeRequest>,
) -> impl IntoResponse {
    let svc = EdgeService::new(state.pool);
    match svc.create(req).await {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn edge_delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let svc = EdgeService::new(state.pool);
    match svc.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
struct EdgeListQuery {
    direction: Option<String>,
    labels: Option<String>,
    limit: Option<usize>,
}

async fn node_edges(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<EdgeListQuery>,
) -> impl IntoResponse {
    let svc = EdgeService::new(state.pool);
    match svc.list_for_node(id, params.direction.as_deref(), params.labels.as_deref(), params.limit.unwrap_or(50)).await {
        Ok(resp) => Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn node_neighborhood(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<NeighborhoodParams>,
) -> impl IntoResponse {
    let svc = EdgeService::new(state.pool);
    match svc.neighborhood(id, params).await {
        Ok(resp) => Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response(),
        Err(e) => e.into_response(),
    }
}

// ── Admin handlers ──────────────────────────────────────────────

async fn admin_stats(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let svc = AdminService::new(state.pool);
    match svc.stats().await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn admin_maintenance(
    State(state): State<AppState>,
    Json(req): Json<MaintenanceRequest>,
) -> impl IntoResponse {
    let svc = AdminService::new(state.pool);
    match svc.maintenance(req).await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}
