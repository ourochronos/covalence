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
    search_service::*,
    contention_service::*,
    memory_service::*,
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
        .route("/search", post(search_handler))
        // Contentions
        .route("/contentions", get(contention_list))
        .route("/contentions/{id}", get(contention_get))
        .route("/contentions/{id}/resolve", post(contention_resolve))
        // Memory
        .route("/memory", post(memory_store))
        .route("/memory/search", post(memory_recall))
        .route("/memory/status", get(memory_status))
        .route("/memory/{id}/forget", patch(memory_forget))
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

// ── Search handler ──────────────────────────────────────────────

async fn search_handler(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let service = SearchService::new(state.pool.clone());
    service.init().await;
    let (results, meta) = service.search(req).await
        .map_err(|e| crate::errors::AppError::Internal(e))?;
    Ok(Json(serde_json::json!({
        "data": results,
        "meta": meta,
    })))
}

// ── Contention handlers ─────────────────────────────────────────

async fn contention_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = ContentionService::new(state.pool.clone());
    let article_id = params.get("node_id").and_then(|s| s.parse().ok());
    let status = params.get("status").cloned();
    let items = svc.list(article_id, status).await?;
    Ok(Json(serde_json::json!({"data": items})))
}

async fn contention_get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = ContentionService::new(state.pool.clone());
    match svc.get(id).await? {
        Some(c) => Ok(Json(serde_json::json!({"data": c}))),
        None => Err(crate::errors::AppError::NotFound("contention not found".into())),
    }
}

async fn contention_resolve(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ResolveRequest>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = ContentionService::new(state.pool.clone());
    let c = svc.resolve(id, req).await?;
    Ok(Json(serde_json::json!({"data": c})))
}

// ── Memory handlers ─────────────────────────────────────────────

async fn memory_store(
    State(state): State<AppState>,
    Json(req): Json<StoreMemoryRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), crate::errors::AppError> {
    let svc = MemoryService::new(state.pool.clone());
    let mem = svc.store(req).await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({"data": mem}))))
}

async fn memory_recall(
    State(state): State<AppState>,
    Json(req): Json<RecallRequest>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = MemoryService::new(state.pool.clone());
    let results = svc.recall(req).await?;
    Ok(Json(serde_json::json!({"data": results})))
}

async fn memory_status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = MemoryService::new(state.pool.clone());
    let stats = svc.status().await?;
    Ok(Json(serde_json::json!({"data": stats})))
}

async fn memory_forget(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, crate::errors::AppError> {
    let svc = MemoryService::new(state.pool.clone());
    let reason = body.get("reason").and_then(|v| v.as_str()).map(String::from);
    svc.forget(id, reason).await?;
    Ok(StatusCode::NO_CONTENT)
}
