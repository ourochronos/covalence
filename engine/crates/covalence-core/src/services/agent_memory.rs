//! Agent memory service — store, recall, consolidate, and forget.
//!
//! Orchestrates the memory lifecycle on top of the source service
//! (for content + embeddings) and the agent_memories table (for
//! agent-specific metadata and access tracking).

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::Result;
use crate::ingestion::ChatBackend;
use crate::models::agent_memory::AgentMemory;
use crate::search::strategy::SearchStrategy;
use crate::services::SearchService;
use crate::services::search::SearchFilters;
use crate::services::session::SessionService;
use crate::services::source::SourceService;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::AgentMemoryRepo;

// ── Request / Response DTOs ─────────────────────────────────────

/// Request to store a memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreRequest {
    /// The content to remember.
    pub content: String,
    /// Optional topic/category for organisation.
    pub topic: Option<String>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
    /// Confidence level (0.0 to 1.0, default 0.8).
    pub confidence: Option<f64>,
    /// Agent that owns this memory.
    pub agent_id: Option<String>,
    /// External task identifier.
    pub task_id: Option<String>,
}

/// Request to recall memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecallRequest {
    /// The query to search for.
    pub query: String,
    /// Maximum number of memories to return.
    pub limit: Option<usize>,
    /// Optional topic filter.
    pub topic: Option<String>,
    /// Minimum confidence threshold.
    pub min_confidence: Option<f64>,
    /// Filter to a specific agent's memories.
    pub agent_id: Option<String>,
}

/// A recalled memory item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Memory identifier (source ID internally).
    pub id: String,
    /// The remembered content.
    pub content: String,
    /// Topic if provided.
    pub topic: Option<String>,
    /// Relevance score from search.
    pub relevance: f64,
    /// Confidence level.
    pub confidence: f64,
    /// When this memory was stored.
    pub stored_at: String,
    /// Agent that owns this memory.
    pub agent_id: Option<String>,
    /// Number of times this memory has been recalled.
    pub access_count: Option<i32>,
    /// When this memory was last accessed.
    pub last_accessed: Option<String>,
}

/// Response from memory store operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreResponse {
    /// ID of the stored memory.
    pub id: String,
    /// Number of entities extracted (if any).
    pub entities_extracted: usize,
    /// Status message.
    pub status: String,
}

/// Memory status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatus {
    /// Total number of memories stored.
    pub total_memories: i64,
    /// Memories for a specific agent (if agent_id provided).
    pub agent_memories: Option<i64>,
}

/// Request to consolidate memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidateRequest {
    /// Agent whose memories to consolidate.
    pub agent_id: String,
    /// Similarity threshold (default from config: 0.85).
    pub threshold: Option<f64>,
}

/// Response from consolidation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidateResponse {
    /// Number of memory groups found.
    pub groups_found: usize,
    /// Number of new merged memories created.
    pub merged: usize,
    /// Number of originals marked for expiry.
    pub expired: usize,
    /// Status message.
    pub status: String,
}

/// Request to apply forgetting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetOldRequest {
    /// Override retention days (default from config: 90).
    pub retention_days: Option<i64>,
}

/// Response from forgetting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetOldResponse {
    /// Number of memories deleted.
    pub deleted: usize,
    /// Status message.
    pub status: String,
}

/// Response from reflection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectResponse {
    /// Number of learnings extracted.
    pub learnings_stored: usize,
    /// Status message.
    pub status: String,
}

// ── Service ─────────────────────────────────────────────────────

/// Service for agent memory lifecycle: store, recall, consolidate,
/// reflect, and forget.
pub struct AgentMemoryService {
    repo: Arc<PgRepo>,
    source_service: Arc<SourceService>,
    search_service: Arc<SearchService>,
    session_service: Arc<SessionService>,
    chat_backend: Option<Arc<dyn ChatBackend>>,
}

impl AgentMemoryService {
    /// Create a new agent memory service.
    pub fn new(
        repo: Arc<PgRepo>,
        source_service: Arc<SourceService>,
        search_service: Arc<SearchService>,
        session_service: Arc<SessionService>,
    ) -> Self {
        Self {
            repo,
            source_service,
            search_service,
            session_service,
            chat_backend: None,
        }
    }

    /// Wire an optional LLM backend for consolidation and reflection.
    pub fn with_chat_backend(mut self, backend: Option<Arc<dyn ChatBackend>>) -> Self {
        self.chat_backend = backend;
        self
    }

    /// Store a new memory.
    ///
    /// Creates a source with `source_type = "observation"`, then
    /// inserts a corresponding `agent_memories` row with the agent
    /// context metadata.
    pub async fn store(&self, req: MemoryStoreRequest) -> Result<MemoryStoreResponse> {
        let confidence = req.confidence.unwrap_or(0.8);

        // Build metadata JSON for the source.
        let mut meta = req.metadata.unwrap_or_else(|| json!({}));
        if let Some(obj) = meta.as_object_mut() {
            if let Some(ref topic) = req.topic {
                obj.insert("topic".to_string(), json!(topic));
            }
            if let Some(ref agent_id) = req.agent_id {
                obj.insert("agent_id".to_string(), json!(agent_id));
            }
            if let Some(ref task_id) = req.task_id {
                obj.insert("task_id".to_string(), json!(task_id));
            }
        }

        // Ingest content as an observation source.
        let source_id = self
            .source_service
            .ingest(
                req.content.as_bytes(),
                "observation",
                "text/plain",
                None,
                meta,
            )
            .await?;

        // Create the agent_memories record.
        let now = Utc::now();
        let memory = AgentMemory {
            id: Uuid::new_v4(),
            source_id: source_id.into_uuid(),
            agent_id: req.agent_id,
            topic: req.topic,
            task_id: req.task_id,
            confidence,
            access_count: 0,
            last_accessed: None,
            expires_at: None,
            created_at: now,
        };
        AgentMemoryRepo::create(&*self.repo, &memory).await?;

        Ok(MemoryStoreResponse {
            id: source_id.into_uuid().to_string(),
            entities_extracted: 0,
            status: format!("Memory stored ({} chars)", req.content.len()),
        })
    }

    /// Recall memories by semantic search within the memory domain.
    ///
    /// Results are enriched with agent_memories metadata and access
    /// counts are incremented for each recalled memory.
    pub async fn recall(&self, req: MemoryRecallRequest) -> Result<Vec<MemoryItem>> {
        let limit = req.limit.unwrap_or(10).min(200);

        let filters = Some(SearchFilters {
            min_confidence: req.min_confidence,
            node_types: None,
            entity_classes: None,
            date_range: None,
            source_types: Some(vec!["observation".to_string()]),
            domains: Some(vec!["memory".to_string()]),
            graph_view: None,
        });

        let results = self
            .search_service
            .search(&req.query, SearchStrategy::Auto, limit, filters)
            .await?;

        let mut items = Vec::with_capacity(results.len());
        for r in results {
            let source_uuid = r.id;

            // Look up agent_memories metadata.
            let am = AgentMemoryRepo::get_by_source(&*self.repo, source_uuid).await?;

            // Apply agent_id filter if requested.
            if let Some(ref filter_agent) = req.agent_id {
                if let Some(ref am) = am {
                    if am.agent_id.as_deref() != Some(filter_agent) {
                        continue;
                    }
                } else {
                    continue;
                }
            }

            // Fetch the source's actual body. The search snippet is a
            // preview only — for node-level matches it can be empty,
            // which previously caused the recall API to return memories
            // with `content: ""`. Falling back to the snippet keeps
            // recall non-empty even if the source row is unexpectedly
            // missing raw_content.
            let source = self.source_service.get(source_uuid.into()).await?;
            let content = source
                .as_ref()
                .and_then(|s| s.raw_content.clone())
                .or(r.snippet)
                .unwrap_or_default();

            // Increment access count (fire-and-forget style).
            let _ = AgentMemoryRepo::increment_access(&*self.repo, source_uuid).await;

            items.push(MemoryItem {
                id: source_uuid.to_string(),
                content,
                topic: am.as_ref().and_then(|m| m.topic.clone()),
                relevance: r.fused_score,
                confidence: am
                    .as_ref()
                    .map(|m| m.confidence)
                    .unwrap_or(r.confidence.unwrap_or(1.0)),
                stored_at: r.created_at.unwrap_or_default(),
                agent_id: am.as_ref().and_then(|m| m.agent_id.clone()),
                access_count: am.as_ref().map(|m| m.access_count),
                last_accessed: am
                    .as_ref()
                    .and_then(|m| m.last_accessed.map(|t| t.to_rfc3339())),
            });
        }

        Ok(items)
    }

    /// Forget a specific memory by its agent_memory ID.
    ///
    /// Deletes the source (which CASCADE-deletes the agent_memories
    /// row).
    pub async fn forget(&self, memory_id: Uuid) -> Result<bool> {
        // Look up the agent_memory to get the source_id.
        let am = AgentMemoryRepo::get(&*self.repo, memory_id).await?;
        let Some(am) = am else {
            return Ok(false);
        };
        let result = self.source_service.delete(am.source_id.into()).await?;
        Ok(result.deleted)
    }

    /// Consolidate similar memories for an agent.
    ///
    /// Searches within the memory domain for the agent's memories,
    /// groups them by vector similarity, and (if a chat backend is
    /// available) merges each group into a single consolidated memory.
    pub async fn consolidate(&self, req: ConsolidateRequest) -> Result<ConsolidateResponse> {
        let threshold = req.threshold.unwrap_or(0.85);

        // Fetch all memories for this agent.
        let memories = AgentMemoryRepo::list_by_agent(&*self.repo, &req.agent_id, 500).await?;

        if memories.len() < 2 {
            return Ok(ConsolidateResponse {
                groups_found: 0,
                merged: 0,
                expired: 0,
                status: "Not enough memories to consolidate".to_string(),
            });
        }

        // Without a chat backend we cannot synthesise merged memories.
        let Some(ref backend) = self.chat_backend else {
            return Ok(ConsolidateResponse {
                groups_found: 0,
                merged: 0,
                expired: 0,
                status: "No chat backend available for consolidation".to_string(),
            });
        };

        // Simple greedy grouping: for each memory, search for similar
        // memories and group them. We track which memories have already
        // been grouped to avoid duplicates.
        let mut grouped: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut groups: Vec<Vec<AgentMemory>> = Vec::new();

        for mem in &memories {
            if grouped.contains(&mem.id) {
                continue;
            }

            // Search for similar memories using this memory's content.
            let filters = Some(SearchFilters {
                min_confidence: None,
                node_types: None,
                entity_classes: None,
                date_range: None,
                source_types: Some(vec!["observation".to_string()]),
                domains: Some(vec!["memory".to_string()]),
                graph_view: None,
            });

            // Look up the source content for the search query.
            let source = self.source_service.get(mem.source_id.into()).await?;
            let query: String = source
                .as_ref()
                .and_then(|s| s.raw_content.clone())
                .unwrap_or_default();

            if query.is_empty() {
                continue;
            }

            let results = self
                .search_service
                .search(&query, SearchStrategy::Precise, 20, filters)
                .await?;

            let mut group = vec![mem.clone()];
            grouped.insert(mem.id);

            for r in &results {
                if r.fused_score < threshold {
                    continue;
                }
                // Find the matching memory from our list.
                if let Some(other) = memories
                    .iter()
                    .find(|m| m.source_id == r.id && !grouped.contains(&m.id))
                {
                    group.push(other.clone());
                    grouped.insert(other.id);
                }
            }

            if group.len() >= 2 {
                groups.push(group);
            }
        }

        let groups_found = groups.len();
        let mut merged = 0;
        let mut expired = 0;

        for group in &groups {
            // Collect content from all memories in the group.
            let mut contents = Vec::new();
            for mem in group {
                let source = self.source_service.get(mem.source_id.into()).await?;
                if let Some(s) = source {
                    if let Some(ref c) = s.raw_content {
                        contents.push(c.clone());
                    }
                }
            }

            if contents.is_empty() {
                continue;
            }

            // Synthesise a merged memory via LLM.
            let prompt = format!(
                "Merge these related memories into a single concise \
                 statement that preserves all important information. \
                 Return ONLY the merged text, no explanation.\n\n{}",
                contents
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("Memory {}: {}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            let resp = backend
                .chat(
                    "You merge related memories into concise \
                     single statements.",
                    &prompt,
                    false,
                    0.3,
                )
                .await;
            let merged_content = match resp {
                Ok(r) => r.text,
                Err(_) => continue,
            };

            // Store the merged memory.
            let first = &group[0];
            let store_req = MemoryStoreRequest {
                content: merged_content,
                topic: first.topic.clone(),
                metadata: None,
                confidence: Some(group.iter().map(|m| m.confidence).fold(0.0f64, f64::max)),
                agent_id: first.agent_id.clone(),
                task_id: None,
            };
            if self.store(store_req).await.is_ok() {
                merged += 1;
            }

            // Mark originals as expired.
            let expiry = Utc::now();
            for mem in group {
                let _ = sqlx::query(
                    "UPDATE agent_memories SET expires_at = $1 \
                     WHERE id = $2",
                )
                .bind(expiry)
                .bind(mem.id)
                .execute(self.repo.pool())
                .await;
                expired += 1;
            }
        }

        Ok(ConsolidateResponse {
            groups_found,
            merged,
            expired,
            status: format!(
                "Consolidated {} groups, {} merged, {} expired",
                groups_found, merged, expired
            ),
        })
    }

    /// Reflect on a session and extract learnings as memories.
    ///
    /// Loads the session's turns, sends them to the LLM to extract
    /// key learnings, and stores each as a separate memory.
    pub async fn reflect(
        &self,
        session_id: Uuid,
        agent_id: Option<String>,
    ) -> Result<ReflectResponse> {
        let Some(ref backend) = self.chat_backend else {
            return Err(crate::error::Error::Config(
                "No chat backend available for reflection".to_string(),
            ));
        };

        let turns = self
            .session_service
            .get_history(session_id, Some(100))
            .await?;

        if turns.is_empty() {
            return Ok(ReflectResponse {
                learnings_stored: 0,
                status: "No turns found in session".to_string(),
            });
        }

        // Build a transcript.
        let transcript: String = turns
            .iter()
            .map(|t| format!("{}: {}", t.role, t.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt = format!(
            "Extract the key learnings, decisions, and facts from \
             this conversation. Return each as a separate line. \
             Do not number them or add bullet points. Each line \
             should be a self-contained statement.\n\n{}",
            transcript
        );

        let resp = backend
            .chat(
                "You extract key learnings from conversations. \
                 Return each learning on its own line.",
                &prompt,
                false,
                0.3,
            )
            .await;
        let content = match resp {
            Ok(r) => r.text,
            Err(e) => {
                return Err(crate::error::Error::Ingestion(format!(
                    "reflection LLM call failed: {e}"
                )));
            }
        };

        // Split by lines and store each non-empty line as a memory.
        let lines: Vec<&str> = content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();

        let mut stored = 0;
        for line in &lines {
            let store_req = MemoryStoreRequest {
                content: line.to_string(),
                topic: Some("reflection".to_string()),
                metadata: Some(json!({
                    "session_id": session_id.to_string(),
                    "source": "reflection"
                })),
                confidence: Some(0.7),
                agent_id: agent_id.clone(),
                task_id: None,
            };
            if self.store(store_req).await.is_ok() {
                stored += 1;
            }
        }

        Ok(ReflectResponse {
            learnings_stored: stored,
            status: format!("Extracted {} learnings from session", stored),
        })
    }

    /// Apply forgetting — delete memories past their retention period.
    pub async fn apply_forgetting(&self, retention_days: i64) -> Result<ForgetOldResponse> {
        let expired = AgentMemoryRepo::find_expired(&*self.repo, retention_days).await?;

        let mut deleted = 0;
        for mem in &expired {
            if self.forget(mem.id).await.unwrap_or(false) {
                deleted += 1;
            }
        }

        Ok(ForgetOldResponse {
            deleted,
            status: format!(
                "Deleted {} expired memories (retention: {} days)",
                deleted, retention_days
            ),
        })
    }

    /// Get memory status, optionally scoped to an agent.
    pub async fn status(&self, agent_id: Option<&str>) -> Result<MemoryStatus> {
        let total = AgentMemoryRepo::count_all(&*self.repo).await?;
        let agent_count = match agent_id {
            Some(aid) => Some(AgentMemoryRepo::count_by_agent(&*self.repo, aid).await?),
            None => None,
        };

        Ok(MemoryStatus {
            total_memories: total,
            agent_memories: agent_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_memory_service_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<AgentMemoryService>();
    }

    #[test]
    fn memory_store_request_round_trip() {
        let req = MemoryStoreRequest {
            content: "test content".to_string(),
            topic: Some("testing".to_string()),
            metadata: None,
            confidence: Some(0.9),
            agent_id: Some("agent-1".to_string()),
            task_id: Some("task-42".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: MemoryStoreRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.content, parsed.content);
        assert_eq!(req.topic, parsed.topic);
        assert_eq!(req.confidence, parsed.confidence);
        assert_eq!(req.agent_id, parsed.agent_id);
        assert_eq!(req.task_id, parsed.task_id);
    }

    #[test]
    fn memory_recall_request_round_trip() {
        let req = MemoryRecallRequest {
            query: "rust patterns".to_string(),
            limit: Some(5),
            topic: None,
            min_confidence: Some(0.5),
            agent_id: Some("agent-1".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: MemoryRecallRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.query, parsed.query);
        assert_eq!(req.limit, parsed.limit);
        assert_eq!(req.agent_id, parsed.agent_id);
    }

    #[test]
    fn memory_item_round_trip() {
        let item = MemoryItem {
            id: "abc-123".to_string(),
            content: "remembered thing".to_string(),
            topic: Some("testing".to_string()),
            relevance: 0.95,
            confidence: 0.8,
            stored_at: "2024-01-01T00:00:00Z".to_string(),
            agent_id: Some("agent-1".to_string()),
            access_count: Some(5),
            last_accessed: Some("2024-01-02T00:00:00Z".to_string()),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: MemoryItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item.id, parsed.id);
        assert_eq!(item.content, parsed.content);
        assert_eq!(item.agent_id, parsed.agent_id);
        assert_eq!(item.access_count, parsed.access_count);
        assert_eq!(item.last_accessed, parsed.last_accessed);
    }

    #[test]
    fn memory_store_response_round_trip() {
        let resp = MemoryStoreResponse {
            id: "abc".to_string(),
            entities_extracted: 3,
            status: "ok".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: MemoryStoreResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp.id, parsed.id);
        assert_eq!(resp.entities_extracted, parsed.entities_extracted);
    }

    #[test]
    fn memory_status_round_trip() {
        let status = MemoryStatus {
            total_memories: 42,
            agent_memories: Some(10),
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: MemoryStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status.total_memories, parsed.total_memories);
        assert_eq!(status.agent_memories, parsed.agent_memories);
    }

    #[test]
    fn consolidate_response_round_trip() {
        let resp = ConsolidateResponse {
            groups_found: 3,
            merged: 2,
            expired: 5,
            status: "done".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ConsolidateResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp.groups_found, parsed.groups_found);
        assert_eq!(resp.merged, parsed.merged);
        assert_eq!(resp.expired, parsed.expired);
    }

    #[test]
    fn forget_old_response_round_trip() {
        let resp = ForgetOldResponse {
            deleted: 7,
            status: "done".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ForgetOldResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp.deleted, parsed.deleted);
    }

    #[test]
    fn reflect_response_round_trip() {
        let resp = ReflectResponse {
            learnings_stored: 4,
            status: "done".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ReflectResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp.learnings_stored, parsed.learnings_stored);
    }
}
