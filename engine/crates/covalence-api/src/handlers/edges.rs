//! Edge handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::dto::{CorrectEdgeRequest, CurationResponse, DeleteEdgeParams, EdgeResponse};
use crate::state::AppState;

/// Get an edge by ID.
#[utoipa::path(
    get,
    path = "/edges/{id}",
    params(("id" = Uuid, Path, description = "Edge ID")),
    responses(
        (status = 200, description = "Edge found", body = EdgeResponse),
        (status = 404, description = "Edge not found"),
    ),
    tag = "edges"
)]
pub async fn get_edge(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<EdgeResponse>, ApiError> {
    let edge =
        state
            .edge_service
            .get(id.into())
            .await?
            .ok_or(covalence_core::error::Error::NotFound {
                entity_type: "edge",
                id: id.to_string(),
            })?;

    Ok(Json(EdgeResponse {
        id: edge.id.into_uuid(),
        source_node_id: edge.source_node_id.into_uuid(),
        target_node_id: edge.target_node_id.into_uuid(),
        rel_type: edge.rel_type,
        weight: edge.weight,
        confidence: edge.confidence,
        clearance_level: edge.clearance_level.as_i32(),
        created_at: edge.created_at.to_rfc3339(),
    }))
}

/// Correct an edge's fields.
#[utoipa::path(
    post,
    path = "/edges/{id}/correct",
    params(("id" = Uuid, Path, description = "Edge ID")),
    request_body = CorrectEdgeRequest,
    responses(
        (status = 200, description = "Edge corrected", body = CurationResponse),
        (status = 404, description = "Edge not found"),
    ),
    tag = "edges"
)]
pub async fn correct_edge(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CorrectEdgeRequest>,
) -> Result<Json<CurationResponse>, ApiError> {
    let audit_id = state
        .edge_service
        .correct(id.into(), req.rel_type, req.confidence)
        .await?;
    Ok(Json(CurationResponse {
        success: true,
        audit_log_id: audit_id.into_uuid(),
    }))
}

/// Delete an edge with a reason.
#[utoipa::path(
    delete,
    path = "/edges/{id}",
    params(
        ("id" = Uuid, Path, description = "Edge ID"),
        DeleteEdgeParams,
    ),
    responses(
        (status = 200, description = "Edge deleted", body = CurationResponse),
        (status = 404, description = "Edge not found"),
    ),
    tag = "edges"
)]
pub async fn delete_edge(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<DeleteEdgeParams>,
) -> Result<Json<CurationResponse>, ApiError> {
    let audit_id = state
        .edge_service
        .delete_with_reason(id.into(), params.reason)
        .await?;
    Ok(Json(CurationResponse {
        success: true,
        audit_log_id: audit_id.into_uuid(),
    }))
}
