//! Application configuration.

/// Configuration for an external STDIO service discovered from env
/// vars.
///
/// Parsed from `COVALENCE_SERVICE_<NAME>_COMMAND` plus the optional
/// `_ARGS` suffix (comma-separated).
#[derive(Debug, Clone)]
pub struct ExternalServiceConfig {
    /// Human-readable service name (lowercased from the env var).
    pub name: String,
    /// Command to execute.
    pub command: String,
    /// Arguments to pass to the command.
    pub args: Vec<String>,
}

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
    /// Graph engine backend: "petgraph" (in-memory sidecar) or "age"
    /// (Apache AGE in PostgreSQL).
    /// Env: `COVALENCE_GRAPH_ENGINE`. Default: `"petgraph"`.
    pub graph_engine: String,

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

    /// Chat backend type: "cli" (default, shells out to a CLI command
    /// like `gemini` with automatic HTTP fallback) or "http"
    /// (OpenAI-compatible API only).
    pub chat_backend: String,

    /// CLI command for the "cli" chat backend.
    /// Supports: "claude", "gemini", "copilot". Default: "claude".
    pub chat_cli_command: String,

    /// Maximum chunk size in bytes before paragraph splitting.
    pub chunk_size: usize,

    /// Number of characters from the end of the previous chunk to
    /// prepend as overlap context to the next chunk.
    pub chunk_overlap: usize,

    /// Minimum section body size for sibling merging.
    ///
    /// Sections below this threshold are merged with consecutive
    /// siblings sharing the same parent heading. Set to 0 to
    /// disable merging.
    pub min_section_size: usize,

    /// Embedding-specific configuration.
    pub embedding: EmbeddingConfig,

    /// Maximum number of concurrent LLM extraction calls during
    /// ingestion. Controls how many chunks are sent to the
    /// extractor in parallel.
    pub extract_concurrency: usize,

    /// Minimum token count for a chunk to be sent to the LLM
    /// extractor. Chunks below this threshold are skipped to
    /// avoid wasting API round-trips on tiny fragments.
    /// Env: `COVALENCE_MIN_EXTRACT_TOKENS`. Default: `30`.
    pub min_extract_tokens: usize,

    /// Token budget for batching adjacent small chunks into a
    /// single LLM extraction call. Adjacent chunks are concatenated
    /// (separated by `---`) up to this budget. Reduces API calls
    /// significantly for documents with many small chunks.
    /// Env: `COVALENCE_EXTRACT_BATCH_TOKENS`. Default: `2000`.
    pub extract_batch_tokens: usize,

    /// Consolidation scheduling configuration.
    pub consolidation: ConsolidationConfig,

    /// Search behavior configuration.
    pub search: SearchConfig,

    /// Per-stage pipeline configuration (enable/disable, windowing).
    pub pipeline: PipelineConfig,

    /// Which entity extractor backend to use.
    ///
    /// Supported values: `"llm"` (default), `"gliner2"`, `"two_pass"`.
    /// `two_pass` uses GLiNER for entities then targeted LLM for
    /// relationships only.
    pub entity_extractor: String,

    /// Base URL for the extraction service (used by `gliner2` backend).
    ///
    /// Env: `COVALENCE_EXTRACT_URL`. Defaults to `http://localhost:8432`
    /// when the `gliner2` backend is selected.
    pub extract_url: Option<String>,

    /// GLiNER2 confidence threshold in \[0.0, 1.0\].
    ///
    /// Entities scoring below this threshold are discarded by the
    /// service. Env: `COVALENCE_GLINER_THRESHOLD`. Default: `0.5`.
    pub gliner_threshold: f32,

    /// Base URL for the Fastcoref neural coreference service.
    ///
    /// When set, neural coreference resolution runs as a
    /// preprocessing stage before entity extraction, benefiting
    /// all extractor backends.
    /// Env: `COVALENCE_COREF_URL`. Default: none.
    pub coref_url: Option<String>,

    /// Base URL for the PDF-to-Markdown conversion service
    /// (e.g., pymupdf4llm). When set, `application/pdf` content
    /// is converted to Markdown via `POST /convert-pdf`.
    /// Env: `COVALENCE_PDF_URL`. Default: none (disabled).
    pub pdf_url: Option<String>,

    /// Base URL for the ReaderLM-v2 HTML-to-Markdown service.
    ///
    /// When set, HTML content is converted to clean Markdown via
    /// the MLX-based ReaderLM model before parsing. Falls back to
    /// the built-in tag-stripping `HtmlConverter` if unavailable.
    /// Env: `COVALENCE_READERLM_URL`. Default: none (disabled).
    pub readerlm_url: Option<String>,

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

    /// Persistent retry queue configuration.
    pub queue: RetryQueueConfig,

    /// Model for the /ask synthesis endpoint.
    ///
    /// Synthesis benefits from deeper reasoning than extraction, so
    /// this defaults to `"sonnet"` while extraction uses Haiku.
    /// Env: `COVALENCE_ASK_MODEL`. Default: `"sonnet"`.
    pub ask_model: String,

    /// External STDIO service configurations discovered from env
    /// vars.
    ///
    /// Parsed from `COVALENCE_SERVICE_<NAME>_COMMAND` (preferred) or
    /// the legacy `COVALENCE_SIDECAR_<NAME>_COMMAND` (fallback),
    /// plus the optional `_ARGS` suffix (comma-separated).
    pub external_services: Vec<ExternalServiceConfig>,

    /// Metadata schema enforcement level for ingestion.
    ///
    /// Controls how metadata is validated against schemas declared
    /// by extensions. Values: "ignore", "warn" (default), "strict".
    /// Env: `COVALENCE_METADATA_ENFORCEMENT`. Default: `"warn"`.
    pub metadata_enforcement: String,
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

    /// Statement-level embeddings (atomic knowledge claims).
    /// Same dimension as chunks for search parity.
    /// Env: `COVALENCE_EMBED_DIM_STATEMENT`. Default: `1024`.
    pub statement: usize,

    /// Section-level embeddings (compiled statement clusters).
    /// Same dimension as chunks for search parity.
    /// Env: `COVALENCE_EMBED_DIM_SECTION`. Default: `1024`.
    pub section: usize,
}

impl Default for TableDimensions {
    fn default() -> Self {
        Self {
            source: 2048,
            chunk: 1024,
            article: 1024,
            node: 256,
            alias: 256,
            statement: 1024,
            section: 1024,
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
            .max(self.statement)
            .max(self.section)
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

/// Per-stage pipeline configuration.
///
/// Each ingestion pipeline stage can be independently toggled
/// on or off. Windowing parameters for extraction models are
/// also configurable here.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Enable format conversion (HTML→MD, PDF→MD).
    /// Env: `COVALENCE_CONVERT_ENABLED`. Default: `true`.
    pub convert_enabled: bool,

    /// Enable text normalization before chunking.
    /// Env: `COVALENCE_NORMALIZE_ENABLED`. Default: `true`.
    pub normalize_enabled: bool,

    /// Enable neural coreference resolution preprocessing.
    /// Env: `COVALENCE_COREF_ENABLED`. Default: `true`.
    pub coref_enabled: bool,

    /// Enable entity resolution (dedup against existing nodes).
    /// When disabled, every extracted entity creates a new node.
    /// Env: `COVALENCE_RESOLVE_ENABLED`. Default: `true`.
    pub resolve_enabled: bool,

    /// Whether to defer unresolved entities to Tier 5 (HDBSCAN pool).
    /// When enabled, entities that miss all 4 resolution tiers go to
    /// the unresolved_entities table for batch clustering instead of
    /// immediately creating new nodes.
    /// Env: `COVALENCE_TIER5_ENABLED`. Default: `false`.
    pub tier5_enabled: bool,

    /// NER model window size in characters.
    /// Env: `COVALENCE_NER_WINDOW_CHARS`. Default: `1200`.
    pub ner_window_chars: usize,

    /// NER model window overlap in characters.
    /// Env: `COVALENCE_NER_WINDOW_OVERLAP`. Default: `200`.
    pub ner_window_overlap: usize,

    /// Coreference model window size in characters.
    /// Env: `COVALENCE_COREF_WINDOW_CHARS`. Default: `15000`.
    pub coref_window_chars: usize,

    /// Coreference model window overlap in characters.
    /// Env: `COVALENCE_COREF_WINDOW_OVERLAP`. Default: `500`.
    pub coref_window_overlap: usize,

    /// Relationship extraction window size in characters.
    /// Env: `COVALENCE_RE_WINDOW_CHARS`. Default: `15000`.
    pub re_window_chars: usize,

    /// Relationship extraction window overlap in characters.
    /// Env: `COVALENCE_RE_WINDOW_OVERLAP`. Default: `500`.
    pub re_window_overlap: usize,

    /// Enable the statement-first extraction pipeline.
    /// When enabled, atomic statements are extracted from source text
    /// and used as the primary retrieval unit alongside chunks.
    /// Env: `COVALENCE_STATEMENT_ENABLED`. Default: `false`.
    pub statement_enabled: bool,

    /// Statement extraction window size in characters.
    /// Env: `COVALENCE_STATEMENT_WINDOW_CHARS`. Default: `8000`.
    pub statement_window_chars: usize,

    /// Statement extraction window overlap in characters.
    /// Env: `COVALENCE_STATEMENT_WINDOW_OVERLAP`. Default: `1000`.
    pub statement_window_overlap: usize,

    /// Entity class for code entities (ontology-configurable).
    /// Env: `COVALENCE_CODE_ENTITY_CLASS`. Default: `code`.
    pub code_entity_class: String,

    /// Domain for code sources (ontology-configurable).
    /// Env: `COVALENCE_CODE_DOMAIN`. Default: `code`.
    pub code_domain: String,

    /// Node types considered "code" for bridge queries and summary
    /// composition. Comma-separated when set via env var.
    /// Env: `COVALENCE_CODE_NODE_TYPES`. Default:
    /// `struct,function,trait,enum,impl_block,constant,macro,module,class`.
    pub code_node_types: Vec<String>,

    /// Model override for the statement pipeline. When set, the
    /// statement extractor and section compiler use this model
    /// instead of `chat_model`. Useful when the statement pipeline
    /// uses a CLI backend with a different model name format.
    /// Env: `COVALENCE_STATEMENT_MODEL`. Default: `None` (uses
    /// `chat_model`).
    pub statement_model: Option<String>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            convert_enabled: true,
            normalize_enabled: true,
            coref_enabled: true,
            resolve_enabled: true,
            tier5_enabled: false,
            ner_window_chars: 1200,
            ner_window_overlap: 200,
            coref_window_chars: 15_000,
            coref_window_overlap: 500,
            re_window_chars: 15_000,
            re_window_overlap: 500,
            statement_enabled: true,
            statement_window_chars: 8_000,
            statement_window_overlap: 1_000,
            statement_model: None,
            code_entity_class: "code".to_string(),
            code_domain: "code".to_string(),
            code_node_types: Self::default_code_node_types(),
        }
    }
}

impl PipelineConfig {
    /// Default list of node types considered "code" entities.
    pub fn default_code_node_types() -> Vec<String> {
        vec![
            "struct".to_string(),
            "function".to_string(),
            "trait".to_string(),
            "enum".to_string(),
            "impl_block".to_string(),
            "constant".to_string(),
            "macro".to_string(),
            "module".to_string(),
            "class".to_string(),
        ]
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

/// Configuration for the persistent retry queue.
#[derive(Debug, Clone)]
pub struct RetryQueueConfig {
    /// How often to poll for pending jobs, in seconds.
    pub poll_interval_secs: u64,
    /// Base backoff delay for failed jobs, in seconds.
    pub base_backoff_secs: u64,
    /// Maximum backoff delay, in seconds.
    pub max_backoff_secs: u64,
    /// Default max retry attempts per job.
    pub max_attempts: i32,
    /// Max concurrent process_source / reprocess_source jobs.
    pub reprocess_concurrency: usize,
    /// Max concurrent extract_chunk jobs.
    pub extract_concurrency: usize,
    /// Max concurrent summarize_entity jobs.
    pub summarize_concurrency: usize,
    /// Max concurrent compose_source_summary jobs.
    pub compose_concurrency: usize,
    /// Max concurrent edge synthesis jobs.
    pub edge_concurrency: usize,
    /// Max concurrent embed_batch jobs.
    pub embed_concurrency: usize,
    /// Maximum time a single job can run before being timed out, in
    /// seconds. Prevents long-running jobs from holding semaphore
    /// permits indefinitely.
    pub job_timeout_secs: u64,
}

impl Default for RetryQueueConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 5,
            base_backoff_secs: 30,
            max_backoff_secs: 3600,
            max_attempts: 5,
            reprocess_concurrency: 4,
            extract_concurrency: 24,
            summarize_concurrency: 10,
            compose_concurrency: 5,
            edge_concurrency: 1,
            embed_concurrency: 4,
            job_timeout_secs: 600, // 10 minutes
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
            .field("graph_engine", &self.graph_engine)
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
            .field("coref_url", &self.coref_url)
            .field("readerlm_url", &self.readerlm_url)
            .field("consolidation", &self.consolidation)
            .field("search", &self.search)
            .field("pipeline", &self.pipeline)
            .field("resolve_trigram_threshold", &self.resolve_trigram_threshold)
            .field("resolve_vector_threshold", &self.resolve_vector_threshold)
            .field("queue", &self.queue)
            .field("ask_model", &self.ask_model)
            .field("external_services", &self.external_services)
            .field("metadata_enforcement", &self.metadata_enforcement)
            .finish()
    }
}

/// Discover external STDIO service configurations from environment
/// variables.
///
/// Scans for vars matching `COVALENCE_SERVICE_<NAME>_COMMAND`
/// (preferred) and falls back to the legacy
/// `COVALENCE_SIDECAR_<NAME>_COMMAND` prefix for backward
/// compatibility. Pairs them with optional `_ARGS`
/// (comma-separated). Returns a vec of [`ExternalServiceConfig`].
pub(crate) fn parse_service_configs() -> Vec<ExternalServiceConfig> {
    use std::collections::HashMap;

    let prefix = "COVALENCE_SERVICE_";
    let suffix_cmd = "_COMMAND";

    let mut commands: HashMap<String, String> = HashMap::new();

    for (key, value) in std::env::vars() {
        if let Some(rest) = key.strip_prefix(prefix) {
            if let Some(name) = rest.strip_suffix(suffix_cmd) {
                if !name.is_empty() && !value.is_empty() {
                    commands.insert(name.to_string(), value);
                }
            }
        }
    }

    let mut configs: Vec<ExternalServiceConfig> = commands
        .into_iter()
        .map(|(name, command)| {
            let args_key = format!("{prefix}{name}_ARGS");
            let args = optional_env(&args_key)
                .map(|s| {
                    s.split(',')
                        .map(|a| a.trim().to_string())
                        .filter(|a| !a.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            ExternalServiceConfig {
                name: name.to_lowercase(),
                command,
                args,
            }
        })
        .collect();

    // Sort for deterministic ordering.
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    configs
}

/// Read an environment variable, returning `None` if unset or empty.
fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_dimensions_default() {
        let dims = TableDimensions::default();
        assert_eq!(dims.source, 2048);
        assert_eq!(dims.chunk, 1024);
        assert_eq!(dims.article, 1024);
        assert_eq!(dims.node, 256);
        assert_eq!(dims.alias, 256);
        assert_eq!(dims.statement, 1024);
        assert_eq!(dims.section, 1024);
    }

    #[test]
    fn table_dimensions_max_dim() {
        let dims = TableDimensions::default();
        assert_eq!(dims.max_dim(), 2048); // source is largest

        let custom = TableDimensions {
            source: 512,
            chunk: 4096,
            article: 1024,
            node: 256,
            alias: 256,
            statement: 1024,
            section: 1024,
        };
        assert_eq!(custom.max_dim(), 4096); // chunk is largest
    }

    #[test]
    fn embedding_config_max_dim_delegates() {
        let cfg = EmbeddingConfig::default();
        assert_eq!(cfg.max_dim(), cfg.table_dims.max_dim());
    }

    #[test]
    fn search_config_defaults() {
        let cfg = SearchConfig::default();
        assert!((cfg.rrf_k - 60.0).abs() < 1e-10);
        assert_eq!(cfg.default_limit, 10);
        assert!((cfg.abstention_threshold - 0.001).abs() < 1e-10);
    }

    #[test]
    fn consolidation_config_defaults() {
        let cfg = ConsolidationConfig::default();
        assert_eq!(cfg.batch_interval_secs, 300);
        assert_eq!(cfg.deep_interval_secs, 86_400);
        assert!((cfg.delta_threshold - 0.1).abs() < 1e-10);
    }

    #[test]
    fn pipeline_config_defaults() {
        let cfg = PipelineConfig::default();
        assert!(cfg.convert_enabled);
        assert!(cfg.normalize_enabled);
        assert!(cfg.coref_enabled);
        assert!(cfg.resolve_enabled);
        assert!(!cfg.tier5_enabled);
        assert_eq!(cfg.ner_window_chars, 1200);
        assert_eq!(cfg.ner_window_overlap, 200);
    }

    #[test]
    fn parse_service_configs_from_env() {
        let cmd_key = "COVALENCE_SERVICE_MYTOOL_COMMAND";
        let args_key = "COVALENCE_SERVICE_MYTOOL_ARGS";

        // SAFETY: test-only, single-threaded test runner.
        unsafe {
            std::env::set_var(cmd_key, "my-converter");
            std::env::set_var(args_key, "--format,markdown,--strict");
        }

        let configs = parse_service_configs();
        let found = configs.iter().find(|c| c.name == "mytool");
        assert!(found.is_some(), "should find 'mytool' service");
        let sc = found.unwrap();
        assert_eq!(sc.command, "my-converter");
        assert_eq!(sc.args, vec!["--format", "markdown", "--strict"]);

        // Cleanup.
        unsafe {
            std::env::remove_var(cmd_key);
            std::env::remove_var(args_key);
        }
    }

    #[test]
    fn parse_service_configs_no_args() {
        let cmd_key = "COVALENCE_SERVICE_NOTOOL_COMMAND";

        // SAFETY: test-only, single-threaded test runner.
        unsafe {
            std::env::set_var(cmd_key, "simple-tool");
        }

        let configs = parse_service_configs();
        let found = configs.iter().find(|c| c.name == "notool");
        assert!(found.is_some(), "should find 'notool' service");
        let sc = found.unwrap();
        assert_eq!(sc.command, "simple-tool");
        assert!(sc.args.is_empty());

        // Cleanup.
        unsafe {
            std::env::remove_var(cmd_key);
        }
    }

    #[test]
    fn parse_service_configs_name_is_lowercased() {
        let cmd_key = "COVALENCE_SERVICE_UPPER_COMMAND";

        // SAFETY: test-only, single-threaded test runner.
        unsafe {
            std::env::set_var(cmd_key, "upper-tool");
        }

        let configs = parse_service_configs();
        let found = configs.iter().find(|c| c.name == "upper");
        assert!(found.is_some(), "service name should be lowercased");

        // Cleanup.
        unsafe {
            std::env::remove_var(cmd_key);
        }
    }

    #[test]
    fn config_debug_redacts_secrets() {
        // Can't easily construct a full Config without DATABASE_URL,
        // but we can verify the Debug impl redacts API keys by
        // building a minimal Config manually.
        let debug_out = format!(
            "{:?}",
            Config {
                database_url: "postgres://test".into(),
                bind_addr: "0.0.0.0:8431".into(),
                api_key: Some("secret-key".into()),
                openai_api_key: Some("sk-secret".into()),
                openai_base_url: None,
                voyage_api_key: Some("vk-secret".into()),
                voyage_base_url: None,
                graph_engine: "petgraph".into(),
                embed_provider: "openai".into(),
                embed_model: "test-model".into(),
                chat_model: "gpt-4o".into(),
                chat_api_key: None,
                chat_base_url: None,
                chat_backend: "cli".into(),
                chat_cli_command: "gemini".into(),
                chunk_size: 1000,
                chunk_overlap: 200,
                min_section_size: 200,
                embedding: EmbeddingConfig::default(),
                extract_concurrency: 8,
                min_extract_tokens: 30,
                extract_batch_tokens: 2000,
                consolidation: ConsolidationConfig::default(),
                search: SearchConfig::default(),
                pipeline: PipelineConfig::default(),
                entity_extractor: "llm".into(),
                extract_url: None,
                gliner_threshold: 0.5,
                coref_url: None,
                pdf_url: None,
                readerlm_url: None,
                resolve_trigram_threshold: 0.4,
                resolve_vector_threshold: 0.85,
                queue: RetryQueueConfig::default(),
                ask_model: "sonnet".into(),
                external_services: vec![],
                metadata_enforcement: "warn".into(),
            }
        );
        assert!(debug_out.contains("[REDACTED]"));
        assert!(!debug_out.contains("secret-key"));
        assert!(!debug_out.contains("sk-secret"));
        assert!(!debug_out.contains("vk-secret"));
    }
}
