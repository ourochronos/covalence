//! Layered configuration loader (ADR-0023).
//!
//! Loads config from multiple sources in precedence order:
//! 1. Hardcoded defaults (serialized from [`RawConfig::default()`])
//! 2. `covalence.conf` (base YAML file)
//! 3. `covalence.conf.d/*.conf` (alphabetical, last value wins)
//! 4. `COVALENCE_*` environment variables
//!
//! Override warnings are logged at startup so operators can see
//! exactly which source provided each overridden value.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Yaml};
use serde::{Deserialize, Serialize};

use crate::config::{
    Config, ConsolidationConfig, EmbeddingConfig, ExternalServiceConfig, PipelineConfig,
    RetryQueueConfig, SearchConfig, TableDimensions,
};
use crate::error::{Error, Result};

/// Default base config file name.
const DEFAULT_CONFIG_FILE: &str = "covalence.conf";

/// Default config directory for override fragments.
const DEFAULT_CONFIG_DIR: &str = "covalence.conf.d";

/// Environment variable prefix stripped by figment.
const ENV_PREFIX: &str = "COVALENCE_";

/// Separator for nested keys in environment variables.
///
/// Example: `COVALENCE_EMBEDDING__MODEL` maps to `embedding.model`.
const ENV_SEPARATOR: &str = "__";

// ── Intermediate deserialization types ────────────────────────────

/// Raw deserialization target for figment. Maps to nested YAML
/// structure, then converts to the existing [`Config`] struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RawConfig {
    database: DatabaseConfig,
    bind_addr: String,
    api_key: Option<String>,
    openai_api_key: Option<String>,
    openai_base_url: Option<String>,
    voyage_api_key: Option<String>,
    voyage_base_url: Option<String>,
    graph: GraphConfig,
    embedding: RawEmbeddingConfig,
    chat: ChatConfig,
    chunk_size: usize,
    chunk_overlap: usize,
    min_section_size: usize,
    extract_concurrency: usize,
    min_extract_tokens: usize,
    extract_batch_tokens: usize,
    entity_extractor: String,
    extract_url: Option<String>,
    gliner_threshold: f32,
    coref_url: Option<String>,
    pdf_url: Option<String>,
    readerlm_url: Option<String>,
    resolve_trigram_threshold: f32,
    resolve_vector_threshold: f32,
    consolidation: RawConsolidationConfig,
    search: RawSearchConfig,
    pipeline: RawPipelineConfig,
    queue: RawQueueConfig,
    ask_model: String,
    metadata_enforcement: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct DatabaseConfig {
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct GraphConfig {
    engine: String,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            engine: "petgraph".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RawEmbeddingConfig {
    provider: String,
    model: String,
    batch_size: usize,
    dim_source: usize,
    dim_chunk: usize,
    dim_article: usize,
    dim_node: usize,
    dim_alias: usize,
    dim_statement: usize,
    dim_section: usize,
}

impl Default for RawEmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "text-embedding-3-large".to_string(),
            batch_size: 64,
            dim_source: 2048,
            dim_chunk: 1024,
            dim_article: 1024,
            dim_node: 256,
            dim_alias: 256,
            dim_statement: 1024,
            dim_section: 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct ChatConfig {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    backend: String,
    cli_command: String,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o".to_string(),
            api_key: None,
            base_url: None,
            backend: "cli".to_string(),
            cli_command: "gemini".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RawConsolidationConfig {
    batch_interval_secs: u64,
    deep_interval_secs: u64,
    delta_threshold: f64,
}

impl Default for RawConsolidationConfig {
    fn default() -> Self {
        let d = ConsolidationConfig::default();
        Self {
            batch_interval_secs: d.batch_interval_secs,
            deep_interval_secs: d.deep_interval_secs,
            delta_threshold: d.delta_threshold,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RawSearchConfig {
    rrf_k: f64,
    default_limit: usize,
    abstention_threshold: f64,
}

impl Default for RawSearchConfig {
    fn default() -> Self {
        let d = SearchConfig::default();
        Self {
            rrf_k: d.rrf_k,
            default_limit: d.default_limit,
            abstention_threshold: d.abstention_threshold,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RawPipelineConfig {
    convert_enabled: bool,
    normalize_enabled: bool,
    coref_enabled: bool,
    resolve_enabled: bool,
    tier5_enabled: bool,
    ner_window_chars: usize,
    ner_window_overlap: usize,
    coref_window_chars: usize,
    coref_window_overlap: usize,
    re_window_chars: usize,
    re_window_overlap: usize,
    statement_enabled: bool,
    statement_window_chars: usize,
    statement_window_overlap: usize,
    statement_model: Option<String>,
    code_entity_class: String,
    code_domain: String,
    code_node_types: Vec<String>,
}

impl Default for RawPipelineConfig {
    fn default() -> Self {
        let d = PipelineConfig::default();
        Self {
            convert_enabled: d.convert_enabled,
            normalize_enabled: d.normalize_enabled,
            coref_enabled: d.coref_enabled,
            resolve_enabled: d.resolve_enabled,
            tier5_enabled: d.tier5_enabled,
            ner_window_chars: d.ner_window_chars,
            ner_window_overlap: d.ner_window_overlap,
            coref_window_chars: d.coref_window_chars,
            coref_window_overlap: d.coref_window_overlap,
            re_window_chars: d.re_window_chars,
            re_window_overlap: d.re_window_overlap,
            statement_enabled: d.statement_enabled,
            statement_window_chars: d.statement_window_chars,
            statement_window_overlap: d.statement_window_overlap,
            statement_model: d.statement_model,
            code_entity_class: d.code_entity_class,
            code_domain: d.code_domain,
            code_node_types: d.code_node_types,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct RawQueueConfig {
    poll_interval_secs: u64,
    base_backoff_secs: u64,
    max_backoff_secs: u64,
    max_attempts: i32,
    reprocess_concurrency: usize,
    extract_concurrency: usize,
    summarize_concurrency: usize,
    compose_concurrency: usize,
    edge_concurrency: usize,
    embed_concurrency: usize,
    job_timeout_secs: u64,
}

impl Default for RawQueueConfig {
    fn default() -> Self {
        let d = RetryQueueConfig::default();
        Self {
            poll_interval_secs: d.poll_interval_secs,
            base_backoff_secs: d.base_backoff_secs,
            max_backoff_secs: d.max_backoff_secs,
            max_attempts: d.max_attempts,
            reprocess_concurrency: d.reprocess_concurrency,
            extract_concurrency: d.extract_concurrency,
            summarize_concurrency: d.summarize_concurrency,
            compose_concurrency: d.compose_concurrency,
            edge_concurrency: d.edge_concurrency,
            embed_concurrency: d.embed_concurrency,
            job_timeout_secs: d.job_timeout_secs,
        }
    }
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            bind_addr: "0.0.0.0:8431".to_string(),
            api_key: None,
            openai_api_key: None,
            openai_base_url: None,
            voyage_api_key: None,
            voyage_base_url: None,
            graph: GraphConfig::default(),
            embedding: RawEmbeddingConfig::default(),
            chat: ChatConfig::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: 200,
            extract_concurrency: 8,
            min_extract_tokens: 30,
            extract_batch_tokens: 2000,
            entity_extractor: "llm".to_string(),
            extract_url: None,
            gliner_threshold: 0.5,
            coref_url: None,
            pdf_url: None,
            readerlm_url: None,
            resolve_trigram_threshold: 0.4,
            resolve_vector_threshold: 0.85,
            consolidation: RawConsolidationConfig::default(),
            search: RawSearchConfig::default(),
            pipeline: RawPipelineConfig::default(),
            queue: RawQueueConfig::default(),
            ask_model: "sonnet".to_string(),
            metadata_enforcement: "warn".to_string(),
        }
    }
}

// ── Conversion from RawConfig to Config ──────────────────────────

impl RawConfig {
    /// Convert the deserialized raw config into the engine [`Config`].
    ///
    /// Validates required fields and applies clamping rules that the
    /// existing `Config::from_env()` enforces.
    fn into_config(self) -> Result<Config> {
        if self.database.url.is_empty() {
            return Err(Error::Config(
                "database.url is required (set in covalence.conf or \
                 COVALENCE_DATABASE__URL)"
                    .to_string(),
            ));
        }

        // Validate clamped float fields.
        validate_range("gliner_threshold", self.gliner_threshold, 0.0, 1.0)?;
        validate_range(
            "resolve_trigram_threshold",
            self.resolve_trigram_threshold,
            0.0,
            1.0,
        )?;
        validate_range(
            "resolve_vector_threshold",
            self.resolve_vector_threshold,
            0.0,
            1.0,
        )?;

        Ok(Config {
            database_url: self.database.url,
            bind_addr: self.bind_addr,
            api_key: self.api_key,
            openai_api_key: self.openai_api_key,
            openai_base_url: self.openai_base_url,
            voyage_api_key: self.voyage_api_key,
            voyage_base_url: self.voyage_base_url,
            graph_engine: self.graph.engine,
            embed_provider: self.embedding.provider,
            embed_model: self.embedding.model.clone(),
            chat_model: self.chat.model,
            chat_api_key: self.chat.api_key,
            chat_base_url: self.chat.base_url,
            chat_backend: self.chat.backend,
            chat_cli_command: self.chat.cli_command,
            chunk_size: self.chunk_size,
            chunk_overlap: self.chunk_overlap,
            min_section_size: self.min_section_size,
            embedding: EmbeddingConfig {
                model: self.embedding.model,
                batch_size: self.embedding.batch_size,
                table_dims: TableDimensions {
                    source: self.embedding.dim_source,
                    chunk: self.embedding.dim_chunk,
                    article: self.embedding.dim_article,
                    node: self.embedding.dim_node,
                    alias: self.embedding.dim_alias,
                    statement: self.embedding.dim_statement,
                    section: self.embedding.dim_section,
                },
            },
            extract_concurrency: self.extract_concurrency,
            min_extract_tokens: self.min_extract_tokens,
            extract_batch_tokens: self.extract_batch_tokens,
            entity_extractor: self.entity_extractor,
            extract_url: self.extract_url,
            gliner_threshold: self.gliner_threshold,
            coref_url: self.coref_url,
            pdf_url: self.pdf_url,
            readerlm_url: self.readerlm_url,
            resolve_trigram_threshold: self.resolve_trigram_threshold,
            resolve_vector_threshold: self.resolve_vector_threshold,
            consolidation: ConsolidationConfig {
                batch_interval_secs: self.consolidation.batch_interval_secs,
                deep_interval_secs: self.consolidation.deep_interval_secs,
                delta_threshold: self.consolidation.delta_threshold,
            },
            search: SearchConfig {
                rrf_k: self.search.rrf_k,
                default_limit: self.search.default_limit,
                abstention_threshold: self.search.abstention_threshold,
            },
            pipeline: PipelineConfig {
                convert_enabled: self.pipeline.convert_enabled,
                normalize_enabled: self.pipeline.normalize_enabled,
                coref_enabled: self.pipeline.coref_enabled,
                resolve_enabled: self.pipeline.resolve_enabled,
                tier5_enabled: self.pipeline.tier5_enabled,
                ner_window_chars: self.pipeline.ner_window_chars,
                ner_window_overlap: self.pipeline.ner_window_overlap,
                coref_window_chars: self.pipeline.coref_window_chars,
                coref_window_overlap: self.pipeline.coref_window_overlap,
                re_window_chars: self.pipeline.re_window_chars,
                re_window_overlap: self.pipeline.re_window_overlap,
                statement_enabled: self.pipeline.statement_enabled,
                statement_window_chars: self.pipeline.statement_window_chars,
                statement_window_overlap: self.pipeline.statement_window_overlap,
                statement_model: self.pipeline.statement_model,
                code_entity_class: self.pipeline.code_entity_class,
                code_domain: self.pipeline.code_domain,
                code_node_types: self.pipeline.code_node_types,
            },
            queue: RetryQueueConfig {
                poll_interval_secs: self.queue.poll_interval_secs,
                base_backoff_secs: self.queue.base_backoff_secs,
                max_backoff_secs: self.queue.max_backoff_secs,
                max_attempts: self.queue.max_attempts,
                reprocess_concurrency: self.queue.reprocess_concurrency,
                extract_concurrency: self.queue.extract_concurrency,
                summarize_concurrency: self.queue.summarize_concurrency,
                compose_concurrency: self.queue.compose_concurrency,
                edge_concurrency: self.queue.edge_concurrency,
                embed_concurrency: self.queue.embed_concurrency,
                job_timeout_secs: self.queue.job_timeout_secs,
            },
            ask_model: self.ask_model,
            // External services are only discovered from env vars
            // (COVALENCE_SERVICE_<NAME>_COMMAND). The figment loader
            // does not model them in YAML — they remain env-only.
            external_services: parse_service_configs_from_env(),
            metadata_enforcement: self.metadata_enforcement,
        })
    }
}

/// Validate that a float value is within `[min, max]`.
fn validate_range(name: &str, value: f32, min: f32, max: f32) -> Result<()> {
    if value < min || value > max {
        return Err(Error::Config(format!(
            "{name} must be in [{min}, {max}], got {value}"
        )));
    }
    Ok(())
}

// ── Public API ───────────────────────────────────────────────────

/// Load configuration using the layered figment system (ADR-0023).
///
/// Sources are merged in precedence order (lowest to highest):
/// 1. Hardcoded defaults
/// 2. `config_path` (default: `covalence.conf`)
/// 3. Files in `config_dir` (default: `covalence.conf.d/`),
///    sorted alphabetically
/// 4. `COVALENCE_*` environment variables
///
/// If no config file exists, the loader falls back gracefully to
/// defaults + env vars, matching the existing `Config::from_env()`
/// behavior.
pub fn load_config(config_path: Option<&str>, config_dir: Option<&str>) -> Result<Config> {
    let figment = build_figment(config_path, config_dir)?;

    // Log which sources contributed overrides.
    log_overrides(&figment);

    let raw: RawConfig = figment
        .extract()
        .map_err(|e| Error::Config(format!("config extraction failed: {e}")))?;

    raw.into_config()
}

/// Build the [`Figment`] instance with all layered providers.
///
/// Exposed separately so callers can inspect metadata without
/// extracting a config (useful for diagnostics / admin endpoints).
pub fn build_figment(config_path: Option<&str>, config_dir: Option<&str>) -> Result<Figment> {
    let conf_file = config_path.unwrap_or(DEFAULT_CONFIG_FILE);
    let conf_dir = config_dir.unwrap_or(DEFAULT_CONFIG_DIR);

    // 1. Hardcoded defaults.
    let mut figment = Figment::from(Serialized::defaults(RawConfig::default()));

    // 2. Base config file (optional — missing file is not an error).
    let conf_path = Path::new(conf_file);
    if conf_path.exists() {
        tracing::info!(path = %conf_path.display(), "loading base config");
        figment = figment.merge(Yaml::file(conf_path));
    } else {
        tracing::debug!(
            path = %conf_path.display(),
            "base config file not found, using defaults"
        );
    }

    // 3. Config directory fragments (alphabetical order).
    let dir_path = Path::new(conf_dir);
    if dir_path.is_dir() {
        let fragments = collect_conf_files(dir_path)?;
        for frag in &fragments {
            tracing::info!(
                path = %frag.display(),
                "loading config fragment"
            );
            figment = figment.merge(Yaml::file(frag));
        }
    }

    // 4. Environment variable overrides.
    figment = figment.merge(
        Env::prefixed(ENV_PREFIX)
            .split(ENV_SEPARATOR)
            .lowercase(true),
    );

    // Also support the legacy non-prefixed DATABASE_URL env var by
    // mapping it into the nested `database.url` key.
    if let Ok(db_url) = std::env::var("DATABASE_URL") {
        if !db_url.is_empty() {
            figment = figment.merge(Serialized::default("database.url", &db_url));
        }
    }

    Ok(figment)
}

/// Collect `.conf`, `.yaml`, and `.yml` files from a directory,
/// sorted alphabetically by filename.
fn collect_conf_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = Vec::new();

    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::Config(format!("failed to read config dir {}: {e}", dir.display())))?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            Error::Config(format!("failed to read entry in {}: {e}", dir.display()))
        })?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "conf" | "yaml" | "yml") {
                    files.push(path);
                }
            }
        }
    }

    // Sort by filename for deterministic ordering.
    files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });

    Ok(files)
}

/// Log override warnings for keys that were set by multiple sources.
///
/// Iterates figment's metadata to identify non-default providers and
/// logs each one so operators can trace where values came from.
fn log_overrides(figment: &Figment) {
    // Collect unique non-default source names.
    let mut sources: BTreeMap<String, ()> = BTreeMap::new();

    for metadata in figment.metadata() {
        // Skip the default serialized provider.
        if metadata.name == "default serialized value"
            || metadata.name.starts_with("Rust serialization")
        {
            continue;
        }
        let source_name = if let Some(ref src) = metadata.source {
            format!("{} ({})", metadata.name, src)
        } else {
            metadata.name.to_string()
        };
        sources.insert(source_name, ());
    }

    if sources.is_empty() {
        tracing::info!("config: using defaults only (no overrides)");
        return;
    }

    for source in sources.keys() {
        tracing::info!(source = %source, "config: loaded from source");
    }
}

/// Re-export of the env-based service config parser from
/// `config.rs`. External services use a dynamic naming pattern
/// (`COVALENCE_SERVICE_<NAME>_COMMAND`) that figment's flat env
/// mapping cannot express, so they remain env-var-only.
fn parse_service_configs_from_env() -> Vec<ExternalServiceConfig> {
    crate::config::parse_service_configs()
}

// ── Config entry point ───────────────────────────────────────────

impl Config {
    /// Load configuration from the layered figment system.
    ///
    /// This is the figment-based alternative to [`Config::from_env()`].
    /// If no `covalence.conf` file exists, it degrades gracefully to
    /// defaults + environment variables.
    pub fn from_figment(config_path: Option<&str>, config_dir: Option<&str>) -> Result<Self> {
        load_config(config_path, config_dir)
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: create a YAML config file in a directory.
    fn write_yaml(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn defaults_produce_valid_raw_config() {
        let raw = RawConfig::default();
        // Should fail because database.url is empty.
        let result = raw.into_config();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("database.url"),
            "error should mention database.url: {err}"
        );
    }

    #[test]
    fn load_from_single_yaml_file() {
        // Use figment::Jail for env var isolation.
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.conf",
                r#"
database:
  url: postgres://test:test@localhost:5432/testdb

bind_addr: "127.0.0.1:9999"

embedding:
  provider: voyage
  model: voyage-3-large
  dim_node: 512

chat:
  model: haiku
  backend: cli
  cli_command: claude

graph:
  engine: age
"#,
            )?;

            let conf_path = jail.directory().join("test.conf");
            let config =
                load_config(Some(conf_path.to_str().unwrap()), Some("/nonexistent/dir")).unwrap();

            assert_eq!(
                config.database_url,
                "postgres://test:test@localhost:5432/testdb"
            );
            assert_eq!(config.bind_addr, "127.0.0.1:9999");
            assert_eq!(config.embed_provider, "voyage");
            assert_eq!(config.embed_model, "voyage-3-large");
            assert_eq!(config.embedding.table_dims.node, 512);
            assert_eq!(config.chat_model, "haiku");
            assert_eq!(config.chat_backend, "cli");
            assert_eq!(config.chat_cli_command, "claude");
            assert_eq!(config.graph_engine, "age");
            // Unspecified fields use defaults.
            assert_eq!(config.chunk_size, 1000);
            assert_eq!(config.embedding.table_dims.source, 2048);

            Ok(())
        });
    }

    #[test]
    fn conf_d_merging_last_wins() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "base.conf",
                r#"
database:
  url: postgres://base@localhost/db

bind_addr: "0.0.0.0:8000"
chunk_size: 500
"#,
            )?;

            let conf_d = jail.directory().join("conf.d");
            std::fs::create_dir(&conf_d).unwrap();

            write_yaml(
                &conf_d,
                "10-first.conf",
                "chunk_size: 800\nask_model: gemini\n",
            );

            write_yaml(
                &conf_d,
                "20-second.conf",
                "chunk_size: 1200\nbind_addr: \"0.0.0.0:9090\"\n",
            );

            let base_path = jail.directory().join("base.conf");
            let config = load_config(
                Some(base_path.to_str().unwrap()),
                Some(conf_d.to_str().unwrap()),
            )
            .unwrap();

            // 20-second.conf overrides 10-first.conf for chunk_size.
            assert_eq!(config.chunk_size, 1200);
            // 20-second.conf overrides base for bind_addr.
            assert_eq!(config.bind_addr, "0.0.0.0:9090");
            // 10-first.conf sets ask_model, not overridden.
            assert_eq!(config.ask_model, "gemini");
            // Base sets database.url.
            assert_eq!(config.database_url, "postgres://base@localhost/db");

            Ok(())
        });
    }

    #[test]
    fn env_var_overrides_file() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.conf",
                r#"
database:
  url: postgres://file@localhost/db

chunk_size: 500
"#,
            )?;

            jail.set_env("COVALENCE_CHUNK_SIZE", "2000");

            let conf_path = jail.directory().join("test.conf");
            let config =
                load_config(Some(conf_path.to_str().unwrap()), Some("/nonexistent")).unwrap();

            assert_eq!(config.chunk_size, 2000);
            // File value for database.url still applies.
            assert_eq!(config.database_url, "postgres://file@localhost/db");

            Ok(())
        });
    }

    #[test]
    fn nested_env_var_override() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.conf",
                r#"
database:
  url: postgres://file@localhost/db

embedding:
  provider: openai
  model: text-embedding-3-large
"#,
            )?;

            jail.set_env("COVALENCE_EMBEDDING__PROVIDER", "voyage");

            let conf_path = jail.directory().join("test.conf");
            let config =
                load_config(Some(conf_path.to_str().unwrap()), Some("/nonexistent")).unwrap();

            assert_eq!(config.embed_provider, "voyage");

            Ok(())
        });
    }

    #[test]
    fn graceful_fallback_no_conf_file() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("COVALENCE_DATABASE__URL", "postgres://env@localhost/db");

            let config = load_config(
                Some("/nonexistent/covalence.conf"),
                Some("/nonexistent/conf.d"),
            )
            .unwrap();

            assert_eq!(config.database_url, "postgres://env@localhost/db");
            // All other values are defaults.
            assert_eq!(config.chunk_size, 1000);

            Ok(())
        });
    }

    #[test]
    fn collect_conf_files_filters_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        write_yaml(dir, "20-second.conf", "chunk_size: 1");
        write_yaml(dir, "10-first.yaml", "chunk_size: 2");
        write_yaml(dir, "30-third.yml", "chunk_size: 3");
        write_yaml(dir, "ignored.txt", "chunk_size: 4");
        write_yaml(dir, "also-ignored.json", "chunk_size: 5");

        let files = collect_conf_files(dir).unwrap();
        assert_eq!(files.len(), 3);

        let names: Vec<&str> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["10-first.yaml", "20-second.conf", "30-third.yml"]
        );
    }

    #[test]
    fn validate_range_rejects_out_of_bounds() {
        assert!(validate_range("test", 0.5, 0.0, 1.0).is_ok());
        assert!(validate_range("test", 0.0, 0.0, 1.0).is_ok());
        assert!(validate_range("test", 1.0, 0.0, 1.0).is_ok());
        assert!(validate_range("test", -0.1, 0.0, 1.0).is_err());
        assert!(validate_range("test", 1.1, 0.0, 1.0).is_err());
    }

    #[test]
    fn pipeline_defaults_preserved() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.conf",
                "database:\n  url: postgres://test@localhost/db\n",
            )?;

            let conf_path = jail.directory().join("test.conf");
            let config =
                load_config(Some(conf_path.to_str().unwrap()), Some("/nonexistent")).unwrap();

            assert!(config.pipeline.convert_enabled);
            assert!(config.pipeline.normalize_enabled);
            assert!(config.pipeline.statement_enabled);
            assert!(!config.pipeline.tier5_enabled);
            assert_eq!(config.pipeline.ner_window_chars, 1200);
            assert_eq!(
                config.pipeline.code_node_types,
                PipelineConfig::default_code_node_types()
            );

            Ok(())
        });
    }

    #[test]
    fn queue_config_from_yaml() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.conf",
                r#"
database:
  url: postgres://test@localhost/db

queue:
  poll_interval_secs: 10
  extract_concurrency: 48
  job_timeout_secs: 1200
"#,
            )?;

            let conf_path = jail.directory().join("test.conf");
            let config =
                load_config(Some(conf_path.to_str().unwrap()), Some("/nonexistent")).unwrap();

            assert_eq!(config.queue.poll_interval_secs, 10);
            assert_eq!(config.queue.extract_concurrency, 48);
            assert_eq!(config.queue.job_timeout_secs, 1200);
            // Defaults for unspecified fields.
            assert_eq!(config.queue.max_attempts, 5);
            assert_eq!(config.queue.base_backoff_secs, 30);

            Ok(())
        });
    }

    #[test]
    fn ui_overrides_file_wins_over_earlier_fragments() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "base.conf",
                "database:\n  url: postgres://test@localhost/db\nchunk_size: 500\n",
            )?;

            let conf_d = jail.directory().join("conf.d");
            std::fs::create_dir(&conf_d).unwrap();

            write_yaml(&conf_d, "10-extension.conf", "chunk_size: 800\n");
            write_yaml(&conf_d, "99-ui-overrides.conf", "chunk_size: 1500\n");

            let base_path = jail.directory().join("base.conf");
            let config = load_config(
                Some(base_path.to_str().unwrap()),
                Some(conf_d.to_str().unwrap()),
            )
            .unwrap();

            assert_eq!(config.chunk_size, 1500);

            Ok(())
        });
    }

    #[test]
    fn legacy_database_url_env_var() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "test.conf",
                "database:\n  url: postgres://file@localhost/db\n",
            )?;

            // The legacy DATABASE_URL env var should override the file.
            jail.set_env("DATABASE_URL", "postgres://legacy@localhost/db");

            let conf_path = jail.directory().join("test.conf");
            let config =
                load_config(Some(conf_path.to_str().unwrap()), Some("/nonexistent")).unwrap();

            assert_eq!(config.database_url, "postgres://legacy@localhost/db");

            Ok(())
        });
    }
}
