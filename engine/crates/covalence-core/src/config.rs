//! Application configuration loaded from environment variables.

use crate::error::{Error, Result};

/// Top-level configuration for the Covalence engine.
#[derive(Clone)]
pub struct Config {
    /// PostgreSQL connection string.
    pub database_url: String,

    /// Address to bind the HTTP server to.
    pub bind_addr: String,

    /// Optional API key for authentication.
    pub api_key: Option<String>,

    /// OpenAI API key for embedding and extraction.
    pub openai_api_key: Option<String>,

    /// OpenAI-compatible base URL.
    pub openai_base_url: Option<String>,

    /// Voyage API key (if using Voyage for embeddings).
    pub voyage_api_key: Option<String>,

    /// Voyage base URL.
    pub voyage_base_url: Option<String>,

    /// Embedding model identifier.
    pub embed_model: String,

    /// Chat/completion model identifier.
    pub chat_model: String,

    /// Separate API key for chat/extraction (falls back to OPENAI_API_KEY).
    pub chat_api_key: Option<String>,

    /// Separate base URL for chat/extraction (falls back to OPENAI_BASE_URL).
    pub chat_base_url: Option<String>,

    /// Maximum chunk size in bytes before paragraph splitting.
    pub chunk_size: usize,

    /// Number of characters from the end of the previous chunk to
    /// prepend as overlap context to the next chunk.
    pub chunk_overlap: usize,

    /// Embedding-specific configuration.
    pub embedding: EmbeddingConfig,

    /// Maximum number of concurrent LLM extraction calls during
    /// ingestion. Controls how many chunks are sent to the
    /// extractor in parallel.
    pub extract_concurrency: usize,

    /// Consolidation scheduling configuration.
    pub consolidation: ConsolidationConfig,

    /// Search behavior configuration.
    pub search: SearchConfig,

    /// Trigram similarity threshold for entity and relationship
    /// resolution (0.0–1.0). Values below this are not considered
    /// matches.
    pub resolve_trigram_threshold: f32,

    /// Cosine similarity threshold for vector-based entity
    /// resolution (0.0–1.0). During ingestion, if exact and alias
    /// matches fail, the resolver compares the entity name embedding
    /// against existing node embeddings. A match is accepted when
    /// similarity exceeds this threshold.
    pub resolve_vector_threshold: f32,
}

/// Configuration for the embedding subsystem.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Model name (e.g. `bge-base-en-v1.5`).
    pub model: String,

    /// Vector dimensions produced by the model.
    pub dimensions: usize,

    /// Maximum number of texts to embed in a single API call.
    pub batch_size: usize,

    /// Vector dimensions for node (entity) embeddings.
    ///
    /// Node embeddings are generated from the entity's canonical name
    /// and description. This may differ from chunk embedding
    /// dimensions to save storage.
    pub node_embed_dim: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "voyage-context-3".to_string(),
            dimensions: 2048,
            batch_size: 64,
            node_embed_dim: 256,
        }
    }
}

/// Configuration for the consolidation tiers.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Interval between batch consolidation runs, in seconds.
    pub batch_interval_secs: u64,

    /// Interval between deep consolidation runs, in seconds.
    pub deep_interval_secs: u64,

    /// Minimum epistemic delta required to trigger recompilation.
    pub delta_threshold: f64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            batch_interval_secs: 300,   // 5 minutes
            deep_interval_secs: 86_400, // 24 hours
            delta_threshold: 0.1,
        }
    }
}

/// Configuration for the search subsystem.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// RRF k parameter (controls rank-score steepness).
    pub rrf_k: f64,

    /// Default result limit when unspecified by the caller.
    pub default_limit: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60.0,
            default_limit: 10,
        }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &self.database_url)
            .field("bind_addr", &self.bind_addr)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field(
                "openai_api_key",
                &self.openai_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("openai_base_url", &self.openai_base_url)
            .field(
                "voyage_api_key",
                &self.voyage_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("voyage_base_url", &self.voyage_base_url)
            .field("embed_model", &self.embed_model)
            .field("chat_model", &self.chat_model)
            .field("chunk_size", &self.chunk_size)
            .field("chunk_overlap", &self.chunk_overlap)
            .field("embedding", &self.embedding)
            .field("extract_concurrency", &self.extract_concurrency)
            .field("consolidation", &self.consolidation)
            .field("search", &self.search)
            .field("resolve_trigram_threshold", &self.resolve_trigram_threshold)
            .field("resolve_vector_threshold", &self.resolve_vector_threshold)
            .finish()
    }
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Call `dotenvy::dotenv().ok()` before this to load `.env` files.
    pub fn from_env() -> Result<Self> {
        let embed_model = env_or("COVALENCE_EMBED_MODEL", "bge-base-en-v1.5");

        Ok(Self {
            database_url: require_env("DATABASE_URL")?,
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8431"),
            api_key: optional_env("COVALENCE_API_KEY"),
            openai_api_key: optional_env("OPENAI_API_KEY"),
            openai_base_url: optional_env("OPENAI_BASE_URL"),
            voyage_api_key: optional_env("VOYAGE_API_KEY"),
            voyage_base_url: optional_env("VOYAGE_BASE_URL"),
            embed_model: embed_model.clone(),
            chat_model: env_or("COVALENCE_CHAT_MODEL", "gpt-4o"),
            chat_api_key: optional_env("COVALENCE_CHAT_API_KEY"),
            chat_base_url: optional_env("COVALENCE_CHAT_BASE_URL"),
            chunk_size: env_parse("COVALENCE_CHUNK_SIZE", 1000)?,
            chunk_overlap: env_parse("COVALENCE_CHUNK_OVERLAP", 200)?,
            embedding: EmbeddingConfig {
                model: embed_model,
                dimensions: env_parse("COVALENCE_EMBED_DIM", 2048)?,
                batch_size: env_parse("COVALENCE_EMBED_BATCH", 64)?,
                node_embed_dim: env_parse("COVALENCE_NODE_EMBED_DIM", 256)?,
            },
            extract_concurrency: env_parse("COVALENCE_EXTRACT_CONCURRENCY", 8)?,
            consolidation: ConsolidationConfig {
                batch_interval_secs: env_parse("COVALENCE_BATCH_INTERVAL", 300)?,
                deep_interval_secs: env_parse("COVALENCE_DEEP_INTERVAL", 86_400)?,
                delta_threshold: env_parse_f64("COVALENCE_DELTA_THRESHOLD", 0.1)?,
            },
            search: SearchConfig {
                rrf_k: env_parse_f64("COVALENCE_RRF_K", 60.0)?,
                default_limit: env_parse("COVALENCE_DEFAULT_LIMIT", 10)?,
            },
            resolve_trigram_threshold: env_parse("COVALENCE_RESOLVE_TRIGRAM_THRESHOLD", 0.4)?,
            resolve_vector_threshold: env_parse("COVALENCE_RESOLVE_VECTOR_THRESHOLD", 0.85_f32)?,
        })
    }
}

fn require_env(key: &str) -> Result<String> {
    std::env::var(key)
        .map_err(|_| Error::Config(format!("required environment variable {key} not set")))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> Result<T> {
    match std::env::var(key).ok().filter(|v| !v.is_empty()) {
        Some(v) => v
            .parse::<T>()
            .map_err(|_| Error::Config(format!("invalid value for {key}: {v}"))),
        None => Ok(default),
    }
}

fn env_parse_f64(key: &str, default: f64) -> Result<f64> {
    match std::env::var(key).ok().filter(|v| !v.is_empty()) {
        Some(v) => v
            .parse::<f64>()
            .map_err(|_| Error::Config(format!("invalid float value for {key}: {v}"))),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_or_returns_default_when_unset() {
        let key = "COVALENCE_TEST_UNSET_12345";
        // SAFETY: test-only, single-threaded test runner.
        unsafe { std::env::remove_var(key) };
        assert_eq!(env_or(key, "fallback"), "fallback");
    }

    #[test]
    fn env_or_returns_default_when_empty() {
        let key = "COVALENCE_TEST_EMPTY_12345";
        // SAFETY: test-only, single-threaded test runner.
        unsafe { std::env::set_var(key, "") };
        assert_eq!(env_or(key, "fallback"), "fallback");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn env_or_returns_value_when_set() {
        let key = "COVALENCE_TEST_SET_12345";
        // SAFETY: test-only, single-threaded test runner.
        unsafe { std::env::set_var(key, "custom") };
        assert_eq!(env_or(key, "fallback"), "custom");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn env_parse_returns_default_when_empty() {
        let key = "COVALENCE_TEST_PARSE_EMPTY_12345";
        // SAFETY: test-only, single-threaded test runner.
        unsafe { std::env::set_var(key, "") };
        assert_eq!(env_parse::<usize>(key, 42).unwrap(), 42);
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn env_parse_f64_returns_default_when_empty() {
        let key = "COVALENCE_TEST_PARSEF64_EMPTY_12345";
        // SAFETY: test-only, single-threaded test runner.
        unsafe { std::env::set_var(key, "") };
        assert!((env_parse_f64(key, 3.14).unwrap() - 3.14).abs() < 1e-10);
        unsafe { std::env::remove_var(key) };
    }
}
