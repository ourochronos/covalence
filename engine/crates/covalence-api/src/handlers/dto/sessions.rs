//! Session/conversation DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Request body for creating a new session.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct CreateSessionRequest {
    /// Optional human-readable session name.
    #[validate(length(max = 500))]
    pub name: Option<String>,
    /// Arbitrary metadata to attach to the session.
    pub metadata: Option<serde_json::Value>,
}

/// Request body for adding a turn to a session.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct AddTurnRequest {
    /// Role of the turn author: user, assistant, system, or tool.
    #[validate(length(min = 1, max = 20))]
    pub role: String,
    /// The message content.
    #[validate(length(min = 1, max = 100_000))]
    pub content: String,
    /// Optional turn metadata.
    pub metadata: Option<serde_json::Value>,
    /// When true, the turn content is ingested as a micro-source
    /// (`source_type = "conversation"`) and enqueued for extraction.
    /// Opt-in: defaults to false.
    #[serde(default)]
    pub extract: Option<bool>,
}

/// Query parameters for listing turns.
#[derive(Debug, Deserialize)]
pub struct GetTurnsParams {
    /// Maximum number of recent turns to return (default 50).
    pub last_n: Option<i64>,
}

/// Response for a session.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionResponse {
    /// Session ID.
    pub id: String,
    /// Optional session name.
    pub name: Option<String>,
    /// Session metadata.
    pub metadata: serde_json::Value,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Response for a single turn.
#[derive(Debug, Serialize, ToSchema)]
pub struct TurnResponse {
    /// Turn ID.
    pub id: String,
    /// Session ID.
    pub session_id: String,
    /// Role: user, assistant, system, or tool.
    pub role: String,
    /// Message content.
    pub content: String,
    /// Turn metadata.
    pub metadata: serde_json::Value,
    /// Ordinal position within the session.
    pub ordinal: i32,
    /// When this turn was created.
    pub created_at: DateTime<Utc>,
}
