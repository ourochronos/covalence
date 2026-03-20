//! Configuration management endpoints.
//!
//! GET /admin/config — list all config entries
//! PUT /admin/config/:key — update a config value

use axum::Json;
use axum::extract::{Path, State};

use crate::error::ApiError;
use crate::state::AppState;

/// List all runtime config entries.
#[utoipa::path(
    get,
    path = "/admin/config",
    responses(
        (status = 200, description = "All config entries",
         body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn list_config(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entries = state.config_service.list_all().await?;
    Ok(Json(serde_json::to_value(entries).unwrap_or_default()))
}

/// Update a config value.
#[utoipa::path(
    put,
    path = "/admin/config/{key}",
    params(("key" = String, Path, description = "Config key")),
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Config updated"),
    ),
    tag = "admin"
)]
pub async fn update_config(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.config_service.set(&key, value).await?;
    Ok(Json(serde_json::json!({ "updated": key })))
}
