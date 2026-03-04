//! Session system — tracking agent interaction context with message buffering.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::source_service::{IngestRequest, SourceService};

// ─── Session ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Session {
    pub id: Uuid,
    pub label: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
    pub platform: Option<String>,
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub label: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub platform: Option<String>,
    pub channel: Option<String>,
}

// ─── SessionMessage ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SessionMessage {
    pub id: Uuid,
    pub session_id: Uuid,
    pub speaker: Option<String>,
    pub role: String,
    pub content: String,
    pub chunk_index: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub flushed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct AppendMessagesRequest {
    pub messages: Vec<MessageItem>,
}

#[derive(Debug, Deserialize)]
pub struct MessageItem {
    pub speaker: Option<String>,
    pub role: String,
    pub content: String,
    pub chunk_index: Option<i32>,
}

// ─── Flush / Finalize ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct FlushResult {
    pub source_id: Uuid,
    pub message_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct FinalizeRequest {
    /// When true, the flushed source is queued for compilation (default false).
    #[serde(default)]
    pub compile: Option<bool>,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct SessionService {
    pool: PgPool,
}

impl SessionService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ── Core session CRUD ─────────────────────────────────────────────────────

    pub async fn create(&self, req: CreateSessionRequest) -> Result<Session, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO covalence.sessions (id, label, metadata, platform, channel)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, label, status, created_at, last_active_at, metadata, platform, channel",
        )
        .bind(id)
        .bind(&req.label)
        .bind(&req.metadata)
        .bind(&req.platform)
        .bind(&req.channel)
        .fetch_one(&self.pool)
        .await?;
        session_from_row(&row)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Session>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, label, status, created_at, last_active_at, metadata, platform, channel
             FROM covalence.sessions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(session_from_row(&r)?)),
            None => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub async fn get_by_label(&self, label: &str) -> Result<Option<Session>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, label, status, created_at, last_active_at, metadata, platform, channel
             FROM covalence.sessions WHERE label = $1",
        )
        .bind(label)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(session_from_row(&r)?)),
            None => Ok(None),
        }
    }

    pub async fn list(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Session>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, label, status, created_at, last_active_at, metadata, platform, channel
             FROM covalence.sessions
             WHERE ($1::text IS NULL OR status = $1)
             ORDER BY last_active_at DESC
             LIMIT $2",
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(session_from_row).collect()
    }

    #[allow(dead_code)]
    pub async fn touch(&self, id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE covalence.sessions SET last_active_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn close(&self, id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE covalence.sessions SET status = 'closed' WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record that a session accessed a node.
    #[allow(dead_code)]
    pub async fn record_access(&self, session_id: Uuid, node_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO covalence.session_nodes (session_id, node_id)
             VALUES ($1, $2)
             ON CONFLICT (session_id, node_id) DO UPDATE SET access_count = session_nodes.access_count + 1"
        )
        .bind(session_id)
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── Message buffering ─────────────────────────────────────────────────────

    /// Batch-insert messages into session_messages and bump last_active_at.
    pub async fn append_messages(
        &self,
        session_id: Uuid,
        req: AppendMessagesRequest,
    ) -> Result<Vec<SessionMessage>, sqlx::Error> {
        let mut inserted = Vec::with_capacity(req.messages.len());
        for item in &req.messages {
            let row = sqlx::query(
                "INSERT INTO covalence.session_messages
                     (session_id, speaker, role, content, chunk_index)
                 VALUES ($1, $2, $3, $4, $5)
                 RETURNING id, session_id, speaker, role, content, chunk_index,
                           created_at, flushed_at",
            )
            .bind(session_id)
            .bind(&item.speaker)
            .bind(&item.role)
            .bind(&item.content)
            .bind(item.chunk_index)
            .fetch_one(&self.pool)
            .await?;
            inserted.push(message_from_row(&row)?);
        }
        // Touch the session
        sqlx::query("UPDATE covalence.sessions SET last_active_at = now() WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(inserted)
    }

    /// Retrieve messages for a session.
    /// When `include_flushed` is false only unflushed messages are returned.
    pub async fn get_messages(
        &self,
        session_id: Uuid,
        include_flushed: bool,
    ) -> Result<Vec<SessionMessage>, sqlx::Error> {
        let rows = if include_flushed {
            sqlx::query(
                "SELECT id, session_id, speaker, role, content, chunk_index,
                        created_at, flushed_at
                 FROM covalence.session_messages
                 WHERE session_id = $1
                 ORDER BY created_at ASC",
            )
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, session_id, speaker, role, content, chunk_index,
                        created_at, flushed_at
                 FROM covalence.session_messages
                 WHERE session_id = $1 AND flushed_at IS NULL
                 ORDER BY created_at ASC",
            )
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter().map(message_from_row).collect()
    }

    /// Serialize unflushed messages as a transcript, ingest as a source, and
    /// mark those messages as flushed.  Returns `AppError::NotFound` when
    /// there are no unflushed messages.
    pub async fn flush(
        &self,
        session_id: Uuid,
        source_svc: &SourceService,
    ) -> Result<FlushResult, AppError> {
        // 1. Fetch unflushed messages
        let messages = self.get_messages(session_id, false).await?;
        if messages.is_empty() {
            return Err(AppError::NotFound(format!(
                "no unflushed messages for session {session_id}"
            )));
        }

        // 2. Fetch session metadata for context
        let session = self
            .get(session_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("session {session_id}")))?;

        let label = session.label.as_deref().unwrap_or("unnamed");
        let platform = session.platform.as_deref().unwrap_or("unknown");
        let channel = session.channel.as_deref().unwrap_or("unknown");

        let first_ts = messages.first().map(|m| m.created_at).unwrap();
        let last_ts = messages.last().map(|m| m.created_at).unwrap();

        // 3. Build transcript
        let mut transcript = format!(
            "Session: {label}\nPlatform: {platform} | Channel: {channel}\nPeriod: {first_ts} \u{2013} {last_ts}\n\n"
        );
        for msg in &messages {
            let speaker = msg.speaker.as_deref().unwrap_or(&msg.role);
            let hhmm = msg.created_at.format("%H:%M");
            transcript.push_str(&format!(
                "[{hhmm}] {} ({}): {}\n",
                speaker, msg.role, msg.content
            ));
        }

        // 4. Ingest as conversation source
        let message_count = messages.len();
        let metadata = serde_json::json!({
            "session_id": session_id.to_string(),
            "session_label": label,
            "message_count": message_count,
            "period_start": first_ts.to_rfc3339(),
            "period_end": last_ts.to_rfc3339(),
        });

        let ingest_req = IngestRequest {
            content: transcript,
            source_type: Some("conversation".to_string()),
            title: Some(format!("Session: {label}")),
            metadata: Some(metadata),
            session_id: Some(session_id),
            reliability: None,
            capture_method: Some("system".to_string()),
            facet_function: None,
            facet_scope: None,
        };

        let source = source_svc.ingest(ingest_req).await?;

        // 5. Mark messages as flushed
        let ids: Vec<Uuid> = messages.iter().map(|m| m.id).collect();
        sqlx::query(
            "UPDATE covalence.session_messages
             SET flushed_at = now()
             WHERE id = ANY($1)",
        )
        .bind(&ids)
        .execute(&self.pool)
        .await?;

        Ok(FlushResult {
            source_id: source.id,
            message_count,
        })
    }

    /// Flush any unflushed messages then close the session.
    pub async fn finalize(
        &self,
        session_id: Uuid,
        _compile: bool,
        source_svc: &SourceService,
    ) -> Result<(), AppError> {
        // Flush if there are any unflushed messages (ignore NotFound — it just means no messages)
        match self.flush(session_id, source_svc).await {
            Ok(_) | Err(AppError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        self.close(session_id).await?;
        Ok(())
    }

    /// Flush all sessions whose last_active_at is older than `threshold_minutes`
    /// and that have at least one unflushed message.
    pub async fn flush_stale(
        &self,
        threshold_minutes: i64,
        source_svc: &SourceService,
    ) -> Result<Vec<FlushResult>, AppError> {
        let stale_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT sm.session_id
             FROM covalence.session_messages sm
             JOIN covalence.sessions s ON s.id = sm.session_id
             WHERE sm.flushed_at IS NULL
               AND s.last_active_at < now() - ($1 * interval '1 minute')",
        )
        .bind(threshold_minutes)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for sid in stale_ids {
            match self.flush(sid, source_svc).await {
                Ok(r) => results.push(r),
                Err(AppError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(results)
    }
}

// ─── row helpers ──────────────────────────────────────────────────────────────

fn session_from_row(row: &sqlx::postgres::PgRow) -> Result<Session, sqlx::Error> {
    Ok(Session {
        id: row.try_get("id")?,
        label: row.try_get("label")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        last_active_at: row.try_get("last_active_at")?,
        metadata: row.try_get("metadata")?,
        platform: row.try_get("platform")?,
        channel: row.try_get("channel")?,
    })
}

fn message_from_row(row: &sqlx::postgres::PgRow) -> Result<SessionMessage, sqlx::Error> {
    Ok(SessionMessage {
        id: row.try_get("id")?,
        session_id: row.try_get("session_id")?,
        speaker: row.try_get("speaker")?,
        role: row.try_get("role")?,
        content: row.try_get("content")?,
        chunk_index: row.try_get("chunk_index")?,
        created_at: row.try_get("created_at")?,
        flushed_at: row.try_get("flushed_at")?,
    })
}
