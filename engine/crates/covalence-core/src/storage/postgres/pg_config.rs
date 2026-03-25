//! ConfigRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::services::config_service::ConfigEntry;
use crate::storage::traits::ConfigRepo;

use super::PgRepo;

impl ConfigRepo for PgRepo {
    async fn list_all_kv(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let rows: Vec<(String, serde_json::Value)> =
            sqlx::query_as("SELECT key, value FROM config")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn list_all_entries(&self) -> Result<Vec<ConfigEntry>> {
        let rows =
            sqlx::query("SELECT key, value, description, updated_at FROM config ORDER BY key")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .iter()
            .map(|r| ConfigEntry {
                key: r.get("key"),
                value: r.get("value"),
                description: r.get("description"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    async fn set(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO config (key, value, updated_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
