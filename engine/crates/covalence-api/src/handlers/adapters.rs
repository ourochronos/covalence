//! Source adapter management endpoints.

use axum::Json;
use axum::extract::State;

use crate::error::ApiError;
use crate::state::AppState;

/// List all source adapters.
#[utoipa::path(
    get,
    path = "/admin/adapters",
    responses(
        (status = 200, description = "All source adapters",
         body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn list_adapters(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let svc = covalence_core::services::adapter_service::AdapterService::new(
        std::sync::Arc::clone(&state.repo),
    );
    let adapters = svc.list_all().await?;
    Ok(Json(serde_json::to_value(adapters).unwrap_or_default()))
}
