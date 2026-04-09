//! Semantic query cache.
//!
//! Caches search responses keyed by query embedding similarity.
//! Hit if cosine distance < 0.05 within TTL. LRU eviction at max
//! entries.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::ingestion::embedder::truncate_and_validate;
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
    /// Truncation dimension for query embeddings.
    ///
    /// The `query_cache.query_embedding` column is `halfvec(1024)`
    /// (see migration `005_search.sql`). Embedders generate vectors
    /// at the engine's max dimension (typically 2048 for source); we
    /// truncate + L2-renormalize before INSERT/SELECT so the column
    /// type matches. Without this, every cache write fails with
    /// `expected 1024 dimensions, not 2048` and every query is a miss.
    dim: usize,
}

impl QueryCache {
    /// Create a new query cache with the given pool, config, and
    /// embedding truncation dimension.
    ///
    /// `dim` must match the `query_embedding` column dimension
    /// (currently 1024 — see migration `005_search.sql`).
    pub fn new(pool: PgPool, config: CacheConfig, dim: usize) -> Self {
        Self { pool, config, dim }
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
        let truncated = truncate_and_validate(query_embedding, self.dim, "query_cache")?;
        let pgvec = format!(
            "[{}]",
            truncated
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let ttl_secs = self.config.ttl_seconds as i32;
        let max_dist = self.config.max_distance;

        // Find the closest cached embedding within TTL and
        // distance threshold, matching strategy.
        //
        // The SP returns `(id UUID, response JSONB)` — column name is
        // `response`, not `results`. The previous SELECT referenced
        // `results` and threw `column "results" does not exist` on
        // every lookup. The error was caught and downgraded to a
        // tracing::warn! ("cache lookup failed, proceeding without
        // cache") so search still returned results, but every query
        // was a hard miss against the cache regardless of contents.
        let row: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, response \
             FROM sp_lookup_query_cache($1::halfvec, $2, $3, $4)",
        )
        .bind(&pgvec)
        .bind(strategy)
        .bind(max_dist)
        .bind(ttl_secs)
        .fetch_optional(&self.pool)
        .await?;

        let Some((id, response)) = row else {
            return Ok(None);
        };

        // Bump hit count asynchronously (best-effort).
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = sqlx::query("SELECT sp_bump_cache_hit_count($1)")
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
        let row: (i64,) = sqlx::query_as("SELECT sp_clear_query_cache()")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u64)
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
        let truncated = truncate_and_validate(query_embedding, self.dim, "query_cache")?;
        let pgvec = format!(
            "[{}]",
            truncated
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let response = serde_json::to_value(results)
            .map_err(|e| crate::error::Error::Search(format!("cache serialize: {e}")))?;

        let id = Uuid::new_v4();

        sqlx::query("SELECT sp_store_query_cache($1, $2::halfvec, $3, $4, $5)")
            .bind(id)
            .bind(&pgvec)
            .bind(strategy)
            .bind(&response)
            .bind(query_text)
            .execute(&self.pool)
            .await?;

        // Evict oldest entries beyond max_entries (best-effort).
        let max = self.config.max_entries as i32;
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = sqlx::query("SELECT sp_evict_old_cache_entries($1)")
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
