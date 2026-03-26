//! Lifecycle hook DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// Request body for creating a lifecycle hook.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct CreateHookRequest {
    /// Human-readable name (must be unique).
    #[validate(length(min = 1, max = 200))]
    pub name: String,
    /// Pipeline phase: pre_search, post_search, or post_synthesis.
    #[validate(length(min = 1, max = 50))]
    pub phase: String,
    /// URL to POST to when the hook fires.
    #[validate(length(min = 1, max = 2000))]
    pub hook_url: String,
    /// Optional adapter ID to scope the hook.
    pub adapter_id: Option<Uuid>,
    /// Per-hook timeout in milliseconds (default 2000).
    pub timeout_ms: Option<i32>,
    /// If true, errors are logged but the pipeline continues
    /// (default true).
    pub fail_open: Option<bool>,
}

/// Response for a single lifecycle hook.
#[derive(Debug, Serialize, ToSchema)]
pub struct HookResponse {
    /// Hook ID.
    pub id: Uuid,
    /// Human-readable name.
    pub name: String,
    /// Pipeline phase.
    pub phase: String,
    /// URL to POST to.
    pub hook_url: String,
    /// Optional adapter ID.
    pub adapter_id: Option<Uuid>,
    /// Timeout in milliseconds.
    pub timeout_ms: i32,
    /// Whether the hook fails open.
    pub fail_open: bool,
    /// Whether the hook is active.
    pub is_active: bool,
}

/// Response for listing hooks.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListHooksResponse {
    /// List of hooks.
    pub hooks: Vec<HookResponse>,
}

/// Response after creating a hook.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateHookResponse {
    /// The created hook.
    pub hook: HookResponse,
}

/// Response after deleting a hook.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteHookResponse {
    /// Whether a hook was deleted.
    pub deleted: bool,
}
