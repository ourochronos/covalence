//! External service health checking utilities.
//!
//! Probes configured service endpoints to determine reachability
//! and generates warnings for unreachable services that will fall
//! back to built-in alternatives.

use serde::Serialize;

use crate::config::Config;

/// Health status of a single external service.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceHealth {
    /// Human-readable service name.
    pub name: String,
    /// Whether the service URL is configured.
    pub configured: bool,
    /// The configured URL, if any.
    pub url: Option<String>,
    /// Whether the service responded to a health probe.
    pub reachable: bool,
    /// Description of the fallback behavior when unreachable.
    pub fallback: Option<String>,
}

/// Result of a full configuration audit.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigAudit {
    /// Summary of the current pipeline configuration.
    pub current_config: serde_json::Value,
    /// Health status of all external services.
    pub services: Vec<ServiceHealth>,
    /// Warnings about potential configuration issues.
    pub warnings: Vec<String>,
}

/// Probe a single URL's `/health` endpoint with a 2-second timeout.
///
/// Returns `true` if the endpoint responds with a 2xx status code.
async fn probe_health(url: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return false,
    };

    let health_url = format!("{}/health", url.trim_end_matches('/'));
    client
        .get(&health_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Check health of all configured external services.
///
/// Probes each service URL's `/health` endpoint with a 2-second
/// timeout. Returns a `ServiceHealth` entry for each known service,
/// regardless of whether it is configured.
pub async fn check_services(config: &Config) -> Vec<ServiceHealth> {
    let entries = vec![
        (
            "readerlm",
            config.readerlm_url.as_deref(),
            "Built-in HTML tag stripper",
        ),
        (
            "coref",
            config.coref_url.as_deref(),
            "Coreference resolution disabled",
        ),
        (
            "extract",
            config.extract_url.as_deref(),
            "LLM-based extraction",
        ),
        ("pdf", config.pdf_url.as_deref(), "PDF ingestion disabled"),
    ];

    let mut results = Vec::with_capacity(entries.len());

    for (name, url, fallback_desc) in entries {
        let (configured, reachable) = match url {
            Some(u) => (true, probe_health(u).await),
            None => (false, false),
        };

        results.push(ServiceHealth {
            name: name.to_string(),
            configured,
            url: url.map(|u| u.to_string()),
            reachable,
            fallback: if configured && !reachable {
                Some(fallback_desc.to_string())
            } else {
                None
            },
        });
    }

    results
}

/// Build a summary of the current pipeline configuration as JSON.
///
/// Intentionally redacts API keys and secrets. Includes the
/// most operationally relevant settings.
pub fn build_config_summary(config: &Config) -> serde_json::Value {
    serde_json::json!({
        "embed_provider": config.embed_provider,
        "embed_model": config.embed_model,
        "chat_model": config.chat_model,
        "entity_extractor": config.entity_extractor,
        "chunk_size": config.chunk_size,
        "chunk_overlap": config.chunk_overlap,
        "extract_concurrency": config.extract_concurrency,
        "min_extract_tokens": config.min_extract_tokens,
        "extract_batch_tokens": config.extract_batch_tokens,
        "gliner_threshold": config.gliner_threshold,
        "resolve_trigram_threshold": config.resolve_trigram_threshold,
        "resolve_vector_threshold": config.resolve_vector_threshold,
        "embedding": {
            "max_dim": config.embedding.max_dim(),
            "source_dim": config.embedding.table_dims.source,
            "chunk_dim": config.embedding.table_dims.chunk,
            "article_dim": config.embedding.table_dims.article,
            "node_dim": config.embedding.table_dims.node,
            "alias_dim": config.embedding.table_dims.alias,
            "batch_size": config.embedding.batch_size,
        },
        "pipeline": {
            "convert_enabled": config.pipeline.convert_enabled,
            "normalize_enabled": config.pipeline.normalize_enabled,
            "coref_enabled": config.pipeline.coref_enabled,
            "resolve_enabled": config.pipeline.resolve_enabled,
            "tier5_enabled": config.pipeline.tier5_enabled,
            "ner_window_chars": config.pipeline.ner_window_chars,
            "ner_window_overlap": config.pipeline.ner_window_overlap,
            "coref_window_chars": config.pipeline.coref_window_chars,
            "coref_window_overlap": config.pipeline.coref_window_overlap,
            "re_window_chars": config.pipeline.re_window_chars,
            "re_window_overlap": config.pipeline.re_window_overlap,
        },
        "search": {
            "rrf_k": config.search.rrf_k,
            "default_limit": config.search.default_limit,
            "abstention_threshold": config.search.abstention_threshold,
        },
        "consolidation": {
            "batch_interval_secs":
                config.consolidation.batch_interval_secs,
            "deep_interval_secs":
                config.consolidation.deep_interval_secs,
            "delta_threshold":
                config.consolidation.delta_threshold,
        },
        "services": {
            "readerlm_url": config.readerlm_url,
            "coref_url": config.coref_url,
            "extract_url": config.extract_url,
            "pdf_url": config.pdf_url,
        },
        "has_openai_key": config.openai_api_key.is_some(),
        "has_voyage_key": config.voyage_api_key.is_some(),
        "has_chat_key": config.chat_api_key.is_some(),
    })
}

/// Generate warnings based on service health and config state.
///
/// Produces human-readable warning strings for cases like:
/// - A service is configured but unreachable
/// - A pipeline stage is enabled but the backing service is missing
/// - Missing API keys that limit functionality
pub fn generate_warnings(config: &Config, services: &[ServiceHealth]) -> Vec<String> {
    let mut warnings = Vec::new();

    for sc in services {
        if sc.configured && !sc.reachable {
            let fallback = sc.fallback.as_deref().unwrap_or("degraded behavior");
            warnings.push(format!(
                "{} service configured at {} but unreachable \
                 — will fall back to: {}",
                sc.name,
                sc.url.as_deref().unwrap_or("unknown"),
                fallback,
            ));
        }
    }

    // Warn if coref is enabled in pipeline but no URL is configured.
    if config.pipeline.coref_enabled && config.coref_url.is_none() {
        warnings.push(
            "Coreference resolution enabled in pipeline config \
             but no COVALENCE_COREF_URL configured"
                .to_string(),
        );
    }

    // Warn if entity extractor requires an extraction service but
    // URL is missing.
    let needs_service = matches!(
        config.entity_extractor.as_str(),
        "gliner2" | "sidecar" | "two_pass"
    );
    if needs_service && config.extract_url.is_none() {
        warnings.push(format!(
            "Entity extractor '{}' requires an extraction service \
             but COVALENCE_EXTRACT_URL is not configured \
             — will use default localhost URL",
            config.entity_extractor,
        ));
    }

    // Warn if no embedding API key is available.
    if config.openai_api_key.is_none() && config.voyage_api_key.is_none() {
        warnings.push(
            "No embedding API key configured (OPENAI_API_KEY or \
             VOYAGE_API_KEY) — embedding and extraction will be \
             disabled"
                .to_string(),
        );
    }

    // Warn if PDF conversion is enabled but no service.
    if config.pipeline.convert_enabled && config.pdf_url.is_none() {
        warnings.push(
            "Format conversion enabled but no PDF service \
             configured (COVALENCE_PDF_URL) — PDF ingestion \
             will fail"
                .to_string(),
        );
    }

    warnings
}

/// Run a full configuration audit.
///
/// Checks all services, builds a config summary, and generates
/// warnings. This is the main entry point for the config audit
/// feature.
pub async fn run_config_audit(config: &Config) -> ConfigAudit {
    let services = check_services(config).await;
    let current_config = build_config_summary(config);
    let warnings = generate_warnings(config, &services);

    ConfigAudit {
        current_config,
        services,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Config, ConsolidationConfig, EmbeddingConfig, PipelineConfig, RetryQueueConfig,
        SearchConfig,
    };

    /// Build a minimal Config for testing without touching env vars.
    fn test_config() -> Config {
        Config {
            database_url: "postgres://test:test@localhost/test".to_string(),
            bind_addr: "0.0.0.0:8431".to_string(),
            api_key: None,
            openai_api_key: Some("sk-test".to_string()),
            openai_base_url: None,
            voyage_api_key: None,
            voyage_base_url: None,
            graph_engine: "petgraph".to_string(),
            embed_provider: "openai".to_string(),
            embed_model: "text-embedding-3-large".to_string(),
            chat_model: "gpt-4o".to_string(),
            chat_api_key: None,
            chat_base_url: None,
            chat_backend: "http".to_string(),
            chat_cli_command: "gemini".to_string(),
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
            entity_extractor: "llm".to_string(),
            extract_url: None,
            gliner_threshold: 0.5,
            coref_url: None,
            pdf_url: None,
            readerlm_url: None,
            resolve_trigram_threshold: 0.4,
            resolve_vector_threshold: 0.85,
            queue: RetryQueueConfig::default(),
            ask_model: "sonnet".to_string(),
            external_services: vec![],
            metadata_enforcement: "warn".to_string(),
            graph_reload_interval_secs: 30,
        }
    }

    #[test]
    fn config_summary_redacts_keys() {
        let mut config = test_config();
        config.openai_api_key = Some("sk-secret-key".to_string());
        config.voyage_api_key = Some("va-secret-key".to_string());

        let summary = build_config_summary(&config);

        // Keys should NOT appear in summary — only boolean
        // indicators.
        let text = serde_json::to_string(&summary).unwrap_or_default();
        assert!(!text.contains("sk-secret-key"));
        assert!(!text.contains("va-secret-key"));
        assert_eq!(summary["has_openai_key"], true);
        assert_eq!(summary["has_voyage_key"], true);
    }

    #[test]
    fn config_summary_includes_pipeline_settings() {
        let config = test_config();
        let summary = build_config_summary(&config);

        assert_eq!(summary["embed_provider"], "openai");
        assert_eq!(summary["entity_extractor"], "llm");
        assert_eq!(summary["chunk_size"], 1000);
        assert_eq!(summary["chunk_overlap"], 200);
        assert_eq!(summary["pipeline"]["coref_enabled"], true);
        assert_eq!(summary["pipeline"]["resolve_enabled"], true);
    }

    #[test]
    fn config_summary_includes_embedding_dims() {
        let config = test_config();
        let summary = build_config_summary(&config);

        assert_eq!(summary["embedding"]["source_dim"], 2048);
        assert_eq!(summary["embedding"]["chunk_dim"], 1024);
        assert_eq!(summary["embedding"]["node_dim"], 256);
        assert_eq!(summary["embedding"]["alias_dim"], 256);
        assert_eq!(summary["embedding"]["max_dim"], 2048);
    }

    #[test]
    fn warnings_for_unreachable_service() {
        let config = test_config();
        let services = vec![ServiceHealth {
            name: "readerlm".to_string(),
            configured: true,
            url: Some("http://localhost:9999".to_string()),
            reachable: false,
            fallback: Some("Built-in HTML tag stripper".to_string()),
        }];

        let warnings = generate_warnings(&config, &services);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("readerlm") && w.contains("unreachable"))
        );
    }

    #[test]
    fn no_warnings_for_reachable_service() {
        let mut config = test_config();
        // Disable coref so the "no coref URL" warning doesn't fire.
        config.pipeline.coref_enabled = false;
        // Set PDF URL so the PDF warning doesn't fire.
        config.pdf_url = Some("http://localhost:8000".to_string());

        let services = vec![ServiceHealth {
            name: "readerlm".to_string(),
            configured: true,
            url: Some("http://localhost:8000".to_string()),
            reachable: true,
            fallback: None,
        }];

        let warnings = generate_warnings(&config, &services);
        assert!(
            !warnings.iter().any(|w| w.contains("readerlm")),
            "should not warn about reachable service"
        );
    }

    #[test]
    fn warning_when_coref_enabled_but_no_url() {
        let mut config = test_config();
        config.pipeline.coref_enabled = true;
        config.coref_url = None;

        let warnings = generate_warnings(&config, &[]);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("Coreference resolution"))
        );
    }

    #[test]
    fn no_coref_warning_when_disabled() {
        let mut config = test_config();
        config.pipeline.coref_enabled = false;
        config.coref_url = None;
        // Set PDF URL to avoid the PDF warning.
        config.pdf_url = Some("http://localhost:8000".to_string());

        let warnings = generate_warnings(&config, &[]);
        assert!(
            !warnings.iter().any(|w| w.contains("Coreference")),
            "should not warn about coref when disabled"
        );
    }

    #[test]
    fn warning_when_no_embedding_keys() {
        let mut config = test_config();
        config.openai_api_key = None;
        config.voyage_api_key = None;

        let warnings = generate_warnings(&config, &[]);
        assert!(warnings.iter().any(|w| w.contains("embedding API key")));
    }

    #[test]
    fn no_key_warning_when_openai_key_present() {
        let mut config = test_config();
        config.openai_api_key = Some("sk-test".to_string());
        config.voyage_api_key = None;
        // Suppress coref and PDF warnings.
        config.pipeline.coref_enabled = false;
        config.pdf_url = Some("http://localhost:8000".to_string());

        let warnings = generate_warnings(&config, &[]);
        assert!(
            !warnings.iter().any(|w| w.contains("embedding API key")),
            "should not warn about keys when OpenAI key is present"
        );
    }

    #[test]
    fn warning_when_extract_service_no_url() {
        let mut config = test_config();
        config.entity_extractor = "gliner2".to_string();
        config.extract_url = None;

        let warnings = generate_warnings(&config, &[]);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("gliner2") && w.contains("COVALENCE_EXTRACT_URL"))
        );
    }

    #[test]
    fn no_service_warning_for_llm_extractor() {
        let mut config = test_config();
        config.entity_extractor = "llm".to_string();
        config.extract_url = None;
        // Suppress coref and PDF warnings.
        config.pipeline.coref_enabled = false;
        config.pdf_url = Some("http://localhost:8000".to_string());

        let warnings = generate_warnings(&config, &[]);
        assert!(
            !warnings.iter().any(|w| w.contains("COVALENCE_EXTRACT_URL")),
            "should not warn about extract URL for LLM extractor"
        );
    }

    #[test]
    fn warning_when_pdf_url_missing() {
        let mut config = test_config();
        config.pipeline.convert_enabled = true;
        config.pdf_url = None;

        let warnings = generate_warnings(&config, &[]);
        assert!(warnings.iter().any(|w| w.contains("PDF service")));
    }

    #[test]
    fn no_pdf_warning_when_convert_disabled() {
        let mut config = test_config();
        config.pipeline.convert_enabled = false;
        config.pdf_url = None;
        // Suppress coref warning.
        config.pipeline.coref_enabled = false;

        let warnings = generate_warnings(&config, &[]);
        assert!(
            !warnings.iter().any(|w| w.contains("PDF service")),
            "should not warn about PDF when conversion disabled"
        );
    }

    #[test]
    fn unconfigured_service_has_no_fallback() {
        let config = test_config();
        let services = vec![ServiceHealth {
            name: "readerlm".to_string(),
            configured: false,
            url: None,
            reachable: false,
            fallback: None,
        }];

        let warnings = generate_warnings(&config, &services);
        // Unconfigured services should not trigger "unreachable"
        // warnings.
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("readerlm") && w.contains("unreachable"))
        );
    }

    #[tokio::test]
    async fn check_services_with_no_config() {
        let config = test_config();
        let results = check_services(&config).await;

        // Should return 4 entries (readerlm, coref, extract, pdf).
        assert_eq!(results.len(), 4);

        for sc in &results {
            assert!(!sc.configured);
            assert!(!sc.reachable);
            assert!(sc.url.is_none());
            assert!(sc.fallback.is_none());
        }
    }

    #[tokio::test]
    async fn check_services_unreachable_url() {
        let mut config = test_config();
        // Use a URL that won't respond.
        config.readerlm_url = Some("http://127.0.0.1:19999".to_string());

        let results = check_services(&config).await;
        let readerlm = results
            .iter()
            .find(|s| s.name == "readerlm")
            .expect("readerlm entry");

        assert!(readerlm.configured);
        assert!(!readerlm.reachable);
        assert!(readerlm.fallback.is_some());
        assert_eq!(readerlm.url.as_deref(), Some("http://127.0.0.1:19999"));
    }

    #[tokio::test]
    async fn run_config_audit_returns_all_fields() {
        let config = test_config();
        let audit = run_config_audit(&config).await;

        assert!(!audit.services.is_empty());
        assert!(audit.current_config.is_object());
        // Should have at least the coref + PDF warnings.
        assert!(!audit.warnings.is_empty());
    }

    #[test]
    fn service_health_serializes_correctly() {
        let health = ServiceHealth {
            name: "readerlm".to_string(),
            configured: true,
            url: Some("http://localhost:8000".to_string()),
            reachable: false,
            fallback: Some("tag stripper".to_string()),
        };

        let json = serde_json::to_value(&health).expect("serialize");
        assert_eq!(json["name"], "readerlm");
        assert_eq!(json["configured"], true);
        assert_eq!(json["reachable"], false);
        assert_eq!(json["url"], "http://localhost:8000");
        assert_eq!(json["fallback"], "tag stripper");
    }

    #[test]
    fn config_audit_serializes_correctly() {
        let audit = ConfigAudit {
            current_config: serde_json::json!({"key": "value"}),
            services: vec![ServiceHealth {
                name: "test".to_string(),
                configured: false,
                url: None,
                reachable: false,
                fallback: None,
            }],
            warnings: vec!["test warning".to_string()],
        };

        let json = serde_json::to_value(&audit).expect("serialize");
        assert_eq!(json["warnings"][0], "test warning");
        assert_eq!(json["services"][0]["name"], "test");
        assert_eq!(json["current_config"]["key"], "value");
    }

    #[test]
    fn multiple_warnings_accumulated() {
        let mut config = test_config();
        config.openai_api_key = None;
        config.voyage_api_key = None;
        config.pipeline.coref_enabled = true;
        config.coref_url = None;
        config.entity_extractor = "gliner2".to_string();
        config.extract_url = None;

        let unreachable = vec![ServiceHealth {
            name: "readerlm".to_string(),
            configured: true,
            url: Some("http://localhost:9999".to_string()),
            reachable: false,
            fallback: Some("fallback".to_string()),
        }];

        let warnings = generate_warnings(&config, &unreachable);
        // Should have at least 4 warnings:
        // 1. readerlm unreachable
        // 2. coref enabled but no URL
        // 3. gliner2 needs extract URL
        // 4. no embedding keys
        // 5. PDF URL missing (convert_enabled=true by default)
        assert!(
            warnings.len() >= 4,
            "expected at least 4 warnings, got {}: {:?}",
            warnings.len(),
            warnings
        );
    }
}
