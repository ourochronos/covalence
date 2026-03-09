//! Memory API handlers.
//!
//! High-level memory interface for AI agent integration.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use covalence_core::services::memory::{
    MemoryItem, MemoryRecallRequest, MemoryStatus, MemoryStoreRequest, MemoryStoreResponse,
};

use crate::state::AppState;

/// POST /memory — Store a new memory.
pub async fn store_memory(
    State(_state): State<AppState>,
    Json(req): Json<MemoryStoreRequest>,
) -> Result<Json<MemoryStoreResponse>, StatusCode> {
    // TODO: wire to SourceService.ingest with source_type =
    // "observation"
    Ok(Json(MemoryStoreResponse {
        id: uuid::Uuid::new_v4().to_string(),
        entities_extracted: 0,
        status: format!("Memory stored ({} chars)", req.content.len()),
    }))
}

/// POST /memory/recall — Search memories.
pub async fn recall_memory(
    State(_state): State<AppState>,
    Json(_req): Json<MemoryRecallRequest>,
) -> Result<Json<Vec<MemoryItem>>, StatusCode> {
    // TODO: wire to SearchService.search
    Ok(Json(Vec::new()))
}

/// DELETE /memory/:id — Forget a specific memory.
pub async fn forget_memory(
    State(_state): State<AppState>,
    axum::extract::Path(_id): axum::extract::Path<String>,
) -> StatusCode {
    // TODO: wire to SourceService.delete
    StatusCode::NO_CONTENT
}

/// GET /memory/status — Get memory system status.
pub async fn memory_status(
    State(_state): State<AppState>,
) -> Result<Json<MemoryStatus>, StatusCode> {
    Ok(Json(MemoryStatus {
        total_memories: 0,
        total_entities: 0,
        total_relationships: 0,
        communities: 0,
    }))
}
