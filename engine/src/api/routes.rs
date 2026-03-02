//! Axum route handlers — thin layer mapping HTTP → service calls.

use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::services::provenance_trace_service::{ProvenanceTraceService, TraceRequest};
use crate::services::{
    admin_service::*, article_service::*, contention_service::*, edge_service::*,
    memory_service::*, search_service::*, session_service::*, source_service::*,
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
        .route("/articles/{id}/trace", post(article_provenance_trace))
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
    let svc = EdgeService::new(state.pool);
    match svc.create(req).await {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!({"data": resp}))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn edge_delete(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
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
    let svc = AdminService::new(state.pool);
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
    let service = SearchService::new(state.pool.clone());
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
