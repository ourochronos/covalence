//! Runtime configuration service.
//!
//! Reads config from the `config` table in PostgreSQL. Workers
//! poll periodically; the WebUI reads on demand. All runtime-
//! adjustable settings live here instead of env vars.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::Row;
use tokio::sync::RwLock;

use crate::error::Result;
use crate::storage::postgres::PgRepo;

/// A single configuration entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub description: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Runtime configuration service with polling-based refresh.
pub struct ConfigService {
    repo: Arc<PgRepo>,
    cache: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl ConfigService {
    /// Create a new config service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self {
            repo,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all config from DB into the cache.
    pub async fn refresh(&self) -> Result<()> {
        let rows: Vec<(String, serde_json::Value)> =
            sqlx::query_as("SELECT key, value FROM config")
                .fetch_all(self.repo.pool())
                .await?;

        let mut cache = self.cache.write().await;
        cache.clear();
        for (key, value) in rows {
            cache.insert(key, value);
        }

        Ok(())
    }

    /// Get a config value by key (from cache).
    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        let cache = self.cache.read().await;
        cache.get(key).cloned()
    }

    /// Get a typed config value.
    pub async fn get_as<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let val = self.get(key).await?;
        serde_json::from_value(val).ok()
    }

    /// Get a config value as i64.
    pub async fn get_i64(&self, key: &str) -> Option<i64> {
        let val = self.get(key).await?;
        val.as_i64()
            .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
    }

    /// Get a config value as bool.
    pub async fn get_bool(&self, key: &str) -> Option<bool> {
        let val = self.get(key).await?;
        val.as_bool()
            .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
    }

    /// Get a config value as f64.
    pub async fn get_f64(&self, key: &str) -> Option<f64> {
        let val = self.get(key).await?;
        val.as_f64()
            .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
    }

    /// List all config entries (for WebUI).
    pub async fn list_all(&self) -> Result<Vec<ConfigEntry>> {
        let rows =
            sqlx::query("SELECT key, value, description, updated_at FROM config ORDER BY key")
                .fetch_all(self.repo.pool())
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

    /// Update a config value (from WebUI or API).
    pub async fn set(&self, key: &str, value: serde_json::Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO config (key, value, updated_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = NOW()",
        )
        .bind(key)
        .bind(&value)
        .execute(self.repo.pool())
        .await?;

        // Update cache immediately.
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), value);

        Ok(())
    }

    /// Start the polling loop (call from worker/API).
    /// Refreshes config every `interval_secs` seconds.
    pub fn spawn_refresh_loop(self: &Arc<Self>, interval_secs: u64) {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(e) = svc.refresh().await {
                    tracing::warn!(error = %e, "config refresh failed");
                }
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
            }
        });
    }
}
