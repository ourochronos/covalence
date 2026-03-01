//! Session system — tracking agent interaction context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct Session {
    pub id: Uuid,
    pub label: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub label: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

pub struct SessionService {
    pool: PgPool,
}

impl SessionService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, req: CreateSessionRequest) -> Result<Session, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO covalence.sessions (id, label, metadata)
             VALUES ($1, $2, $3)
             RETURNING id, label, status, created_at, last_active_at, metadata",
        )
        .bind(id)
        .bind(&req.label)
        .bind(&req.metadata)
        .fetch_one(&self.pool)
        .await?;
        session_from_row(&row)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Session>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, label, status, created_at, last_active_at, metadata
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

    pub async fn get_by_label(&self, label: &str) -> Result<Option<Session>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, label, status, created_at, last_active_at, metadata
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
            "SELECT id, label, status, created_at, last_active_at, metadata
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
}

fn session_from_row(row: &sqlx::postgres::PgRow) -> Result<Session, sqlx::Error> {
    Ok(Session {
        id: row.try_get("id")?,
        label: row.try_get("label")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        last_active_at: row.try_get("last_active_at")?,
        metadata: row.try_get("metadata")?,
    })
}
