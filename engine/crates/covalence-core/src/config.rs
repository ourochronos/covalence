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

    /// Embedding provider: `"openai"` (default) or `"voyage"`.
    ///
    /// When set to `"voyage"` or when `VOYAGE_API_KEY` is present,
    /// the Voyage AI embedder is used instead of OpenAI.
    /// Env: `COVALENCE_EMBED_PROVIDER`.
    pub embed_provider: String,

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

    /// Which entity extractor backend to use.
    ///
    /// Supported values: `"llm"` (default), `"gliner2"`.
    pub entity_extractor: String,

    /// Base URL for the extraction sidecar (used by `gliner2` backend).
    ///
    /// Env: `COVALENCE_EXTRACT_URL`. Defaults to `http://localhost:8432`
    /// when the `gliner2` backend is selected.
    pub extract_url: Option<String>,

    /// GLiNER2 confidence threshold in \[0.0, 1.0\].
    ///
    /// Entities scoring below this threshold are discarded by the
    /// sidecar. Env: `COVALENCE_GLINER_THRESHOLD`. Default: `0.5`.
    pub gliner_threshold: f32,

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
///
/// Supports per-table embedding dimensions for matryoshka models
/// (e.g. OpenAI `text-embedding-3-large`). The API request uses
/// `max_dim()` and results are truncated + renormalized per table.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Model name (e.g. `text-embedding-3-large`).
    pub model: String,

    /// Maximum number of texts to embed in a single API call.
    pub batch_size: usize,

    /// Per-table embedding dimensions.
    pub table_dims: TableDimensions,
}

/// Per-table embedding dimensions. Each table can have a different
/// vector dimension to optimize quality vs. storage/performance.
///
/// Models with matryoshka support (OpenAI `text-embedding-3-*`,
/// Jina v3) produce vectors that can be truncated to fewer
/// dimensions while preserving quality. Tables with fewer records
/// or richer content benefit from higher dimensionality.
#[derive(Debug, Clone)]
pub struct TableDimensions {
    /// Source-level embeddings (full document).
    /// Fewest records, richest content.
    /// Env: `COVALENCE_EMBED_DIM_SOURCE`. Default: `2048`.
    pub source: usize,

    /// Chunk-level embeddings (paragraphs/sections).
    /// Most records, searched frequently.
    /// Env: `COVALENCE_EMBED_DIM_CHUNK`. Default: `1024`.
    pub chunk: usize,

    /// Article-level embeddings (compiled summaries).
    /// Env: `COVALENCE_EMBED_DIM_ARTICLE`. Default: `1024`.
    pub article: usize,

    /// Node-level embeddings (entity name + description).
    /// Short text, used in resolution lookups.
    /// Env: `COVALENCE_EMBED_DIM_NODE`. Default: `256`.
    pub node: usize,

    /// Node alias embeddings (alternate names).
    /// Must match node dimension for cosine comparisons.
    /// Env: `COVALENCE_EMBED_DIM_ALIAS`. Default: `256`.
    pub alias: usize,
}

impl Default for TableDimensions {
    fn default() -> Self {
        Self {
            source: 2048,
            chunk: 1024,
            article: 1024,
            node: 256,
            alias: 256,
        }
    }
}

impl TableDimensions {
    /// The maximum dimension across all tables. Used as the API
    /// request dimension — results are truncated per table.
    pub fn max_dim(&self) -> usize {
        self.source
            .max(self.chunk)
            .max(self.article)
            .max(self.node)
            .max(self.alias)
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "text-embedding-3-large".to_string(),
            batch_size: 64,
            table_dims: TableDimensions::default(),
        }
    }
}

impl EmbeddingConfig {
    /// The maximum dimension across all tables.
    ///
    /// Sent to the embedding API as the `dimensions` parameter.
    pub fn max_dim(&self) -> usize {
        self.table_dims.max_dim()
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

    /// Minimum fused score for the top result before abstention
    /// triggers. RRF with k=60 produces scores ~0.001-0.01.
    /// Env: `COVALENCE_SEARCH_ABSTENTION_THRESHOLD`. Default: `0.001`.
    pub abstention_threshold: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            rrf_k: 60.0,
            default_limit: 10,
            abstention_threshold: 0.001,
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
            .field("embed_provider", &self.embed_provider)
            .field("embed_model", &self.embed_model)
            .field("chat_model", &self.chat_model)
            .field("chunk_size", &self.chunk_size)
            .field("chunk_overlap", &self.chunk_overlap)
            .field("embedding", &self.embedding)
            .field("extract_concurrency", &self.extract_concurrency)
            .field("entity_extractor", &self.entity_extractor)
            .field("extract_url", &self.extract_url)
            .field("gliner_threshold", &self.gliner_threshold)
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
        let embed_provider = env_or("COVALENCE_EMBED_PROVIDER", "openai");
        let embed_model = env_or("COVALENCE_EMBED_MODEL", "text-embedding-3-large");

        Ok(Self {
            database_url: require_env("DATABASE_URL")?,
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8431"),
            api_key: optional_env("COVALENCE_API_KEY"),
            openai_api_key: optional_env("OPENAI_API_KEY"),
            openai_base_url: optional_env("OPENAI_BASE_URL"),
            voyage_api_key: optional_env("VOYAGE_API_KEY"),
            voyage_base_url: optional_env("VOYAGE_BASE_URL"),
            embed_provider,
            embed_model: embed_model.clone(),
            chat_model: env_or("COVALENCE_CHAT_MODEL", "gpt-4o"),
            chat_api_key: optional_env("COVALENCE_CHAT_API_KEY"),
            chat_base_url: optional_env("COVALENCE_CHAT_BASE_URL"),
            chunk_size: env_parse("COVALENCE_CHUNK_SIZE", 1000)?,
            chunk_overlap: env_parse("COVALENCE_CHUNK_OVERLAP", 200)?,
            embedding: {
                // Per-table dimension config. Legacy COVALENCE_EMBED_DIM
                // is used as fallback for chunk/article/source if the
                // per-table vars are not set.
                let legacy_dim: usize = env_parse("COVALENCE_EMBED_DIM", 1024)?;
                let legacy_node: usize = env_parse("COVALENCE_NODE_EMBED_DIM", 256)?;
                EmbeddingConfig {
                    model: embed_model,
                    batch_size: env_parse("COVALENCE_EMBED_BATCH", 64)?,
                    table_dims: TableDimensions {
                        source: env_parse("COVALENCE_EMBED_DIM_SOURCE", 2048)?,
                        chunk: env_parse("COVALENCE_EMBED_DIM_CHUNK", legacy_dim)?,
                        article: env_parse("COVALENCE_EMBED_DIM_ARTICLE", legacy_dim)?,
                        node: env_parse("COVALENCE_EMBED_DIM_NODE", legacy_node)?,
                        alias: env_parse("COVALENCE_EMBED_DIM_ALIAS", legacy_node)?,
                    },
                }
            },
            extract_concurrency: env_parse("COVALENCE_EXTRACT_CONCURRENCY", 8)?,
            entity_extractor: env_or("COVALENCE_ENTITY_EXTRACTOR", "llm"),
            extract_url: optional_env("COVALENCE_EXTRACT_URL"),
            gliner_threshold: env_parse("COVALENCE_GLINER_THRESHOLD", 0.5_f32)?,
            consolidation: ConsolidationConfig {
                batch_interval_secs: env_parse("COVALENCE_BATCH_INTERVAL", 300)?,
                deep_interval_secs: env_parse("COVALENCE_DEEP_INTERVAL", 86_400)?,
                delta_threshold: env_parse_f64("COVALENCE_DELTA_THRESHOLD", 0.1)?,
            },
            search: SearchConfig {
                rrf_k: env_parse_f64("COVALENCE_RRF_K", 60.0)?,
                default_limit: env_parse("COVALENCE_DEFAULT_LIMIT", 10)?,
                abstention_threshold: env_parse_f64(
                    "COVALENCE_SEARCH_ABSTENTION_THRESHOLD",
                    0.001,
                )?,
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
        assert!(
            (env_parse_f64(key, std::f64::consts::PI).unwrap() - std::f64::consts::PI).abs()
                < 1e-10
        );
        unsafe { std::env::remove_var(key) };
    }
}
