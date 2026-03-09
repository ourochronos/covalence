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
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "voyage-context-3".to_string(),
            dimensions: 2048,
            batch_size: 64,
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
            .field("embedding", &self.embedding)
            .field("extract_concurrency", &self.extract_concurrency)
            .field("consolidation", &self.consolidation)
            .field("search", &self.search)
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
            embedding: EmbeddingConfig {
                model: embed_model,
                dimensions: env_parse("COVALENCE_EMBED_DIM", 2048)?,
                batch_size: env_parse("COVALENCE_EMBED_BATCH", 64)?,
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
        })
    }
}

fn require_env(key: &str) -> Result<String> {
    std::env::var(key)
        .map_err(|_| Error::Config(format!("required environment variable {key} not set")))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> Result<T> {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<T>()
            .map_err(|_| Error::Config(format!("invalid value for {key}: {v}"))),
        Err(_) => Ok(default),
    }
}

fn env_parse_f64(key: &str, default: f64) -> Result<f64> {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<f64>()
            .map_err(|_| Error::Config(format!("invalid float value for {key}: {v}"))),
        Err(_) => Ok(default),
    }
}
