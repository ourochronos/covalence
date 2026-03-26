//! SessionRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::session::{Session, Turn};
use crate::storage::traits::SessionRepo;

use super::PgRepo;

impl SessionRepo for PgRepo {
    async fn create_session(&self, session: &Session) -> Result<()> {
        sqlx::query(
            "INSERT INTO sessions (id, name, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(session.id)
        .bind(&session.name)
        .bind(&session.metadata)
        .bind(session.created_at)
        .bind(session.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_session(&self, id: uuid::Uuid) -> Result<Option<Session>> {
        let row = sqlx::query(
            "SELECT id, name, metadata, created_at, updated_at
             FROM sessions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| session_from_row(&r)))
    }

    async fn add_turn(&self, turn: &Turn) -> Result<Turn> {
        let row = sqlx::query(
            "INSERT INTO turns (id, session_id, role, content, metadata, ordinal, created_at)
             SELECT $1, $2, $3, $4, $5,
                    COALESCE(MAX(ordinal), 0) + 1,
                    $6
             FROM turns WHERE session_id = $2
             RETURNING id, session_id, role, content, metadata, ordinal, created_at",
        )
        .bind(turn.id)
        .bind(turn.session_id)
        .bind(&turn.role)
        .bind(&turn.content)
        .bind(&turn.metadata)
        .bind(turn.created_at)
        .fetch_one(&self.pool)
        .await?;

        // Update the session's updated_at timestamp.
        sqlx::query("UPDATE sessions SET updated_at = NOW() WHERE id = $1")
            .bind(turn.session_id)
            .execute(&self.pool)
            .await?;

        Ok(turn_from_row(&row))
    }

    async fn get_history(&self, session_id: uuid::Uuid, last_n: i64) -> Result<Vec<Turn>> {
        // Select the last N turns ordered by ordinal DESC, then
        // reverse in Rust so the caller gets chronological order.
        let rows = sqlx::query(
            "SELECT id, session_id, role, content, metadata, ordinal, created_at
             FROM turns
             WHERE session_id = $1
             ORDER BY ordinal DESC
             LIMIT $2",
        )
        .bind(session_id)
        .bind(last_n)
        .fetch_all(&self.pool)
        .await?;

        let mut turns: Vec<Turn> = rows.iter().map(turn_from_row).collect();
        turns.reverse();
        Ok(turns)
    }

    async fn close_session(&self, id: uuid::Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_sessions(&self, limit: i64, offset: i64) -> Result<Vec<Session>> {
        let rows = sqlx::query(
            "SELECT id, name, metadata, created_at, updated_at
             FROM sessions
             ORDER BY updated_at DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(session_from_row).collect())
    }
}

fn session_from_row(row: &sqlx::postgres::PgRow) -> Session {
    Session {
        id: row.get("id"),
        name: row.get("name"),
        metadata: row.get("metadata"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn turn_from_row(row: &sqlx::postgres::PgRow) -> Turn {
    Turn {
        id: row.get("id"),
        session_id: row.get("session_id"),
        role: row.get("role"),
        content: row.get("content"),
        metadata: row.get("metadata"),
        ordinal: row.get("ordinal"),
        created_at: row.get("created_at"),
    }
}
