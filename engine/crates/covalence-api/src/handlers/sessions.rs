//! Session handlers — lightweight conversation context endpoints.

use axum::Json;
use axum::extract::{Path, Query, State};

use crate::error::{ApiError, validate_request};
use crate::handlers::dto::{
    AddTurnRequest, CreateSessionRequest, GetTurnsParams, SessionResponse, TurnResponse,
};
use crate::state::AppState;

/// Create a new session.
#[utoipa::path(
    post,
    path = "/sessions",
    request_body = CreateSessionRequest,
    responses(
        (status = 201, description = "Session created", body = SessionResponse),
    ),
    tag = "sessions"
)]
pub async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(axum::http::StatusCode, Json<SessionResponse>), ApiError> {
    validate_request(&req)?;

    let session = state
        .session_service
        .create_session(req.name.as_deref(), req.metadata)
        .await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(SessionResponse {
            id: session.id.to_string(),
            name: session.name,
            metadata: session.metadata,
            created_at: session.created_at,
            updated_at: session.updated_at,
        }),
    ))
}

/// List sessions (most recently updated first).
#[utoipa::path(
    get,
    path = "/sessions",
    params(
        ("limit" = Option<i64>, Query, description = "Max sessions to return"),
        ("offset" = Option<i64>, Query, description = "Offset for pagination"),
    ),
    responses(
        (status = 200, description = "Session list", body = Vec<SessionResponse>),
    ),
    tag = "sessions"
)]
pub async fn list_sessions(
    State(state): State<AppState>,
    Query(params): Query<crate::handlers::dto::PaginationParams>,
) -> Result<Json<Vec<SessionResponse>>, ApiError> {
    let limit = params.limit.unwrap_or(20).min(200);
    let offset = params.offset.unwrap_or(0);

    let sessions = state.session_service.list_sessions(limit, offset).await?;

    let resp: Vec<SessionResponse> = sessions
        .into_iter()
        .map(|s| SessionResponse {
            id: s.id.to_string(),
            name: s.name,
            metadata: s.metadata,
            created_at: s.created_at,
            updated_at: s.updated_at,
        })
        .collect();

    Ok(Json(resp))
}

/// Get a single session by ID.
#[utoipa::path(
    get,
    path = "/sessions/{id}",
    params(("id" = String, Path, description = "Session UUID")),
    responses(
        (status = 200, description = "Session details", body = SessionResponse),
        (status = 404, description = "Session not found"),
    ),
    tag = "sessions"
)]
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionResponse>, ApiError> {
    let uuid = id.parse::<uuid::Uuid>().map_err(|_| {
        ApiError::from(covalence_core::error::Error::InvalidInput(
            "invalid UUID".to_string(),
        ))
    })?;

    let session = state
        .session_service
        .get_session(uuid)
        .await?
        .ok_or_else(|| {
            ApiError::from(covalence_core::error::Error::NotFound {
                entity_type: "session",
                id: id.clone(),
            })
        })?;

    Ok(Json(SessionResponse {
        id: session.id.to_string(),
        name: session.name,
        metadata: session.metadata,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }))
}

/// Get turns for a session (chronological order).
#[utoipa::path(
    get,
    path = "/sessions/{id}/turns",
    params(
        ("id" = String, Path, description = "Session UUID"),
        ("last_n" = Option<i64>, Query, description = "Limit to last N turns"),
    ),
    responses(
        (status = 200, description = "Turn history", body = Vec<TurnResponse>),
    ),
    tag = "sessions"
)]
pub async fn get_turns(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<GetTurnsParams>,
) -> Result<Json<Vec<TurnResponse>>, ApiError> {
    let uuid = id.parse::<uuid::Uuid>().map_err(|_| {
        ApiError::from(covalence_core::error::Error::InvalidInput(
            "invalid UUID".to_string(),
        ))
    })?;

    let last_n = params.last_n.map(|n| n.min(500));
    let turns = state.session_service.get_history(uuid, last_n).await?;

    let resp: Vec<TurnResponse> = turns
        .into_iter()
        .map(|t| TurnResponse {
            id: t.id.to_string(),
            session_id: t.session_id.to_string(),
            role: t.role,
            content: t.content,
            metadata: t.metadata,
            ordinal: t.ordinal,
            created_at: t.created_at,
        })
        .collect();

    Ok(Json(resp))
}

/// Add a turn to a session.
#[utoipa::path(
    post,
    path = "/sessions/{id}/turns",
    params(("id" = String, Path, description = "Session UUID")),
    request_body = AddTurnRequest,
    responses(
        (status = 201, description = "Turn added", body = TurnResponse),
    ),
    tag = "sessions"
)]
pub async fn add_turn(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AddTurnRequest>,
) -> Result<(axum::http::StatusCode, Json<TurnResponse>), ApiError> {
    validate_request(&req)?;

    let uuid = id.parse::<uuid::Uuid>().map_err(|_| {
        ApiError::from(covalence_core::error::Error::InvalidInput(
            "invalid UUID".to_string(),
        ))
    })?;

    let valid_roles = ["user", "assistant", "system", "tool"];
    if !valid_roles.contains(&req.role.as_str()) {
        return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
            format!(
                "invalid role '{}': must be one of {:?}",
                req.role, valid_roles
            ),
        )));
    }

    let turn = state
        .session_service
        .add_turn(uuid, &req.role, &req.content, req.metadata)
        .await?;

    // Optionally ingest the turn content as a micro-source for
    // graph extraction. The existing pipeline handles the rest.
    if req.extract == Some(true) {
        let meta = serde_json::json!({
            "session_id": uuid.to_string(),
            "turn_id": turn.id.to_string(),
            "role": turn.role,
        });
        match state
            .source_service
            .ingest(
                turn.content.as_bytes(),
                "conversation",
                "text/plain",
                None,
                meta,
            )
            .await
        {
            Ok(source_id) => {
                tracing::info!(
                    turn_id = %turn.id,
                    source_id = %source_id,
                    "turn content ingested for extraction"
                );
            }
            Err(e) => {
                tracing::warn!(
                    turn_id = %turn.id,
                    error = %e,
                    "failed to ingest turn content for extraction"
                );
            }
        }
    }

    Ok((
        axum::http::StatusCode::CREATED,
        Json(TurnResponse {
            id: turn.id.to_string(),
            session_id: turn.session_id.to_string(),
            role: turn.role,
            content: turn.content,
            metadata: turn.metadata,
            ordinal: turn.ordinal,
            created_at: turn.created_at,
        }),
    ))
}

/// Delete a session and all its turns.
#[utoipa::path(
    delete,
    path = "/sessions/{id}",
    params(("id" = String, Path, description = "Session UUID")),
    responses(
        (status = 204, description = "Session deleted"),
        (status = 404, description = "Session not found"),
    ),
    tag = "sessions"
)]
pub async fn close_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    let uuid = id.parse::<uuid::Uuid>().map_err(|_| {
        ApiError::from(covalence_core::error::Error::InvalidInput(
            "invalid UUID".to_string(),
        ))
    })?;

    let deleted = state.session_service.close_session(uuid).await?;

    if deleted {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::from(covalence_core::error::Error::NotFound {
            entity_type: "session",
            id,
        }))
    }
}
