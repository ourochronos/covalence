//! Session and turn models for lightweight conversation context.
//!
//! Sessions group turns into a conversation. Turns are individual
//! messages (user, assistant, system, tool) ordered by an auto-
//! incrementing ordinal. No embedding pipeline or entity extraction
//! is applied — sessions are ephemeral conversation context for
//! `/ask` synthesis.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A conversation session grouping related turns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session identifier.
    pub id: Uuid,
    /// Optional human-readable name for the session.
    pub name: Option<String>,
    /// Arbitrary key-value metadata.
    pub metadata: serde_json::Value,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
}

/// A single message turn within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// Unique turn identifier.
    pub id: Uuid,
    /// The session this turn belongs to.
    pub session_id: Uuid,
    /// Role of the turn author: "user", "assistant", "system",
    /// or "tool".
    pub role: String,
    /// The message content.
    pub content: String,
    /// Arbitrary key-value metadata (e.g. model used, latency).
    pub metadata: serde_json::Value,
    /// Ordinal position within the session (1-based, auto-assigned).
    pub ordinal: i32,
    /// When this turn was created.
    pub created_at: DateTime<Utc>,
}
