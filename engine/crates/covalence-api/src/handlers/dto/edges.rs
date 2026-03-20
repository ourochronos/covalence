//! Edge-related DTOs.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Response for an edge entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct EdgeResponse {
    pub id: Uuid,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub rel_type: String,
    pub weight: f64,
    pub confidence: f64,
    pub clearance_level: i32,
    pub created_at: String,
}

/// Request body for correcting an edge.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CorrectEdgeRequest {
    /// New relationship type (optional).
    pub rel_type: Option<String>,
    /// New confidence value 0.0–1.0 (optional).
    pub confidence: Option<f64>,
}

/// Query parameter for edge deletion reason.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct DeleteEdgeParams {
    /// Reason for deleting the edge (required).
    pub reason: String,
}
