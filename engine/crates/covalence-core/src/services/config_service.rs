//! Runtime configuration service.
//!
//! Reads config from the `config` table in PostgreSQL. Workers
//! poll periodically; the WebUI reads on demand. All runtime-
//! adjustable settings live here instead of env vars.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::Result;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::ConfigRepo;

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
        let rows = ConfigRepo::list_all_kv(&*self.repo).await?;

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
        ConfigRepo::list_all_entries(&*self.repo).await
    }

    /// Get all config entries for a given extension.
    ///
    /// Queries the cache for keys starting with `ext.<extension_name>.`
    /// and returns them with the namespace prefix stripped.
    pub async fn extension_config(
        &self,
        extension_name: &str,
    ) -> HashMap<String, serde_json::Value> {
        let prefix = format!("ext.{}.", extension_name);
        let cache = self.cache.read().await;
        cache
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&prefix)
                    .map(|stripped| (stripped.to_string(), v.clone()))
            })
            .collect()
    }

    /// Update a config value (from WebUI or API).
    pub async fn set(&self, key: &str, value: serde_json::Value) -> Result<()> {
        ConfigRepo::set(&*self.repo, key, &value).await?;

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

    /// Inject values into the cache for testing (bypasses DB).
    #[cfg(test)]
    pub async fn inject_for_test(&self, entries: Vec<(String, serde_json::Value)>) {
        let mut cache = self.cache.write().await;
        for (k, v) in entries {
            cache.insert(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a ConfigService without a real DB for cache-only tests.
    fn test_config_service() -> ConfigService {
        // Safety: we never call refresh() or set() — only
        // cache-only methods.
        let pool = unsafe {
            sqlx::PgPool::connect_lazy("postgres://fake@localhost/fake").unwrap_unchecked()
        };
        let repo = Arc::new(PgRepo::from_pool(pool));
        ConfigService::new(repo)
    }

    #[tokio::test]
    async fn extension_config_returns_matching_keys() {
        let svc = test_config_service();
        svc.inject_for_test(vec![
            (
                "ext.code-analysis.languages".into(),
                serde_json::json!(["rust", "python"]),
            ),
            ("ext.code-analysis.max_depth".into(), serde_json::json!(10)),
            ("ext.research.enabled".into(), serde_json::json!(true)),
            ("other.key".into(), serde_json::json!("ignored")),
        ])
        .await;

        let cfg = svc.extension_config("code-analysis").await;
        assert_eq!(cfg.len(), 2);
        assert_eq!(
            cfg.get("languages"),
            Some(&serde_json::json!(["rust", "python"]))
        );
        assert_eq!(cfg.get("max_depth"), Some(&serde_json::json!(10)));
    }

    #[tokio::test]
    async fn extension_config_returns_empty_for_unknown() {
        let svc = test_config_service();
        svc.inject_for_test(vec![(
            "ext.code-analysis.languages".into(),
            serde_json::json!(["rust"]),
        )])
        .await;

        let cfg = svc.extension_config("nonexistent").await;
        assert!(cfg.is_empty());
    }

    #[tokio::test]
    async fn extension_config_no_partial_prefix_match() {
        let svc = test_config_service();
        svc.inject_for_test(vec![(
            "ext.code-analysis-v2.key".into(),
            serde_json::json!("val"),
        )])
        .await;

        // Should NOT match "code-analysis" (partial prefix).
        let cfg = svc.extension_config("code-analysis").await;
        assert!(cfg.is_empty());
    }
}
