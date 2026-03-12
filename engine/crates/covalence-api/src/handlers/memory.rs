//! Memory API handlers.
//!
//! High-level memory interface for AI agent integration.
//! Wraps source ingestion and search with a simple
//! store/recall/forget interface.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

use covalence_core::search::strategy::SearchStrategy;
use covalence_core::services::memory::{
    MemoryItem, MemoryRecallRequest, MemoryStatus, MemoryStoreRequest, MemoryStoreResponse,
};

use crate::state::AppState;

/// POST /memory — Store a new memory.
pub async fn store_memory(
    State(state): State<AppState>,
    Json(req): Json<MemoryStoreRequest>,
) -> Result<Json<MemoryStoreResponse>, StatusCode> {
    if req.content.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let metadata = match (&req.topic, &req.metadata) {
        (Some(topic), Some(extra)) => {
            let mut m = extra.clone();
            if let Some(obj) = m.as_object_mut() {
                obj.insert("topic".to_string(), json!(topic));
            }
            m
        }
        (Some(topic), None) => json!({ "topic": topic }),
        (None, Some(extra)) => extra.clone(),
        (None, None) => json!({}),
    };

    let source_id = state
        .source_service
        .ingest(
            req.content.as_bytes(),
            "observation",
            "text/plain",
            None,
            metadata,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory store failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(MemoryStoreResponse {
        id: source_id.into_uuid().to_string(),
        entities_extracted: 0,
        status: format!("Memory stored ({} chars)", req.content.len()),
    }))
}

/// POST /memory/recall — Search memories.
pub async fn recall_memory(
    State(state): State<AppState>,
    Json(req): Json<MemoryRecallRequest>,
) -> Result<Json<Vec<MemoryItem>>, StatusCode> {
    if req.query.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    if let Some(mc) = req.min_confidence {
        if !mc.is_finite() || !(0.0..=1.0).contains(&mc) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let limit = req.limit.unwrap_or(10).min(200);

    let filters = if req.min_confidence.is_some() {
        Some(covalence_core::services::search::SearchFilters {
            min_confidence: req.min_confidence,
            node_types: None,
            date_range: None,
            source_types: None,
        })
    } else {
        None
    };

    let results = state
        .search_service
        .search(&req.query, SearchStrategy::Auto, limit, filters)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "memory recall failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let items = results
        .into_iter()
        .map(|r| MemoryItem {
            id: r.id.to_string(),
            content: r.snippet.unwrap_or_default(),
            topic: None,
            relevance: r.fused_score,
            confidence: r.confidence.unwrap_or(1.0),
            stored_at: r.created_at.unwrap_or_default(),
        })
        .collect();

    Ok(Json(items))
}

/// DELETE /memory/:id — Forget a specific memory.
pub async fn forget_memory(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    let uuid = match id.parse::<uuid::Uuid>() {
        Ok(u) => u,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    match state.source_service.delete(uuid.into()).await {
        Ok(result) if result.deleted => StatusCode::NO_CONTENT,
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!(error = %e, "memory forget failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// GET /memory/status — Get memory system status.
pub async fn memory_status(
    State(state): State<AppState>,
) -> Result<Json<MemoryStatus>, StatusCode> {
    let total_memories = state.source_service.count().await.unwrap_or(0) as u64;

    let graph = state.graph.read().await;
    let total_entities = graph.node_count() as u64;
    let total_relationships = graph.edge_count() as u64;
    let communities =
        covalence_core::graph::community::detect_communities(graph.graph()).len() as u64;

    Ok(Json(MemoryStatus {
        total_memories,
        total_entities,
        total_relationships,
        communities,
    }))
}
