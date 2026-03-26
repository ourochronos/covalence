//! Extension management handlers.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;

use covalence_core::extensions::ExtensionLoader;

use crate::error::ApiError;
use crate::handlers::dto::{ListExtensionsResponse, ReloadExtensionsResponse};
use crate::state::AppState;

/// Resolve the extensions directory from the environment or default.
fn extensions_dir() -> String {
    std::env::var("COVALENCE_EXTENSIONS_DIR").unwrap_or_else(|_| "extensions".to_string())
}

/// List available extensions found on disk.
///
/// GET /api/v1/admin/extensions
#[utoipa::path(
    get,
    path = "/admin/extensions",
    responses(
        (status = 200, description = "Available extensions",
         body = ListExtensionsResponse),
    ),
    tag = "admin"
)]
pub async fn list_extensions() -> Result<Json<ListExtensionsResponse>, ApiError> {
    let dir = extensions_dir();
    let names = ExtensionLoader::list_available(&dir).map_err(ApiError::from)?;
    Ok(Json(ListExtensionsResponse { extensions: names }))
}

/// Reload extensions from disk and seed the database.
///
/// POST /api/v1/admin/extensions/reload
#[utoipa::path(
    post,
    path = "/admin/extensions/reload",
    responses(
        (status = 200, description = "Reload result",
         body = ReloadExtensionsResponse),
    ),
    tag = "admin"
)]
pub async fn reload_extensions(
    State(state): State<AppState>,
) -> Result<Json<ReloadExtensionsResponse>, ApiError> {
    let dir = extensions_dir();
    let loader = ExtensionLoader::new(Arc::clone(&state.repo));
    let results = loader.load_directory(&dir).await.map_err(ApiError::from)?;
    let loaded: Vec<String> = results.iter().map(|r| r.name.clone()).collect();

    // Refresh ontology cache so newly loaded types are visible.
    if !loaded.is_empty() {
        if let Err(e) = state.ontology_service.refresh().await {
            tracing::warn!(
                error = %e,
                "ontology refresh after extension reload failed"
            );
        }
    }

    Ok(Json(ReloadExtensionsResponse { loaded }))
}
