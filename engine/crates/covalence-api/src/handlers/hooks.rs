//! Lifecycle hook handlers — CRUD for external pipeline hooks.

use axum::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use covalence_core::services::hooks::{HookPhase, LifecycleHook};
use covalence_core::storage::traits::HookRepo;

use crate::error::{ApiError, validate_request};
use crate::handlers::dto::{
    CreateHookRequest, CreateHookResponse, DeleteHookResponse, HookResponse, ListHooksResponse,
};
use crate::state::AppState;

/// Map a domain LifecycleHook to the API response DTO.
fn to_response(hook: &LifecycleHook) -> HookResponse {
    HookResponse {
        id: hook.id,
        name: hook.name.clone(),
        phase: hook.phase.as_str().to_string(),
        hook_url: hook.hook_url.clone(),
        adapter_id: hook.adapter_id,
        timeout_ms: hook.timeout_ms,
        fail_open: hook.fail_open,
        is_active: hook.is_active,
    }
}

/// Create a new lifecycle hook.
///
/// POST /api/v1/admin/hooks
#[utoipa::path(
    post,
    path = "/admin/hooks",
    request_body = CreateHookRequest,
    responses(
        (status = 200, description = "Hook created", body = CreateHookResponse),
    ),
    tag = "admin"
)]
pub async fn create_hook(
    State(state): State<AppState>,
    Json(req): Json<CreateHookRequest>,
) -> Result<Json<CreateHookResponse>, ApiError> {
    validate_request(&req)?;

    let phase = HookPhase::parse(&req.phase).ok_or_else(|| {
        ApiError::from(covalence_core::error::Error::InvalidInput(format!(
            "invalid phase '{}' — must be pre_search, post_search, or \
             post_synthesis",
            req.phase
        )))
    })?;

    let hook = LifecycleHook {
        id: Uuid::new_v4(),
        name: req.name,
        phase,
        hook_url: req.hook_url,
        adapter_id: req.adapter_id,
        timeout_ms: req.timeout_ms.unwrap_or(2000),
        fail_open: req.fail_open.unwrap_or(true),
        is_active: true,
    };

    HookRepo::create(&*state.repo, &hook).await?;

    Ok(Json(CreateHookResponse {
        hook: to_response(&hook),
    }))
}

/// List all lifecycle hooks.
///
/// GET /api/v1/admin/hooks
#[utoipa::path(
    get,
    path = "/admin/hooks",
    responses(
        (status = 200, description = "All lifecycle hooks", body = ListHooksResponse),
    ),
    tag = "admin"
)]
pub async fn list_hooks(
    State(state): State<AppState>,
) -> Result<Json<ListHooksResponse>, ApiError> {
    let hooks = HookRepo::list_all(&*state.repo).await?;
    Ok(Json(ListHooksResponse {
        hooks: hooks.iter().map(to_response).collect(),
    }))
}

/// Delete a lifecycle hook.
///
/// DELETE /api/v1/admin/hooks/{id}
#[utoipa::path(
    delete,
    path = "/admin/hooks/{id}",
    params(
        ("id" = Uuid, Path, description = "Hook ID to delete")
    ),
    responses(
        (status = 200, description = "Deletion result", body = DeleteHookResponse),
    ),
    tag = "admin"
)]
pub async fn delete_hook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteHookResponse>, ApiError> {
    let deleted = HookRepo::delete(&*state.repo, id).await?;
    Ok(Json(DeleteHookResponse { deleted }))
}
