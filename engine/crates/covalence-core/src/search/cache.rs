//! Semantic query cache.
//!
//! Caches search responses keyed by query embedding similarity.
//! Hit if cosine distance < 0.05 within TTL. LRU eviction at max
//! entries.

use serde::{Deserialize, Serialize};

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
    /// The cached response payload.
    pub response: serde_json::Value,
    /// Strategy that was used.
    pub strategy_used: String,
    /// Number of times this cache entry was hit.
    pub hit_count: u64,
}
