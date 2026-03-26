//! Session service — lightweight conversation context management.
//!
//! Sessions group turns into multi-turn conversations without
//! triggering the embedding/extraction pipeline. Used by the ask
//! service to maintain conversational context across `/ask` calls.

use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::error::Result;
use crate::models::session::{Session, Turn};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::SessionRepo;

/// Service for managing conversation sessions and turns.
pub struct SessionService {
    repo: Arc<PgRepo>,
}

impl SessionService {
    /// Create a new session service backed by the given repository.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self { repo }
    }

    /// Create a new session with an optional name and metadata.
    pub async fn create_session(
        &self,
        name: Option<&str>,
        metadata: Option<Value>,
    ) -> Result<Session> {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            name: name.map(|s| s.to_string()),
            metadata: metadata.unwrap_or(Value::Object(Default::default())),
            created_at: now,
            updated_at: now,
        };
        self.repo.create_session(&session).await?;
        Ok(session)
    }

    /// Add a turn to an existing session. The ordinal is auto-
    /// computed by the storage layer.
    pub async fn add_turn(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
        metadata: Option<Value>,
    ) -> Result<Turn> {
        let turn = Turn {
            id: Uuid::new_v4(),
            session_id,
            role: role.to_string(),
            content: content.to_string(),
            metadata: metadata.unwrap_or(Value::Object(Default::default())),
            ordinal: 0, // auto-computed by DB
            created_at: Utc::now(),
        };
        let created = self.repo.add_turn(&turn).await?;
        Ok(created)
    }

    /// Retrieve the most recent turns for a session in
    /// chronological order. Defaults to 50 if `last_n` is None.
    pub async fn get_history(&self, session_id: Uuid, last_n: Option<i64>) -> Result<Vec<Turn>> {
        let n = last_n.unwrap_or(50);
        self.repo.get_history(session_id, n).await
    }

    /// Delete a session and all its turns. Returns `true` if the
    /// session existed.
    pub async fn close_session(&self, session_id: Uuid) -> Result<bool> {
        self.repo.close_session(session_id).await
    }

    /// List sessions ordered by most recently updated first.
    pub async fn list_sessions(&self, limit: i64, offset: i64) -> Result<Vec<Session>> {
        self.repo.list_sessions(limit, offset).await
    }

    /// Get a single session by ID.
    pub async fn get_session(&self, session_id: Uuid) -> Result<Option<Session>> {
        self.repo.get_session(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_service_requires_repo() {
        // Verify SessionService is Send + Sync.
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<SessionService>();
    }

    #[test]
    fn session_model_round_trip() {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            name: Some("test session".to_string()),
            metadata: serde_json::json!({"key": "value"}),
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session.id, parsed.id);
        assert_eq!(session.name, parsed.name);
    }

    #[test]
    fn turn_model_round_trip() {
        let now = Utc::now();
        let turn = Turn {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            role: "user".to_string(),
            content: "Hello world".to_string(),
            metadata: serde_json::json!({}),
            ordinal: 1,
            created_at: now,
        };
        let json = serde_json::to_string(&turn).unwrap();
        let parsed: Turn = serde_json::from_str(&json).unwrap();
        assert_eq!(turn.id, parsed.id);
        assert_eq!(turn.role, parsed.role);
        assert_eq!(turn.content, parsed.content);
        assert_eq!(turn.ordinal, parsed.ordinal);
    }

    #[test]
    fn turn_roles_are_valid_strings() {
        for role in &["user", "assistant", "system", "tool"] {
            let turn = Turn {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                role: role.to_string(),
                content: "test".to_string(),
                metadata: serde_json::json!({}),
                ordinal: 1,
                created_at: Utc::now(),
            };
            assert_eq!(turn.role, *role);
        }
    }

    #[test]
    fn session_default_metadata_is_empty_object() {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            name: None,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        };
        assert!(session.metadata.is_object());
        assert!(session.metadata.as_object().unwrap().is_empty());
    }
}
