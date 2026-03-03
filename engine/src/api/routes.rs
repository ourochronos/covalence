//! Axum route handlers — thin layer mapping HTTP → service calls.

use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, patch, post},
};

use super::openapi::{openapi_json, swagger_ui};
use serde::Deserialize;
use uuid::Uuid;

use crate::services::audit_service::AuditService;
use crate::services::concerns_service::{ConcernsService, UpsertConcernRequest};
use crate::services::provenance_trace_service::{ProvenanceTraceService, TraceRequest};
use crate::services::{
    admin_service::*, article_service::*, contention_service::*, edge_service::*,
    memory_service::*, search_service::*, session_service::*, source_service::*,
};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // OpenAPI / docs
        .route("/openapi.json", get(openapi_json))
        .route("/docs", get(swagger_ui))
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
        .route("/articles/{id}/trace", post(article_provenance_trace))
        // Edges
        .route("/edges", post(edge_create))
        .route("/edges/{id}", delete(edge_delete))
        // Nodes (shared)
        .route("/nodes/{id}/edges", get(node_edges))
        .route("/nodes/{id}/neighborhood", get(node_neighborhood))
        .route("/search", post(search_handler))
        .route("/search/debug", post(search_debug_handler))
        // Contentions
        .route("/contentions", get(contention_list))
        .route("/contentions/{id}", get(contention_get))
        .route("/contentions/{id}/resolve", post(contention_resolve))
        // Memory
        .route("/memory", post(memory_store))
        .route("/memory/search", post(memory_recall))
        .route("/memory/status", get(memory_status))
        .route("/memory/{id}/forget", patch(memory_forget))
        // Sessions — static sub-routes MUST come before the /{id} catch-all
        .route("/sessions/flush-stale", post(session_flush_stale))
        .route("/sessions", post(session_create))
        .route("/sessions", get(session_list))
        .route("/sessions/{id}", get(session_get))
        .route("/sessions/{id}/close", post(session_close))
        .route("/sessions/{id}/messages", post(session_append_messages))
        .route("/sessions/{id}/messages", get(session_get_messages))
        .route("/sessions/{id}/flush", post(session_flush))
        .route("/sessions/{id}/finalize", post(session_finalize))
        // Dashboard
        .route("/dashboard", get(dashboard_handler))
        // Admin
        .route("/admin/queue", get(admin_queue_list))
        .route("/admin/queue/{id}", get(admin_queue_get))
        .route("/admin/queue/{id}/retry", post(admin_queue_retry))
        .route("/admin/queue/{id}", delete(admin_queue_delete))
        .route("/admin/stats", get(admin_stats))
        .route("/admin/maintenance", post(admin_maintenance))
        .route("/admin/sync-edges", get(admin_sync_edges))
        .route("/admin/embed-all", post(admin_embed_all))
        .route("/admin/tree-index-all", post(admin_tree_index_all))
        .route("/admin/staleness-scan", post(admin_staleness_scan))
        .route(
            "/admin/concerns",
            post(admin_upsert_concerns).get(admin_list_concerns),
        )
        .route("/admin/graph/stats", get(admin_graph_stats))
        .route("/admin/graph/pagerank", get(admin_graph_pagerank))
        .route("/admin/graph/centrality", get(admin_graph_centrality))
        .route("/admin/graph/intent-stats", get(admin_graph_intent_stats))
        .route("/admin/knowledge/audit", get(admin_knowledge_audit))
        // Divergence detection (covalence#58)
        .route("/admin/divergence/scan", get(admin_divergence_scan))
        .route("/admin/divergence/report", get(admin_divergence_report))
}

// ── Graph reload helper ─────────────────────────────────────────

/// Rebuild the shared in-memory graph from the current DB state.
/// Called after any mutation to edges (create / delete).
async fn reload_shared_graph(state: &AppState) {
    use sqlx::Row as _;
    let rows = sqlx::query("SELECT source_node_id, target_node_id, edge_type FROM covalence.edges")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let mut new_graph = crate::graph::CovalenceGraph::new();
    for row in rows {
        let source: Uuid = row.try_get("source_node_id").unwrap_or_default();
        let target: Uuid = row.try_get("target_node_id").unwrap_or_default();
        let edge_type: String = row.try_get("edge_type").unwrap_or_default();
        new_graph.add_edge(source, target, edge_type);
    }

    let mut g = state.graph.write().await;
    *g = new_graph;
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

async fn source_get(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
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
        Ok(resp) => {
            Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn source_delete(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
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

async fn article_get(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
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
    match svc
        .list(
            params.limit.unwrap_or(20),
            params.cursor,
            params.status.as_deref(),
        )
        .await
    {
        Ok(resp) => {
            Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response()
        }
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

async fn article_delete(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    let svc = ArticleService::new(state.pool);
    match svc.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn article_split(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
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
        Ok(resp) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"data": resp})),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
#[allow(dead_code)]
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

// ── Provenance trace handler ─────────────────────────────────────────────────

async fn article_provenance_trace(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<TraceRequest>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = ProvenanceTraceService::new(state.pool.clone());
    let results = svc.trace(id, req).await?;
    Ok(Json(serde_json::json!({
        "data": results,
        "meta": {"count": results.len()}
    })))
}

// ── Edge handlers ───────────────────────────────────────────────

async fn edge_create(
    State(state): State<AppState>,
    Json(req): Json<CreateEdgeRequest>,
) -> impl IntoResponse {
    let svc = EdgeService::new(state.pool.clone());
    match svc.create(req).await {
        Ok(resp) => {
            reload_shared_graph(&state).await;
            (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn edge_delete(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    let svc = EdgeService::new(state.pool.clone());
    match svc.delete(id).await {
        Ok(()) => {
            reload_shared_graph(&state).await;
            StatusCode::NO_CONTENT.into_response()
        }
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
    match svc
        .list_for_node(
            id,
            params.direction.as_deref(),
            params.labels.as_deref(),
            params.limit.unwrap_or(50),
        )
        .await
    {
        Ok(resp) => {
            Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response()
        }
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
        Ok(resp) => {
            Json(serde_json::json!({"data": resp, "meta": {"count": resp.len()}})).into_response()
        }
        Err(e) => e.into_response(),
    }
}

// ── Admin handlers ──────────────────────────────────────────────

async fn admin_graph_stats(State(state): State<AppState>) -> impl IntoResponse {
    let g = state.graph.read().await;
    Json(serde_json::json!({
        "data": {
            "node_count": g.node_count(),
            "edge_count": g.edge_count(),
        }
    }))
}

async fn admin_graph_pagerank(State(state): State<AppState>) -> impl IntoResponse {
    let g = state.graph.read().await;
    let scores = crate::graph::pagerank(&g, 0.85, 20);
    let mut entries: Vec<_> = scores.into_iter().collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    entries.truncate(50);
    let result: serde_json::Value = serde_json::json!({
        "data": entries.into_iter().map(|(id, score)| serde_json::json!({ "node_id": id, "score": score })).collect::<Vec<_>>()
    });
    Json(result)
}

async fn admin_graph_centrality(State(state): State<AppState>) -> impl IntoResponse {
    let g = state.graph.read().await;
    let scores = crate::graph::betweenness_centrality(&g);
    let mut entries: Vec<_> = scores.into_iter().collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    entries.truncate(50);
    let result: serde_json::Value = serde_json::json!({
        "data": entries.into_iter().map(|(id, score)| serde_json::json!({ "node_id": id, "score": score })).collect::<Vec<_>>()
    });
    Json(result)
}

/// `GET /admin/graph/intent-stats` — edge count breakdown by intent category.
///
/// Returns the number of edges that fall into each of the four MAGMA-inspired
/// intent categories (factual, temporal, causal, entity) plus an `other_edges`
/// count for edge types that don't map to any intent.  Useful for diagnosing
/// graph coverage and understanding which retrieval intents are well-supported
/// by the current knowledge graph (Phase 7, covalence#54).
async fn admin_graph_intent_stats(State(state): State<AppState>) -> impl IntoResponse {
    let svc = AdminService::new(state.pool);
    match svc.graph_intent_stats().await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn admin_stats(State(state): State<AppState>) -> impl IntoResponse {
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
    let svc = AdminService::new(state.pool).with_graph(state.graph.clone());
    match svc.maintenance(req).await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn admin_sync_edges(State(state): State<AppState>) -> impl IntoResponse {
    let svc = AdminService::new(state.pool);
    match svc.sync_edges().await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

// ── Search handler ──────────────────────────────────────────────

async fn search_handler(
    State(state): State<AppState>,
    Json(mut req): Json<SearchRequest>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    // Auto-embed query text if no embedding provided
    if req.embedding.is_none() && !req.query.is_empty() {
        match state.llm.embed(&req.query).await {
            Ok(vec) => {
                // Only use if not all zeros (stub check)
                if vec.iter().any(|v| *v != 0.0) {
                    req.embedding = Some(vec);
                }
            }
            Err(e) => tracing::warn!("failed to embed search query: {e:#}"),
        }
    }
    let service = SearchService::new(state.pool.clone()).with_graph(state.graph.clone());
    service.init().await;
    let (results, meta) = service
        .search(req)
        .await
        .map_err(crate::errors::AppError::Internal)?;
    Ok(Json(serde_json::json!({
        "data": results,
        "meta": meta,
    })))
}

// ── Search debug handler ────────────────────────────────────────

/// `POST /search/debug` — same request body as `/search`, but returns a
/// verbose breakdown of per-dimension raw scores before fusion.  Useful for
/// eval harnesses, weight tuning, and debugging relevance issues.
async fn search_debug_handler(
    State(state): State<AppState>,
    Json(mut req): Json<SearchRequest>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    // Auto-embed query text if no embedding provided (same as /search).
    if req.embedding.is_none() && !req.query.is_empty() {
        match state.llm.embed(&req.query).await {
            Ok(vec) => {
                if vec.iter().any(|v| *v != 0.0) {
                    req.embedding = Some(vec);
                }
            }
            Err(e) => tracing::warn!("failed to embed search/debug query: {e:#}"),
        }
    }
    let service = SearchService::new(state.pool.clone()).with_graph(state.graph.clone());
    service.init().await;
    let debug = service
        .search_debug(req)
        .await
        .map_err(crate::errors::AppError::Internal)?;
    Ok(Json(serde_json::json!({ "data": debug })))
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
        None => Err(crate::errors::AppError::NotFound(
            "contention not found".into(),
        )),
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
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .map(String::from);
    svc.forget(id, reason).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Admin queue handlers ────────────────────────────────────────

async fn admin_queue_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = AdminService::new(state.pool.clone());
    let status = params.get("status").map(|s| s.as_str());
    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50i64);
    let entries = svc.list_queue(status, limit).await?;
    Ok(Json(serde_json::json!({"data": entries})))
}

async fn admin_queue_get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = AdminService::new(state.pool.clone());
    match svc.get_queue_entry(id).await? {
        Some(e) => Ok(Json(serde_json::json!({"data": e}))),
        None => Err(crate::errors::AppError::NotFound(
            "queue entry not found".into(),
        )),
    }
}

async fn admin_queue_retry(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = AdminService::new(state.pool.clone());
    let entry = svc.retry_queue_entry(id).await?;
    Ok(Json(serde_json::json!({"data": entry})))
}

async fn admin_queue_delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, crate::errors::AppError> {
    let svc = AdminService::new(state.pool.clone());
    let deleted = svc.delete_queue_entry(id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(crate::errors::AppError::NotFound(
            "queue entry not found".into(),
        ))
    }
}

// ── Session handlers ────────────────────────────────────────────

async fn session_create(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    let session = svc.create(req).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"data": session})),
    ))
}

async fn session_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    let status = params.get("status").map(|s| s.as_str());
    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50i64);
    let sessions = svc.list(status, limit).await?;
    Ok(Json(serde_json::json!({"data": sessions})))
}

async fn session_get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    match svc.get(id).await? {
        Some(s) => Ok(Json(serde_json::json!({"data": s}))),
        None => Err(crate::errors::AppError::NotFound(
            "session not found".into(),
        )),
    }
}

async fn session_close(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    svc.close(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn session_append_messages(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AppendMessagesRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    let messages = svc.append_messages(id, req).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"data": messages})),
    ))
}

#[derive(Deserialize)]
struct GetMessagesQuery {
    include_flushed: Option<bool>,
}

async fn session_get_messages(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<GetMessagesQuery>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    let include_flushed = params.include_flushed.unwrap_or(true);
    let messages = svc.get_messages(id, include_flushed).await?;
    Ok(Json(
        serde_json::json!({"data": messages, "meta": {"count": messages.len()}}),
    ))
}

async fn session_flush(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    let source_svc = SourceService::new(state.pool.clone());
    let result = svc.flush(id, &source_svc).await?;
    Ok(Json(serde_json::json!({"data": result})))
}

async fn session_finalize(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<FinalizeRequest>,
) -> Result<StatusCode, crate::errors::AppError> {
    let svc = SessionService::new(state.pool.clone());
    let source_svc = SourceService::new(state.pool.clone());
    svc.finalize(id, req.compile.unwrap_or(false), &source_svc)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FlushStaleBody {
    threshold_minutes: Option<i64>,
}

async fn session_flush_stale(
    State(state): State<AppState>,
    body: Option<Json<FlushStaleBody>>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let threshold_minutes = body.and_then(|b| b.threshold_minutes).unwrap_or(60);
    let svc = SessionService::new(state.pool.clone());
    let source_svc = SourceService::new(state.pool.clone());
    let results = svc.flush_stale(threshold_minutes, &source_svc).await?;
    Ok(Json(
        serde_json::json!({"data": results, "meta": {"count": results.len()}}),
    ))
}

async fn admin_embed_all(State(state): State<AppState>) -> impl IntoResponse {
    let svc = AdminService::new(state.pool);
    match svc.queue_embed_all().await {
        Ok(queued) => Json(serde_json::json!({"queued": queued})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn admin_staleness_scan(State(state): State<AppState>) -> impl IntoResponse {
    let svc = AdminService::new(state.pool);
    match svc.staleness_scan().await {
        Ok(resp) => Json(serde_json::json!({"data": resp})).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn admin_tree_index_all(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let overlap = body.get("overlap").and_then(|v| v.as_f64()).unwrap_or(0.20);
    let force = body.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let min_chars = body
        .get("min_chars")
        .and_then(|v| v.as_i64())
        .unwrap_or(700) as i32;

    // Find nodes without tree_index that have content above threshold
    let query = if force {
        format!(
            "SELECT id FROM covalence.nodes WHERE status = 'active' AND content IS NOT NULL AND LENGTH(content) >= {}",
            min_chars
        )
    } else {
        format!(
            "SELECT id FROM covalence.nodes WHERE status = 'active' AND content IS NOT NULL AND LENGTH(content) >= {} AND (metadata->>'tree_index' IS NULL OR metadata->>'tree_indexed_at' IS NULL)",
            min_chars
        )
    };

    let rows = match sqlx::query_as::<_, (uuid::Uuid,)>(&query)
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})).into_response(),
    };

    let mut queued = 0i64;
    for (node_id,) in &rows {
        let payload = serde_json::json!({ "overlap": overlap, "force": force });
        let result = sqlx::query(
            "INSERT INTO covalence.slow_path_queue (task_type, node_id, payload, priority)
             VALUES ('tree_index', $1, $2, 5)",
        )
        .bind(node_id)
        .bind(&payload)
        .execute(&state.pool)
        .await;

        if result.is_ok() {
            // Also queue tree_embed to run after
            let _ = sqlx::query(
                "INSERT INTO covalence.slow_path_queue (task_type, node_id, payload, priority)
                 VALUES ('tree_embed', $1, '{}'::jsonb, 4)",
            )
            .bind(node_id)
            .execute(&state.pool)
            .await;
            queued += 1;
        }
    }

    Json(serde_json::json!({
        "queued": queued,
        "overlap": overlap,
        "force": force,
        "min_chars": min_chars,
    }))
    .into_response()
}

// ── Standing concerns handlers ──────────────────────────────────

async fn admin_upsert_concerns(
    State(state): State<AppState>,
    Json(concerns): Json<Vec<UpsertConcernRequest>>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = ConcernsService::new(state.pool);
    let results = svc.upsert_many(concerns).await?;
    Ok(Json(serde_json::json!({
        "data": results,
        "meta": { "count": results.len() }
    })))
}

async fn admin_list_concerns(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let svc = ConcernsService::new(state.pool);
    let results = svc.list().await?;
    Ok(Json(serde_json::json!({
        "data": results,
        "meta": { "count": results.len() }
    })))
}

// ── Divergence detection handlers (covalence#58) ───────────────

/// `GET /admin/divergence/scan`
///
/// Triggers a live cross-dimensional divergence scan over all active nodes
/// that have both content embeddings (`node_embeddings`) and graph embeddings
/// (`graph_embeddings`).  Results are persisted to node metadata and returned
/// immediately.
///
/// Optional query parameter: `threshold` (float, default 0.5).
#[derive(serde::Deserialize)]
struct DivergenceScanQuery {
    threshold: Option<f64>,
}

async fn admin_divergence_scan(
    State(state): State<AppState>,
    Query(params): Query<DivergenceScanQuery>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let threshold = params
        .threshold
        .unwrap_or(crate::worker::divergence::DEFAULT_DIVERGENCE_THRESHOLD)
        .clamp(0.0, 1.0);

    let result = crate::worker::divergence::run_divergence_scan(&state.pool, threshold)
        .await
        .map_err(crate::errors::AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "data": {
            "scanned":   result.scanned,
            "flagged":   result.flagged,
            "anomalies": result.anomalies,
        },
        "meta": {
            "threshold": threshold,
        }
    })))
}

/// `GET /admin/divergence/report`
///
/// Returns the most recent divergence scan results stored in node metadata,
/// without triggering a new scan.  Only nodes flagged in a previous scan are
/// returned.
async fn admin_divergence_report(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    let result = crate::worker::divergence::read_divergence_report(&state.pool)
        .await
        .map_err(crate::errors::AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "data": {
            "scanned":   result.scanned,
            "flagged":   result.flagged,
            "anomalies": result.anomalies,
        }
    })))
}

// ── Knowledge audit handler ─────────────────────────────────────

#[derive(Deserialize)]
struct AuditQuery {
    q: String,
}

/// `GET /admin/knowledge/audit?q=<topic>`
///
/// Returns a structured epistemic assessment for the given topic, including:
/// consensus articles ranked by confidence, active contentions with both sides,
/// a provenance source summary, confidence distribution statistics, and graph
/// topology metrics.
async fn admin_knowledge_audit(
    State(state): State<AppState>,
    Query(params): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, crate::errors::AppError> {
    if params.q.trim().is_empty() {
        return Err(crate::errors::AppError::BadRequest(
            "query parameter `q` must not be empty".into(),
        ));
    }
    let svc = AuditService::new(state.pool.clone(), state.graph.clone());
    let report = svc.audit(&params.q).await?;
    Ok(Json(serde_json::json!({ "data": report })))
}

// ── Dashboard handler ───────────────────────────────────────────

/// Serve the self-contained browser dashboard at GET /dashboard.
async fn dashboard_handler() -> impl IntoResponse {
    Html(DASHBOARD_HTML)
}

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1.0"/>
<title>Covalence Dashboard</title>
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
  :root {
    --bg:        #1a1a2e;
    --surface:   #16213e;
    --card:      #0f3460;
    --accent:    #e94560;
    --accent2:   #533483;
    --text:      #e0e0e0;
    --muted:     #8888aa;
    --success:   #4caf50;
    --warning:   #ff9800;
    --radius:    8px;
    --gap:       16px;
  }
  body {
    background: var(--bg);
    color: var(--text);
    font-family: 'Segoe UI', system-ui, sans-serif;
    font-size: 14px;
    line-height: 1.6;
    min-height: 100vh;
  }
  header {
    background: var(--surface);
    border-bottom: 2px solid var(--accent);
    padding: 14px 24px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  header h1 { font-size: 1.25rem; font-weight: 700; letter-spacing: 0.5px; }
  header span.version { color: var(--muted); font-size: 0.8rem; }
  main { max-width: 1100px; margin: 0 auto; padding: 24px var(--gap); display: flex; flex-direction: column; gap: 28px; }
  section { background: var(--surface); border-radius: var(--radius); padding: 20px; border: 1px solid #253260; }
  section h2 { font-size: 1rem; font-weight: 600; color: var(--accent); margin-bottom: 14px; text-transform: uppercase; letter-spacing: 0.8px; }
  /* Stats grid */
  .stats-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: var(--gap); }
  .stat-card { background: var(--card); border-radius: var(--radius); padding: 14px; text-align: center; }
  .stat-card .value { font-size: 2rem; font-weight: 700; color: var(--accent); }
  .stat-card .label { font-size: 0.75rem; color: var(--muted); margin-top: 4px; text-transform: uppercase; letter-spacing: 0.5px; }
  .stat-card.warn .value { color: var(--warning); }
  /* Buttons */
  button {
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: var(--radius);
    padding: 8px 18px;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s;
  }
  button:hover { opacity: 0.85; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button.secondary { background: var(--accent2); }
  /* Search */
  .search-row { display: flex; gap: 8px; margin-bottom: 16px; }
  .search-row input {
    flex: 1;
    background: var(--card);
    color: var(--text);
    border: 1px solid #334080;
    border-radius: var(--radius);
    padding: 8px 12px;
    font-size: 0.9rem;
  }
  .search-row input:focus { outline: none; border-color: var(--accent); }
  /* Result cards */
  .results { display: flex; flex-direction: column; gap: 10px; }
  .result-card { background: var(--card); border-radius: var(--radius); padding: 12px 14px; border-left: 3px solid var(--accent2); }
  .result-card.article { border-left-color: var(--accent); }
  .result-card .rc-header { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 6px; }
  .result-card .rc-title { font-weight: 600; font-size: 0.95rem; }
  .result-card .rc-meta { font-size: 0.75rem; color: var(--muted); white-space: nowrap; margin-left: 8px; }
  .result-card .rc-preview { font-size: 0.85rem; color: var(--muted); }
  .badge { display: inline-block; padding: 1px 7px; border-radius: 4px; font-size: 0.7rem; font-weight: 700; text-transform: uppercase; }
  .badge.article { background: var(--accent); color: #fff; }
  .badge.source  { background: var(--accent2); color: #fff; }
  /* Article browser */
  .article-list { display: flex; flex-direction: column; gap: 8px; max-height: 320px; overflow-y: auto; }
  .article-item { background: var(--card); border-radius: var(--radius); padding: 10px 14px; cursor: pointer; transition: background 0.12s; display: flex; justify-content: space-between; align-items: center; }
  .article-item:hover { background: #1a3a5c; }
  .article-item .ai-title { font-weight: 600; font-size: 0.9rem; }
  .article-item .ai-meta  { font-size: 0.75rem; color: var(--muted); text-align: right; min-width: 110px; }
  /* Detail pane */
  #article-detail { display: none; margin-top: 16px; background: var(--card); border-radius: var(--radius); padding: 16px; }
  #article-detail.visible { display: block; }
  #article-detail h3 { font-size: 1rem; font-weight: 700; margin-bottom: 10px; color: var(--accent); }
  #article-detail .detail-meta { font-size: 0.75rem; color: var(--muted); margin-bottom: 12px; }
  #article-detail .detail-content { white-space: pre-wrap; font-size: 0.85rem; line-height: 1.7; max-height: 360px; overflow-y: auto; }
  /* Error / empty states */
  .error-state { color: var(--warning); font-size: 0.85rem; padding: 10px 0; }
  .empty-state  { color: var(--muted);   font-size: 0.85rem; padding: 10px 0; }
  /* Spinner */
  .spinner { display: inline-block; width: 14px; height: 14px; border: 2px solid var(--muted); border-top-color: var(--accent); border-radius: 50%; animation: spin 0.6s linear infinite; vertical-align: middle; margin-right: 6px; }
  @keyframes spin { to { transform: rotate(360deg); } }
  /* Standing concerns */
  .concerns-table { width: 100%; border-collapse: collapse; }
  .concerns-table td { padding: 8px 10px; border-bottom: 1px solid #253260; font-size: 0.85rem; vertical-align: middle; }
  .concerns-table tr:last-child td { border-bottom: none; }
  .concerns-table .cn-dot { width: 12px; height: 12px; border-radius: 50%; display: inline-block; flex-shrink: 0; }
  .cn-dot.green  { background: #4caf50; box-shadow: 0 0 6px #4caf5088; }
  .cn-dot.yellow { background: #ff9800; box-shadow: 0 0 6px #ff980088; }
  .cn-dot.red    { background: #e94560; box-shadow: 0 0 6px #e9456088; }
  .concerns-table .cn-name  { font-weight: 600; }
  .concerns-table .cn-notes { color: var(--muted); }
  .concerns-table .cn-ts    { color: var(--muted); white-space: nowrap; font-size: 0.75rem; }
  /* Scrollbar (webkit) */
  ::-webkit-scrollbar { width: 6px; } ::-webkit-scrollbar-track { background: var(--bg); } ::-webkit-scrollbar-thumb { background: var(--accent2); border-radius: 4px; }
</style>
</head>
<body>
<header>
  <svg width="28" height="28" viewBox="0 0 28 28" fill="none" xmlns="http://www.w3.org/2000/svg">
    <circle cx="14" cy="14" r="13" stroke="#e94560" stroke-width="2"/>
    <circle cx="14" cy="14" r="5" fill="#e94560"/>
    <line x1="14" y1="1" x2="14" y2="7"  stroke="#533483" stroke-width="2"/>
    <line x1="14" y1="21" x2="14" y2="27" stroke="#533483" stroke-width="2"/>
    <line x1="1"  y1="14" x2="7"  y2="14" stroke="#533483" stroke-width="2"/>
    <line x1="21" y1="14" x2="27" y2="14" stroke="#533483" stroke-width="2"/>
  </svg>
  <h1>Covalence Dashboard</h1>
  <span class="version">knowledge engine</span>
</header>
<main>

  <!-- ── Section 1: System Status ── -->
  <section id="section-stats">
    <h2>System Status
      <button id="btn-refresh" style="float:right;font-size:0.75rem;padding:5px 12px" onclick="loadStats()">↻ Refresh</button>
    </h2>
    <div id="stats-content"><span class="spinner"></span> Loading…</div>
  </section>

  <!-- ── Section 2: Standing Concerns ── -->
  <section id="section-concerns">
    <h2>Standing Concerns
      <button id="btn-refresh-concerns" style="float:right;font-size:0.75rem;padding:5px 12px" onclick="loadConcerns()">↻ Refresh</button>
    </h2>
    <div id="concerns-content"><span class="spinner"></span> Loading…</div>
  </section>

  <!-- ── Section 3: Search ── -->
  <section id="section-search">
    <h2>Search</h2>
    <div class="search-row">
      <input id="search-input" type="text" placeholder="Enter search query…" onkeydown="if(event.key==='Enter')runSearch()"/>
      <button onclick="runSearch()">Search</button>
    </div>
    <div id="search-results"></div>
  </section>

  <!-- ── Section 4: Article Browser ── -->
  <section id="section-articles">
    <h2>Article Browser
      <button id="btn-reload-articles" style="float:right;font-size:0.75rem;padding:5px 12px" onclick="loadArticles()">↻ Reload</button>
    </h2>
    <div id="articles-list"><span class="spinner"></span> Loading…</div>
    <div id="article-detail"></div>
  </section>

</main>
<script>
// ── Utilities ──────────────────────────────────────────────────────────────

function fmtDate(iso) {
  if (!iso) return '—';
  try { return new Date(iso).toLocaleString(); } catch { return iso; }
}

function fmtScore(n) {
  if (n == null) return '—';
  return (typeof n === 'number') ? n.toFixed(3) : n;
}

function escHtml(s) {
  return String(s ?? '')
    .replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
    .replace(/"/g,'&quot;');
}

async function apiFetch(url, opts) {
  const res = await fetch(url, opts);
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`HTTP ${res.status}: ${text}`);
  }
  return res.json();
}

// ── Section 1: System Status ───────────────────────────────────────────────

async function loadStats() {
  const el = document.getElementById('stats-content');
  el.innerHTML = '<span class="spinner"></span> Loading…';
  document.getElementById('btn-refresh').disabled = true;
  try {
    const body = await apiFetch('/admin/stats');
    const d = body.data;
    el.innerHTML = renderStats(d);
  } catch (e) {
    el.innerHTML = `<p class="error-state">⚠ Failed to load stats: ${escHtml(e.message)}</p>`;
  } finally {
    document.getElementById('btn-refresh').disabled = false;
  }
}

function renderStats(d) {
  const cards = [
    { label: 'Active Nodes',    value: d?.nodes?.active    ?? '—' },
    { label: 'Sources',         value: d?.nodes?.sources   ?? '—' },
    { label: 'Articles',        value: d?.nodes?.articles  ?? '—' },
    { label: 'Pinned',          value: d?.nodes?.pinned    ?? '—' },
    { label: 'Queue Pending',   value: d?.queue?.pending   ?? '—', warn: (d?.queue?.pending > 0) },
    { label: 'Queue Failed',    value: d?.queue?.failed    ?? '—', warn: (d?.queue?.failed  > 0) },
    { label: 'Embeddings',      value: d?.embeddings?.total       ?? '—' },
    { label: 'Missing Embeds',  value: d?.embeddings?.nodes_without ?? '—', warn: (d?.embeddings?.nodes_without > 0) },
  ];
  return `<div class="stats-grid">${cards.map(c => `
    <div class="stat-card${c.warn ? ' warn' : ''}">
      <div class="value">${escHtml(c.value)}</div>
      <div class="label">${escHtml(c.label)}</div>
    </div>`).join('')}</div>`;
}

// ── Section 2: Search ──────────────────────────────────────────────────────

async function runSearch() {
  const input = document.getElementById('search-input');
  const query = input.value.trim();
  const el = document.getElementById('search-results');
  if (!query) { el.innerHTML = '<p class="empty-state">Enter a query above.</p>'; return; }

  el.innerHTML = '<span class="spinner"></span> Searching…';
  try {
    const body = await apiFetch('/search', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ query, limit: 10 }),
    });
    const results = body.data ?? [];
    if (results.length === 0) {
      el.innerHTML = '<p class="empty-state">No results found.</p>';
      return;
    }
    el.innerHTML = `<div class="results">${results.map(renderSearchResult).join('')}</div>`;
  } catch (e) {
    el.innerHTML = `<p class="error-state">⚠ Search failed: ${escHtml(e.message)}</p>`;
  }
}

function renderSearchResult(r) {
  const isArticle = r.node_type === 'article';
  const title     = r.title || r.node_id || '(untitled)';
  const preview   = r.content_preview || '';
  return `
  <div class="result-card${isArticle ? ' article' : ''}">
    <div class="rc-header">
      <span class="rc-title"><span class="badge ${isArticle ? 'article' : 'source'}">${escHtml(r.node_type)}</span>
        &nbsp;${escHtml(title)}</span>
      <span class="rc-meta">score: ${fmtScore(r.score)}</span>
    </div>
    <div class="rc-preview">${escHtml(preview)}</div>
  </div>`;
}

// ── Section 3: Article Browser ─────────────────────────────────────────────

async function loadArticles() {
  const listEl  = document.getElementById('articles-list');
  const detailEl = document.getElementById('article-detail');
  listEl.innerHTML = '<span class="spinner"></span> Loading…';
  detailEl.classList.remove('visible');
  detailEl.innerHTML = '';
  document.getElementById('btn-reload-articles').disabled = true;
  try {
    const body = await apiFetch('/articles?limit=20');
    const articles = body.data ?? [];
    if (articles.length === 0) {
      listEl.innerHTML = '<p class="empty-state">No articles found.</p>';
      return;
    }
    listEl.innerHTML = `<div class="article-list">${articles.map(a => `
      <div class="article-item" onclick="loadArticle('${escHtml(a.id)}')">
        <span class="ai-title">${escHtml(a.title || '(untitled)')}</span>
        <span class="ai-meta">
          conf: ${fmtScore(a.confidence)}<br/>
          ${escHtml(fmtDate(a.created_at))}
        </span>
      </div>`).join('')}</div>`;
  } catch (e) {
    listEl.innerHTML = `<p class="error-state">⚠ Failed to load articles: ${escHtml(e.message)}</p>`;
  } finally {
    document.getElementById('btn-reload-articles').disabled = false;
  }
}

async function loadArticle(id) {
  const detailEl = document.getElementById('article-detail');
  detailEl.innerHTML = '<span class="spinner"></span> Loading…';
  detailEl.classList.add('visible');
  try {
    const body = await apiFetch(`/articles/${id}`);
    const a = body.data;
    const domains = (a.domain_path || []).join(', ') || '—';
    detailEl.innerHTML = `
      <h3>${escHtml(a.title || '(untitled)')}</h3>
      <div class="detail-meta">
        ID: ${escHtml(a.id)} &nbsp;|&nbsp;
        Confidence: ${fmtScore(a.confidence)} &nbsp;|&nbsp;
        Version: ${escHtml(a.version)} &nbsp;|&nbsp;
        Status: ${escHtml(a.status)}<br/>
        Domain: ${escHtml(domains)} &nbsp;|&nbsp;
        Created: ${escHtml(fmtDate(a.created_at))} &nbsp;|&nbsp;
        Modified: ${escHtml(fmtDate(a.modified_at))}
      </div>
      <div class="detail-content">${escHtml(a.content || '(no content)')}</div>`;
  } catch (e) {
    detailEl.innerHTML = `<p class="error-state">⚠ Failed to load article: ${escHtml(e.message)}</p>`;
  }
}

// ── Section 2: Standing Concerns ──────────────────────────────────────────

async function loadConcerns() {
  const el  = document.getElementById('concerns-content');
  const btn = document.getElementById('btn-refresh-concerns');
  el.innerHTML = '<span class="spinner"></span> Loading…';
  if (btn) btn.disabled = true;
  try {
    const body = await apiFetch('/admin/concerns');
    const concerns = body.data ?? [];
    if (concerns.length === 0) {
      el.innerHTML = '<p class="empty-state">No concerns data — heartbeat has not written yet.</p>';
      return;
    }
    const rows = concerns.map(c => {
      const dot   = `<span class="cn-dot ${escHtml(c.status)}"></span>`;
      const name  = `<span class="cn-name">${escHtml(c.name)}</span>`;
      const notes = `<span class="cn-notes">${escHtml(c.notes ?? '')}</span>`;
      const ts    = `<span class="cn-ts">${escHtml(fmtDate(c.updated_at))}</span>`;
      return `<tr>
        <td style="width:20px">${dot}</td>
        <td>${name}</td>
        <td>${notes}</td>
        <td>${ts}</td>
      </tr>`;
    }).join('');
    el.innerHTML = `<table class="concerns-table"><tbody>${rows}</tbody></table>`;
  } catch (e) {
    el.innerHTML = `<p class="error-state">⚠ Failed to load concerns: ${escHtml(e.message)}</p>`;
  } finally {
    if (btn) btn.disabled = false;
  }
}

// ── Boot ───────────────────────────────────────────────────────────────────

loadStats();
loadConcerns();
loadArticles();
</script>
</body>
</html>
"##;
