//! Agent memory model — supplementary context for observation sources.
//!
//! Each `AgentMemory` record is linked 1:1 with a `Source` via
//! `source_id`. The source holds the content and embedding; this
//! record holds agent-specific metadata (agent_id, topic, access
//! tracking, expiry).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An agent memory record that supplements a source with agent context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    /// Unique memory identifier.
    pub id: Uuid,
    /// The source that holds the memory content and embedding.
    pub source_id: Uuid,
    /// Agent that created this memory (optional for shared memories).
    pub agent_id: Option<String>,
    /// Topic or category for organisation and filtering.
    pub topic: Option<String>,
    /// External task or session identifier.
    pub task_id: Option<String>,
    /// Confidence in this memory (0.0 to 1.0).
    pub confidence: f64,
    /// Number of times this memory has been recalled.
    pub access_count: i32,
    /// When this memory was last accessed via recall.
    pub last_accessed: Option<DateTime<Utc>>,
    /// When this memory becomes a candidate for forgetting.
    pub expires_at: Option<DateTime<Utc>>,
    /// When this memory was created.
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_memory_round_trip() {
        let now = Utc::now();
        let mem = AgentMemory {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            agent_id: Some("agent-1".to_string()),
            topic: Some("rust patterns".to_string()),
            task_id: None,
            confidence: 0.9,
            access_count: 3,
            last_accessed: Some(now),
            expires_at: None,
            created_at: now,
        };
        let json = serde_json::to_string(&mem).unwrap();
        let parsed: AgentMemory = serde_json::from_str(&json).unwrap();
        assert_eq!(mem.id, parsed.id);
        assert_eq!(mem.source_id, parsed.source_id);
        assert_eq!(mem.agent_id, parsed.agent_id);
        assert_eq!(mem.topic, parsed.topic);
        assert_eq!(mem.confidence, parsed.confidence);
        assert_eq!(mem.access_count, parsed.access_count);
    }

    #[test]
    fn agent_memory_defaults() {
        let now = Utc::now();
        let mem = AgentMemory {
            id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            agent_id: None,
            topic: None,
            task_id: None,
            confidence: 0.5,
            access_count: 0,
            last_accessed: None,
            expires_at: None,
            created_at: now,
        };
        assert!(mem.agent_id.is_none());
        assert!(mem.topic.is_none());
        assert!(mem.task_id.is_none());
        assert_eq!(mem.access_count, 0);
        assert!(mem.last_accessed.is_none());
        assert!(mem.expires_at.is_none());
    }
}
