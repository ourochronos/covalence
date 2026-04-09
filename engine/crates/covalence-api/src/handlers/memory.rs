//! Memory API handlers.
//!
//! High-level memory interface for AI agent integration.
//! Delegates to [`AgentMemoryService`] for store, recall,
//! consolidate, reflect, and forget operations.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;

use covalence_core::services::agent_memory;

use crate::handlers::dto;
use crate::state::AppState;

/// POST /memory — Store a new memory.
pub async fn store_memory(
    State(state): State<AppState>,
    Json(req): Json<dto::StoreMemoryRequest>,
) -> Result<Json<dto::StoreMemoryResponse>, StatusCode> {
    if req.content.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let svc_req = agent_memory::MemoryStoreRequest {
        content: req.content,
        topic: req.topic,
        metadata: req.metadata,
        confidence: req.confidence,
        agent_id: req.agent_id,
        task_id: req.task_id,
    };

    let resp = state
        .agent_memory_service
        .store(svc_req)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory store failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(dto::StoreMemoryResponse {
        id: resp.id,
        entities_extracted: resp.entities_extracted,
        status: resp.status,
    }))
}

/// POST /memory/recall — Search memories.
pub async fn recall_memory(
    State(state): State<AppState>,
    Json(req): Json<dto::RecallMemoryRequest>,
) -> Result<Json<Vec<dto::MemoryItemResponse>>, StatusCode> {
    if req.query.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    if let Some(mc) = req.min_confidence {
        if !mc.is_finite() || !(0.0..=1.0).contains(&mc) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let svc_req = agent_memory::MemoryRecallRequest {
        query: req.query,
        limit: req.limit,
        topic: req.topic,
        min_confidence: req.min_confidence,
        agent_id: req.agent_id,
    };

    let items = state
        .agent_memory_service
        .recall(svc_req)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory recall failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(
        items
            .into_iter()
            .map(|i| dto::MemoryItemResponse {
                id: i.id,
                content: i.content,
                topic: i.topic,
                relevance: i.relevance,
                confidence: i.confidence,
                stored_at: i.stored_at,
                agent_id: i.agent_id,
                access_count: i.access_count,
                last_accessed: i.last_accessed,
            })
            .collect(),
    ))
}

/// DELETE /memory/{id} — Forget a specific memory.
pub async fn forget_memory(State(state): State<AppState>, Path(id): Path<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let uuid = match id.parse::<uuid::Uuid>() {
        Ok(u) => u,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state.agent_memory_service.forget(uuid).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "Memory record not found for this ID").into_response(),
        Err(e) => {
            tracing::error!(error = %e, "memory forget failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// GET /memory/status — Get memory system status.
pub async fn memory_status(
    State(state): State<AppState>,
    Query(params): Query<dto::MemoryStatusParams>,
) -> Result<Json<dto::MemoryStatusResponse>, StatusCode> {
    let status = state
        .agent_memory_service
        .status(params.agent_id.as_deref())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory status failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(dto::MemoryStatusResponse {
        total_memories: status.total_memories,
        agent_memories: status.agent_memories,
    }))
}

/// POST /memory/consolidate — Consolidate similar memories.
pub async fn consolidate_memory(
    State(state): State<AppState>,
    Json(req): Json<dto::ConsolidateMemoryRequest>,
) -> Result<Json<dto::ConsolidateMemoryResponse>, StatusCode> {
    let svc_req = agent_memory::ConsolidateRequest {
        agent_id: req.agent_id,
        threshold: req.threshold,
    };

    let resp = state
        .agent_memory_service
        .consolidate(svc_req)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory consolidate failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(dto::ConsolidateMemoryResponse {
        groups_found: resp.groups_found,
        merged: resp.merged,
        expired: resp.expired,
        status: resp.status,
    }))
}

/// POST /memory/reflect/:session_id — Reflect on a session.
pub async fn reflect_memory(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<dto::ReflectParams>,
) -> Result<Json<dto::ReflectMemoryResponse>, StatusCode> {
    let uuid = match session_id.parse::<uuid::Uuid>() {
        Ok(u) => u,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    let resp = state
        .agent_memory_service
        .reflect(uuid, params.agent_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory reflect failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(dto::ReflectMemoryResponse {
        learnings_stored: resp.learnings_stored,
        status: resp.status,
    }))
}

/// POST /memory/forget-old — Apply forgetting to expired memories.
pub async fn apply_forgetting(
    State(state): State<AppState>,
    Json(req): Json<dto::ForgetOldMemoryRequest>,
) -> Result<Json<dto::ForgetOldMemoryResponse>, StatusCode> {
    let retention_days = req.retention_days.unwrap_or(90);

    let resp = state
        .agent_memory_service
        .apply_forgetting(retention_days)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory forget-old failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(dto::ForgetOldMemoryResponse {
        deleted: resp.deleted,
        status: resp.status,
    }))
}
