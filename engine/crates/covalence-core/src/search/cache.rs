//! Semantic query cache.
//!
//! Caches search responses keyed by query embedding similarity.
//! Hit if cosine distance < 0.05 within TTL. LRU eviction at max
//! entries.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::search::fusion::FusedResult;

/// Configuration for the semantic query cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum cosine distance for a cache hit.
    pub max_distance: f64,
    /// TTL in seconds.
    pub ttl_seconds: u64,
    /// Maximum number of cached entries.
    pub max_entries: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_distance: 0.05,
            ttl_seconds: 3600,
            max_entries: 10_000,
        }
    }
}

/// A cached query response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    /// The original query text.
    pub query_text: String,
    /// The cached response payload (serialized `Vec<FusedResult>`).
    pub response: serde_json::Value,
    /// Strategy that was used.
    pub strategy_used: String,
    /// Number of times this cache entry was hit.
    pub hit_count: u64,
}

/// Semantic query cache backed by PostgreSQL.
///
/// Uses pgvector cosine distance to find semantically similar
/// previous queries within the configured TTL window. On a hit
/// the cached results are returned directly, avoiding a full
/// multi-dimension search.
pub struct QueryCache {
    pool: PgPool,
    config: CacheConfig,
}

impl QueryCache {
    /// Create a new query cache with the given pool and config.
    pub fn new(pool: PgPool, config: CacheConfig) -> Self {
        Self { pool, config }
    }

    /// Look up a semantically similar cached query.
    ///
    /// Searches the `query_cache` table for an entry whose
    /// embedding has cosine distance < `max_distance` from
    /// the provided embedding and whose `created_at` is within
    /// the TTL window. Returns the cached results on hit.
    pub async fn lookup(
        &self,
        query_embedding: &[f64],
        strategy: &str,
    ) -> Result<Option<Vec<FusedResult>>> {
        let pgvec = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let ttl_secs = self.config.ttl_seconds as f64;
        let max_dist = self.config.max_distance;

        // Find the closest cached embedding within TTL and
        // distance threshold, matching strategy.
        let sql = "\
            SELECT id, response, \
                   (query_embedding <=> $1::halfvec)::float8 AS dist \
            FROM query_cache \
            WHERE strategy_used = $2 \
              AND created_at > NOW() - ($3::float8 || ' seconds')::interval \
              AND (query_embedding <=> $1::halfvec)::float8 < $4 \
            ORDER BY query_embedding <=> $1::halfvec \
            LIMIT 1";

        let row: Option<(Uuid, serde_json::Value, f64)> = sqlx::query_as(sql)
            .bind(&pgvec)
            .bind(strategy)
            .bind(ttl_secs)
            .bind(max_dist)
            .fetch_optional(&self.pool)
            .await?;

        let Some((id, response, _dist)) = row else {
            return Ok(None);
        };

        // Bump hit count asynchronously (best-effort).
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE query_cache \
                 SET hit_count = hit_count + 1 \
                 WHERE id = $1",
            )
            .bind(id)
            .execute(&pool)
            .await;
        });

        let results: Vec<FusedResult> = serde_json::from_value(response)
            .map_err(|e| crate::error::Error::Search(format!("cache deserialize: {e}")))?;

        Ok(Some(results))
    }

    /// Clear all cached entries.
    pub async fn clear(&self) -> Result<u64> {
        let result = sqlx::query("DELETE FROM query_cache")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Store search results in the cache.
    ///
    /// Inserts the query text, embedding, strategy, and serialized
    /// results into the `query_cache` table. Enforces the max
    /// entries limit by deleting the oldest entries.
    pub async fn store(
        &self,
        query_text: &str,
        query_embedding: &[f64],
        strategy: &str,
        results: &[FusedResult],
    ) -> Result<()> {
        let pgvec = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let response = serde_json::to_value(results)
            .map_err(|e| crate::error::Error::Search(format!("cache serialize: {e}")))?;

        let id = Uuid::new_v4();

        sqlx::query(
            "INSERT INTO query_cache \
             (id, query_text, query_embedding, strategy_used, \
              response, hit_count, created_at) \
             VALUES ($1, $2, $3::halfvec, $4, $5, 0, NOW())",
        )
        .bind(id)
        .bind(query_text)
        .bind(&pgvec)
        .bind(strategy)
        .bind(&response)
        .execute(&self.pool)
        .await?;

        // Evict oldest entries beyond max_entries (best-effort).
        let max = self.config.max_entries as i64;
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "DELETE FROM query_cache \
                 WHERE id IN ( \
                     SELECT id FROM query_cache \
                     ORDER BY created_at DESC \
                     OFFSET $1 \
                 )",
            )
            .bind(max)
            .execute(&pool)
            .await;
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_config_defaults() {
        let config = CacheConfig::default();
        assert!((config.max_distance - 0.05).abs() < 1e-10);
        assert_eq!(config.ttl_seconds, 3600);
        assert_eq!(config.max_entries, 10_000);
    }

    #[test]
    fn cached_response_serde_roundtrip() {
        let resp = CachedResponse {
            query_text: "test query".to_string(),
            response: serde_json::json!({"results": []}),
            strategy_used: "balanced".to_string(),
            hit_count: 5,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let back: CachedResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.query_text, "test query");
        assert_eq!(back.hit_count, 5);
        assert_eq!(back.strategy_used, "balanced");
    }

    #[test]
    fn cache_config_custom() {
        let config = CacheConfig {
            max_distance: 0.1,
            ttl_seconds: 7200,
            max_entries: 5000,
        };
        assert!((config.max_distance - 0.1).abs() < 1e-10);
        assert_eq!(config.ttl_seconds, 7200);
        assert_eq!(config.max_entries, 5000);
    }
}
