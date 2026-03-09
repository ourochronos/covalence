//! AuditLogRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::audit::AuditLog;
use crate::storage::traits::AuditLogRepo;
use crate::types::ids::AuditLogId;

use super::PgRepo;

impl AuditLogRepo for PgRepo {
    async fn create(&self, log: &AuditLog) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_logs (
                id, action, actor, target_type,
                target_id, payload, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(log.id)
        .bind(&log.action)
        .bind(&log.actor)
        .bind(&log.target_type)
        .bind(log.target_id)
        .bind(&log.payload)
        .bind(log.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: AuditLogId) -> Result<Option<AuditLog>> {
        let row = sqlx::query(
            "SELECT id, action, actor, target_type,
                    target_id, payload, created_at
             FROM audit_logs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| audit_from_row(&r)))
    }

    async fn list_by_target(
        &self,
        target_type: &str,
        target_id: uuid::Uuid,
        limit: i64,
    ) -> Result<Vec<AuditLog>> {
        let rows = sqlx::query(
            "SELECT id, action, actor, target_type,
                    target_id, payload, created_at
             FROM audit_logs
             WHERE target_type = $1 AND target_id = $2
             ORDER BY created_at DESC
             LIMIT $3",
        )
        .bind(target_type)
        .bind(target_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(audit_from_row).collect())
    }

    async fn list_recent(&self, limit: i64) -> Result<Vec<AuditLog>> {
        let rows = sqlx::query(
            "SELECT id, action, actor, target_type,
                    target_id, payload, created_at
             FROM audit_logs
             ORDER BY created_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(audit_from_row).collect())
    }
}

fn audit_from_row(row: &sqlx::postgres::PgRow) -> AuditLog {
    AuditLog {
        id: row.get("id"),
        action: row.get("action"),
        actor: row.get("actor"),
        target_type: row.get("target_type"),
        target_id: row.get("target_id"),
        payload: row.get("payload"),
        created_at: row.get("created_at"),
    }
}
