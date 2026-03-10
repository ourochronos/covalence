//! Source handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::dto::{
    ChunkResponse, CreateSourceRequest, CreateSourceResponse, DeleteSourceResponse,
    PaginationParams, SourceResponse,
};
use crate::state::AppState;

/// Ingest a new source.
#[utoipa::path(
    post,
    path = "/sources",
    request_body = CreateSourceRequest,
    responses(
        (status = 201, description = "Source created", body = CreateSourceResponse),
        (status = 400, description = "Invalid input"),
    ),
    tag = "sources"
)]
pub async fn create_source(
    State(state): State<AppState>,
    Json(req): Json<CreateSourceRequest>,
) -> Result<(axum::http::StatusCode, Json<CreateSourceResponse>), ApiError> {
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD
        .decode(&req.content)
        .map_err(|e| {
            covalence_core::error::Error::InvalidInput(format!("invalid base64 content: {e}"))
        })?;

    let mime = req.mime.as_deref().unwrap_or("text/plain");

    // Build metadata, merging format_origin and authors into the
    // JSONB metadata object.
    let mut metadata = req
        .metadata
        .unwrap_or(serde_json::Value::Object(Default::default()));
    if let serde_json::Value::Object(ref mut map) = metadata {
        if let Some(ref fmt) = req.format_origin {
            map.insert("format_origin".to_string(), serde_json::json!(fmt));
        }
        if let Some(ref authors) = req.authors {
            map.insert("authors".to_string(), serde_json::json!(authors));
        }
    }

    let id = state
        .source_service
        .ingest(
            &content,
            &req.source_type,
            mime,
            req.uri.as_deref(),
            metadata,
        )
        .await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreateSourceResponse { id: id.into_uuid() }),
    ))
}

/// Get a source by ID.
#[utoipa::path(
    get,
    path = "/sources/{id}",
    params(("id" = Uuid, Path, description = "Source ID")),
    responses(
        (status = 200, description = "Source found", body = SourceResponse),
        (status = 404, description = "Source not found"),
    ),
    tag = "sources"
)]
pub async fn get_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceResponse>, ApiError> {
    let source = state.source_service.get(id.into()).await?.ok_or(
        covalence_core::error::Error::NotFound {
            entity_type: "source",
            id: id.to_string(),
        },
    )?;

    Ok(Json(SourceResponse {
        id: source.id.into_uuid(),
        source_type: source.source_type,
        uri: source.uri,
        title: source.title,
        author: source.author,
        ingested_at: source.ingested_at.to_rfc3339(),
        reliability_score: source.reliability_score,
        clearance_level: source.clearance_level.as_i32(),
        content_version: source.content_version,
    }))
}

/// List sources with pagination.
#[utoipa::path(
    get,
    path = "/sources",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of sources", body = Vec<SourceResponse>),
    ),
    tag = "sources"
)]
pub async fn list_sources(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<SourceResponse>>, ApiError> {
    let sources = state
        .source_service
        .list(params.limit(), params.offset())
        .await?;

    Ok(Json(
        sources
            .into_iter()
            .map(|s| SourceResponse {
                id: s.id.into_uuid(),
                source_type: s.source_type,
                uri: s.uri,
                title: s.title,
                author: s.author,
                ingested_at: s.ingested_at.to_rfc3339(),
                reliability_score: s.reliability_score,
                clearance_level: s.clearance_level.as_i32(),
                content_version: s.content_version,
            })
            .collect(),
    ))
}

/// Get chunks for a source.
#[utoipa::path(
    get,
    path = "/sources/{id}/chunks",
    params(("id" = Uuid, Path, description = "Source ID")),
    responses(
        (status = 200, description = "Chunks for source", body = Vec<ChunkResponse>),
    ),
    tag = "sources"
)]
pub async fn get_source_chunks(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ChunkResponse>>, ApiError> {
    let chunks = state.source_service.get_chunks(id.into()).await?;

    Ok(Json(
        chunks
            .into_iter()
            .map(|c| ChunkResponse {
                id: c.id.into_uuid(),
                source_id: c.source_id.into_uuid(),
                level: c.level,
                ordinal: c.ordinal,
                content: c.content,
                token_count: c.token_count,
            })
            .collect(),
    ))
}

/// Delete a source and its chunks.
#[utoipa::path(
    delete,
    path = "/sources/{id}",
    params(("id" = Uuid, Path, description = "Source ID")),
    responses(
        (status = 200, description = "Source deleted", body = DeleteSourceResponse),
    ),
    tag = "sources"
)]
pub async fn delete_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteSourceResponse>, ApiError> {
    let result = state.source_service.delete(id.into()).await?;
    Ok(Json(DeleteSourceResponse {
        deleted: result.deleted,
        chunks_deleted: result.chunks_deleted,
    }))
}
