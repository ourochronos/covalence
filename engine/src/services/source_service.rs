//! Source lifecycle — ingest, get, list, delete (SPEC §5.4, §8.1).

use chrono::Utc;
use sha2::{Sha256, Digest};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::errors::*;
use crate::graph::{AgeGraphRepository, GraphRepository};
use crate::models::*;

/// Request to ingest a new source.
#[derive(Debug, serde::Deserialize)]
pub struct IngestRequest {
    pub content: String,
    pub source_type: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub session_id: Option<Uuid>,
    pub reliability: Option<f32>,
}

/// Paginated list params.
#[derive(Debug, serde::Deserialize, Default)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<Uuid>,
    pub source_type: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
}

/// Response envelope for a source.
#[derive(Debug, serde::Serialize)]
pub struct SourceResponse {
    pub id: Uuid,
    pub node_type: String,
    pub title: Option<String>,
    pub content: String,
    pub source_type: Option<String>,
    pub status: String,
    pub confidence: f32,
    pub reliability: f32,
    pub fingerprint: String,
    pub metadata: serde_json::Value,
    pub version: i32,
    pub created_at: chrono::DateTime<Utc>,
    pub modified_at: chrono::DateTime<Utc>,
}

pub struct SourceService {
    pool: PgPool,
    graph: AgeGraphRepository,
}

impl SourceService {
    pub fn new(pool: PgPool) -> Self {
        let graph = AgeGraphRepository::new(pool.clone(), "covalence");
        Self { pool, graph }
    }

    /// Ingest a source — idempotent via SHA-256 fingerprint (SPEC §8.1 fast path).
    pub async fn ingest(&self, req: IngestRequest) -> AppResult<SourceResponse> {
        // 1. Compute fingerprint
        let fingerprint = hex::encode(Sha256::digest(req.content.as_bytes()));

        // 2. Check for existing source with same fingerprint (idempotent)
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM covalence.nodes WHERE fingerprint = $1 AND node_type = 'source'"
        )
            .bind(&fingerprint)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(id) = existing {
            return self.get(id).await;
        }

        // 3. Create node
        let id = Uuid::new_v4();
        let now = Utc::now();
        let reliability = req.reliability.unwrap_or(default_reliability(req.source_type.as_deref()));
        let metadata = req.metadata.unwrap_or(serde_json::json!({}));
        let source_type = req.source_type.unwrap_or_else(|| "document".into());
        let content_hash = hex::encode(Sha256::digest(req.content.as_bytes()));
        let size_tokens = (req.content.split_whitespace().count() as f64 / 0.75) as i32;

        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, title, content, status, source_type, reliability, \
              fingerprint, content_hash, size_tokens, metadata, \
              confidence_overall, confidence_source, \
              created_at, modified_at, accessed_at) \
             VALUES ($1, 'source', $2, $3, 'active', $4, $5, $6, $7, $8, $9, $10, $10, $11, $11, $11)"
        )
            .bind(id)
            .bind(&req.title)
            .bind(&req.content)
            .bind(&source_type)
            .bind(reliability as f64)
            .bind(&fingerprint)
            .bind(&content_hash)
            .bind(size_tokens)
            .bind(&metadata)
            .bind(reliability as f64)
            .bind(now)
            .execute(&self.pool)
            .await?;

        // 4. Create AGE vertex
        if let Err(e) = self.graph.create_vertex(id, NodeType::Source, serde_json::json!({})).await {
            tracing::warn!(error = %e, source_id = %id, "failed to create AGE vertex for source");
        }

        // 5. Create CAPTURED_IN edge if session provided
        if let Some(session_id) = req.session_id {
            if let Err(e) = self.graph.create_edge(
                id, session_id, EdgeType::CapturedIn, 1.0, "algorithmic", serde_json::json!({}),
            ).await {
                tracing::warn!(error = %e, "failed to create CAPTURED_IN edge");
            }
        }

        // 6. Enqueue slow-path tasks
        self.enqueue_slow_path(id, &source_type).await?;

        self.get(id).await
    }

    /// Get a source by ID.
    pub async fn get(&self, id: Uuid) -> AppResult<SourceResponse> {
        let row = sqlx::query(
            "SELECT id, title, content, source_type, status, \
             confidence_overall, reliability, fingerprint, metadata, version, \
             created_at, modified_at \
             FROM covalence.nodes WHERE id = $1 AND node_type = 'source'"
        )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("source {id}")))?;

        Ok(source_from_row(&row))
    }

    /// List sources with optional filters.
    pub async fn list(&self, params: ListParams) -> AppResult<Vec<SourceResponse>> {
        let limit = params.limit.unwrap_or(20).min(100);
        let mut sql = String::from(
            "SELECT id, title, content, source_type, status, \
             confidence_overall, reliability, fingerprint, metadata, version, \
             created_at, modified_at \
             FROM covalence.nodes WHERE node_type = 'source'"
        );

        if let Some(ref st) = params.source_type {
            sql.push_str(&format!(" AND source_type = '{st}'"));
        }
        if let Some(ref status) = params.status {
            sql.push_str(&format!(" AND status = '{status}'"));
        }
        if let Some(ref q) = params.q {
            sql.push_str(&format!(
                " AND content_tsv @@ websearch_to_tsquery('english', '{}')",
                q.replace('\'', "''")
            ));
        }
        if let Some(cursor) = params.cursor {
            sql.push_str(&format!(" AND id > '{cursor}'"));
        }
        sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {limit}"));

        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        Ok(rows.iter().map(source_from_row).collect())
    }

    /// Delete a source (hard delete with cascade).
    pub async fn delete(&self, id: Uuid) -> AppResult<()> {
        // Delete AGE vertex (cascades edges)
        if let Err(e) = self.graph.delete_vertex(id).await {
            tracing::warn!(error = %e, "failed to delete AGE vertex for source {id}");
        }

        // Delete embedding
        sqlx::query("DELETE FROM covalence.node_embeddings WHERE node_id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;

        // Delete node
        let result = sqlx::query("DELETE FROM covalence.nodes WHERE id = $1 AND node_type = 'source'")
            .bind(id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!("source {id}")));
        }

        Ok(())
    }

    /// Enqueue slow-path tasks for a new source.
    async fn enqueue_slow_path(&self, source_id: Uuid, _source_type: &str) -> AppResult<()> {
        // Queue embedding generation
        sqlx::query(
            "INSERT INTO covalence.slow_path_queue (id, task_type, node_id, priority, status) \
             VALUES ($1, 'embed', $2, 3, 'pending')"
        )
            .bind(Uuid::new_v4())
            .bind(source_id)
            .execute(&self.pool)
            .await?;

        // Queue contention check
        sqlx::query(
            "INSERT INTO covalence.slow_path_queue (id, task_type, node_id, priority, status) \
             VALUES ($1, 'contention_check', $2, 5, 'pending')"
        )
            .bind(Uuid::new_v4())
            .bind(source_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

fn source_from_row(row: &PgRow) -> SourceResponse {
    SourceResponse {
        id: row.get("id"),
        node_type: "source".into(),
        title: row.get("title"),
        content: row.get("content"),
        source_type: row.get("source_type"),
        status: row.get("status"),
        confidence: row.get::<Option<f64>, _>("confidence_overall").unwrap_or(0.5) as f32,
        reliability: row.get::<Option<f64>, _>("reliability").unwrap_or(0.5) as f32,
        fingerprint: row.get::<Option<String>, _>("fingerprint").unwrap_or_default(),
        metadata: row.get("metadata"),
        version: row.get::<Option<i32>, _>("version").unwrap_or(1),
        created_at: row.get("created_at"),
        modified_at: row.get("modified_at"),
    }
}

/// Default reliability by source type (matches Valence v2).
fn default_reliability(source_type: Option<&str>) -> f32 {
    match source_type {
        Some("document") | Some("code") => 0.8,
        Some("tool_output") => 0.7,
        Some("user_input") => 0.75,
        Some("web") => 0.6,
        Some("conversation") => 0.5,
        Some("observation") => 0.4,
        _ => 0.5,
    }
}
