//! HookRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::services::hooks::{HookPhase, LifecycleHook};
use crate::storage::traits::HookRepo;

use super::PgRepo;

/// Convert a PgRow to a LifecycleHook.
fn hook_from_row(row: &sqlx::postgres::PgRow) -> LifecycleHook {
    let phase_str: String = row.get("phase");
    LifecycleHook {
        id: row.get("id"),
        name: row.get("name"),
        phase: HookPhase::parse(&phase_str).unwrap_or(HookPhase::PreSearch),
        hook_url: row.get("hook_url"),
        adapter_id: row.get("adapter_id"),
        timeout_ms: row.get("timeout_ms"),
        fail_open: row.get("fail_open"),
        is_active: row.get("is_active"),
    }
}

impl HookRepo for PgRepo {
    async fn list_by_phase(&self, phase: &str) -> Result<Vec<LifecycleHook>> {
        let rows = sqlx::query(
            "SELECT id, name, phase, hook_url, adapter_id, \
             timeout_ms, fail_open, is_active \
             FROM lifecycle_hooks \
             WHERE phase = $1 AND is_active = true \
             ORDER BY name",
        )
        .bind(phase)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(hook_from_row).collect())
    }

    async fn create(&self, hook: &LifecycleHook) -> Result<()> {
        sqlx::query(
            "INSERT INTO lifecycle_hooks \
             (id, name, phase, hook_url, adapter_id, \
              timeout_ms, fail_open, is_active) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(hook.id)
        .bind(&hook.name)
        .bind(hook.phase.as_str())
        .bind(&hook.hook_url)
        .bind(hook.adapter_id)
        .bind(hook.timeout_ms)
        .bind(hook.fail_open)
        .bind(hook.is_active)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: uuid::Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM lifecycle_hooks WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_all(&self) -> Result<Vec<LifecycleHook>> {
        let rows = sqlx::query(
            "SELECT id, name, phase, hook_url, adapter_id, \
             timeout_ms, fail_open, is_active \
             FROM lifecycle_hooks \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(hook_from_row).collect())
    }
}
