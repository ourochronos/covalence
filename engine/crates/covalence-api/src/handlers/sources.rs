//! Source handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::dto::{
    ChunkResponse, CreateSourceRequest, CreateSourceResponse, DeleteSourceResponse,
    PaginationParams, ReprocessSourceResponse, SourceResponse,
};
use crate::state::AppState;

/// Ingest a new source.
///
/// Accepts either base64-encoded `content` or a `url` to fetch.
/// When `url` is provided, the server fetches the content,
/// auto-detects MIME type and source classification, and extracts
/// metadata (title, author, date) from the response.
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
    // Build metadata, merging format_origin, authors, title, and
    // author into the JSONB metadata object.
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
        if let Some(ref title) = req.title {
            map.insert("title".to_string(), serde_json::json!(title));
        }
        if let Some(ref author) = req.author {
            map.insert("author".to_string(), serde_json::json!(author));
        }
    }

    let id = if let Some(ref url) = req.url {
        // URL-based ingestion: fetch, detect, extract metadata.
        state
            .source_service
            .ingest_url(
                url,
                req.source_type.as_deref(),
                req.mime.as_deref(),
                req.title.as_deref(),
                req.author.as_deref(),
                metadata,
            )
            .await?
    } else if let Some(ref content_b64) = req.content {
        // Direct content ingestion (existing flow).
        use base64::Engine;
        let content = base64::engine::general_purpose::STANDARD
            .decode(content_b64)
            .map_err(|e| {
                covalence_core::error::Error::InvalidInput(format!("invalid base64 content: {e}"))
            })?;

        let source_type = req.source_type.as_deref().ok_or_else(|| {
            covalence_core::error::Error::InvalidInput(
                "source_type is required when providing content directly".to_string(),
            )
        })?;
        let mime = req.mime.as_deref().unwrap_or("text/plain");

        state
            .source_service
            .ingest(&content, source_type, mime, req.uri.as_deref(), metadata)
            .await?
    } else {
        return Err(covalence_core::error::Error::InvalidInput(
            "either 'content' (base64) or 'url' must be provided".to_string(),
        )
        .into());
    };

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

/// Reprocess a source through the current pipeline.
///
/// Re-runs ingestion (convert, normalize, chunk, embed, extract)
/// using the source's existing raw content and the current pipeline
/// config. Old extractions are superseded, old chunks are replaced,
/// and entity resolution ensures convergent graph state.
#[utoipa::path(
    post,
    path = "/sources/{id}/reprocess",
    params(("id" = Uuid, Path, description = "Source ID")),
    responses(
        (status = 200, description = "Source reprocessed", body = ReprocessSourceResponse),
        (status = 404, description = "Source not found"),
    ),
    tag = "sources"
)]
pub async fn reprocess_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ReprocessSourceResponse>, ApiError> {
    let result = state.source_service.reprocess(id.into()).await?;
    Ok(Json(ReprocessSourceResponse {
        source_id: result.source_id,
        extractions_superseded: result.extractions_superseded,
        chunks_deleted: result.chunks_deleted,
        chunks_created: result.chunks_created,
        content_version: result.content_version,
    }))
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
