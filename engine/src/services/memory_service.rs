//! Memory subsystem — thin wrapper over source ingest with memory metadata (SPEC §5.4).
//!
//! Memories are sources with source_type='observation' and memory-specific metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::graph::{AgeGraphRepository, GraphRepository as _};
use crate::models::EdgeType;

#[derive(Debug, Deserialize)]
pub struct StoreMemoryRequest {
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_importance")]
    pub importance: f64,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub supersedes_id: Option<Uuid>,
}

fn default_importance() -> f64 {
    0.5
}

#[derive(Debug, Deserialize)]
pub struct RecallRequest {
    pub query: String,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// Only return memories created at or after this timestamp.
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    /// Only return memories whose `context` metadata field starts with this prefix.
    ///
    /// Matched via `LIKE '<prefix>%'`, so `"session:main"` matches both
    /// `"session:main"` and `"session:main:2026-03-02"`.
    #[serde(default)]
    pub context_prefix: Option<String>,
}

fn default_recall_limit() -> usize {
    5
}

#[derive(Debug, Serialize)]
pub struct Memory {
    pub id: Uuid,
    pub content: String,
    pub tags: serde_json::Value,
    pub importance: f64,
    pub context: Option<String>,
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
    pub forgotten: bool,
}

pub struct MemoryService {
    pool: PgPool,
    graph: AgeGraphRepository,
}

impl MemoryService {
    pub fn new(pool: PgPool) -> Self {
        let graph = AgeGraphRepository::new(pool.clone(), "covalence");
        Self { pool, graph }
    }

    /// Store a memory (source_type='observation' with memory metadata).
    pub async fn store(&self, req: StoreMemoryRequest) -> Result<Memory, sqlx::Error> {
        let id = Uuid::new_v4();
        let metadata = serde_json::json!({
            "memory": true,
            "tags": req.tags,
            "importance": req.importance,
            "context": req.context,
            "forgotten": false,
        });

        let content_hash = {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(req.content.as_bytes());
            hex::encode(hash)
        };

        let size_tokens = req.content.split_whitespace().count() as i32;
        let reliability = 0.4 + (req.importance * 0.4); // observations: 0.4 base + importance boost

        // Insert as source node
        sqlx::query(
            "INSERT INTO covalence.nodes (id, node_type, source_type, title, content, content_hash, fingerprint,
                 size_tokens, reliability, metadata, status, confidence)
             VALUES ($1, 'source', 'observation', 'memory', $2, $3, $3, $4, $5, $6, 'active', $5)"
        )
        .bind(id)
        .bind(&req.content)
        .bind(&content_hash)
        .bind(size_tokens)
        .bind(reliability)
        .bind(&metadata)
        .execute(&self.pool)
        .await?;

        // Handle supersession
        if let Some(old_id) = req.supersedes_id {
            // Mark old memory as forgotten
            sqlx::query(
                "UPDATE covalence.nodes SET metadata = jsonb_set(metadata, '{forgotten}', 'true')
                 WHERE id = $1",
            )
            .bind(old_id)
            .execute(&self.pool)
            .await?;

            // Create SUPERSEDES edge via GraphRepository (dual-writes AGE + SQL).
            if let Err(e) = self
                .graph
                .create_edge(
                    id,
                    old_id,
                    EdgeType::Supersedes,
                    1.0,
                    "system",
                    serde_json::json!({}),
                )
                .await
            {
                tracing::warn!(
                    new_id = %id,
                    old_id = %old_id,
                    "memory store: failed to create SUPERSEDES edge via GraphRepository: {e}"
                );
            }
        }

        // Queue embedding
        sqlx::query(
            "INSERT INTO covalence.slow_path_queue (id, task_type, node_id, priority, status)
             VALUES ($1, 'embed', $2, 3, 'pending')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(Memory {
            id,
            content: req.content,
            tags: serde_json::json!(req.tags),
            importance: req.importance,
            context: req.context,
            confidence: reliability,
            created_at: Utc::now(),
            forgotten: false,
        })
    }

    /// Search memories via ts_rank (embedding search added when slow-path generates embeddings).
    ///
    /// Supports optional filters:
    /// - `min_confidence`: minimum confidence threshold (default 0.0)
    /// - `tags`: only return memories whose `tags` metadata array contains all listed tags
    /// - `since`: only return memories created at or after the given timestamp
    /// - `context_prefix`: only return memories whose `context` metadata field starts with the prefix
    pub async fn recall(&self, req: RecallRequest) -> Result<Vec<Memory>, sqlx::Error> {
        let min_conf = req.min_confidence.unwrap_or(0.0);

        // Fixed parameters: $1 = min_conf, $2 = query text, $3 = limit.
        // Optional parameters follow in the order: tags, since, context_prefix.
        let mut next_param: usize = 3;
        let mut extra_clauses: Vec<String> = Vec::new();

        let tags_json = if !req.tags.is_empty() {
            next_param += 1;
            extra_clauses.push(format!("metadata->'tags' @> ${next_param}"));
            Some(serde_json::json!(req.tags))
        } else {
            None
        };

        if req.since.is_some() {
            next_param += 1;
            extra_clauses.push(format!("created_at >= ${next_param}"));
        }

        if req.context_prefix.is_some() {
            next_param += 1;
            extra_clauses.push(format!("metadata->>'context' LIKE ${next_param}"));
        }

        let extra_sql = if extra_clauses.is_empty() {
            String::new()
        } else {
            format!("AND {}", extra_clauses.join(" AND "))
        };

        let sql = format!(
            "SELECT id, content, metadata, COALESCE(confidence, 0.5)::float8, created_at,
                    metadata->>'context'
             FROM covalence.nodes
             WHERE node_type = 'source' AND source_type = 'observation'
               AND (metadata->>'memory')::boolean = true
               AND COALESCE((metadata->>'forgotten')::boolean, false) = false
               AND status = 'active'
               AND COALESCE(confidence, 0.5) >= $1
               AND content_tsv @@ websearch_to_tsquery('english', $2)
               {extra_sql}
             ORDER BY ts_rank(content_tsv, websearch_to_tsquery('english', $2)) DESC
             LIMIT $3"
        );

        let mut q = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                serde_json::Value,
                f64,
                DateTime<Utc>,
                Option<String>,
            ),
        >(&sql)
        .bind(min_conf)
        .bind(&req.query)
        .bind(req.limit as i64);

        if let Some(ref tags) = tags_json {
            q = q.bind(tags);
        }
        if let Some(since) = req.since {
            q = q.bind(since);
        }
        if let Some(ref prefix) = req.context_prefix {
            q = q.bind(format!("{}%", prefix));
        }

        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|(id, content, metadata, confidence, created_at, context)| {
                let tags = metadata
                    .get("tags")
                    .cloned()
                    .unwrap_or(serde_json::json!([]));
                let importance = metadata
                    .get("importance")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5);
                let forgotten = metadata
                    .get("forgotten")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Memory {
                    id,
                    content,
                    tags,
                    importance,
                    context,
                    confidence,
                    created_at,
                    forgotten,
                }
            })
            .collect())
    }

    /// Soft-delete a memory (mark as forgotten).
    pub async fn forget(&self, id: Uuid, reason: Option<String>) -> Result<(), sqlx::Error> {
        let meta_update = if let Some(r) = reason {
            serde_json::json!({"forgotten": true, "forget_reason": r})
        } else {
            serde_json::json!({"forgotten": true})
        };
        sqlx::query(
            "UPDATE covalence.nodes SET metadata = metadata || $2
             WHERE id = $1 AND node_type = 'source' AND source_type = 'observation'",
        )
        .bind(id)
        .bind(&meta_update)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Stats about the memory system.
    pub async fn status(&self) -> Result<serde_json::Value, sqlx::Error> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM covalence.nodes
             WHERE node_type = 'source' AND source_type = 'observation'
               AND (metadata->>'memory')::boolean = true AND status = 'active'",
        )
        .fetch_one(&self.pool)
        .await?;

        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM covalence.nodes
             WHERE node_type = 'source' AND source_type = 'observation'
               AND (metadata->>'memory')::boolean = true
               AND COALESCE((metadata->>'forgotten')::boolean, false) = false
               AND status = 'active'",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(serde_json::json!({
            "total_memories": count,
            "active_memories": active,
            "forgotten_memories": count - active,
        }))
    }
}
