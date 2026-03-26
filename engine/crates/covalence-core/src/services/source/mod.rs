//! Source service — ingestion orchestration and source management.
//!
//! The shared pipeline stages (chunk → embed → extract → resolve)
//! live in [`super::pipeline`]. This module owns source lifecycle
//! (create, supersede, reprocess, delete) and delegates the heavy
//! lifting.

mod crud;
mod ingest;
mod reprocess;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{PipelineConfig, TableDimensions};
use crate::ingestion::ChatBackend;
use crate::ingestion::converter::ConverterRegistry;
use crate::ingestion::coreference::FastcorefClient;
use crate::ingestion::embedder::Embedder;
use crate::ingestion::extractor::Extractor;
use crate::ingestion::fingerprint::{FingerprintConfig, PipelineFingerprint};
use crate::ingestion::pg_resolver::PgResolver;
use crate::ingestion::resolver::EntityResolver;
use crate::ingestion::section_compiler::{SectionCompiler, SourceSummaryCompiler};
use crate::ingestion::statement_extractor::StatementExtractor;
use crate::services::adapter_service::AdapterService;
use crate::services::hooks::HookService;
use crate::storage::postgres::PgRepo;

pub use crud::DeleteResult;
pub use reprocess::ReprocessResult;

/// Derive the knowledge domain from a source's type and URI.
///
/// Rules are applied in priority order:
/// 1. Code sources → `code`
/// 2. URI pattern matching for file:// paths
/// 3. HTTP/HTTPS defaults to `research`
/// 4. Remaining documents → `external`
///
/// Returns a single domain string. Use [`derive_domains`] for
/// multi-domain classification.
#[allow(deprecated)]
pub fn derive_domain(source_type: &str, uri: Option<&str>) -> Option<String> {
    derive_domains_hardcoded(source_type, uri)
        .into_iter()
        .next()
}

/// Derive knowledge domains from a source's type and URI using
/// hardcoded fallback rules.
///
/// Returns a vec of matching domain IDs. This is the legacy
/// fallback used when no DB rules or adapters match.
#[deprecated(note = "use derive_domains_via_adapter() which uses DB rules from extensions")]
pub fn derive_domains_hardcoded(source_type: &str, uri: Option<&str>) -> Vec<String> {
    // Code source type takes priority
    if source_type == "code" {
        return vec!["code".to_string()];
    }

    let uri = match uri {
        Some(u) => u,
        None => return Vec::new(),
    };

    // File URI patterns
    if uri.starts_with("file://spec/") {
        return vec!["spec".to_string()];
    }
    if uri.starts_with("file://docs/adr/")
        || uri.starts_with("file://VISION")
        || uri.starts_with("file://CLAUDE")
        || uri.starts_with("file://MILESTONES")
        || uri.starts_with("file://design/")
    {
        return vec!["design".to_string()];
    }
    if uri.starts_with("file://engine/")
        || uri.starts_with("file://cli/")
        || uri.starts_with("file://dashboard/")
    {
        return vec!["code".to_string()];
    }

    // HTTP sources
    if uri.starts_with("https://arxiv") || uri.starts_with("https://doi") {
        return vec!["research".to_string()];
    }
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return vec!["research".to_string()];
    }

    // Remaining documents
    if source_type == "document" {
        return vec!["external".to_string()];
    }

    Vec::new()
}

/// Service for source ingestion and management.
pub struct SourceService {
    pub(crate) repo: Arc<PgRepo>,
    pub(crate) embedder: Option<Arc<dyn Embedder>>,
    pub(crate) extractor: Option<Arc<dyn Extractor>>,
    pub(crate) resolver: Option<Arc<dyn EntityResolver>>,
    pub(crate) extract_concurrency: usize,
    pub(crate) rel_type_resolver: Option<Arc<PgResolver>>,
    pub(crate) converter_registry: Option<ConverterRegistry>,
    pub(crate) coref_client: Option<Arc<FastcorefClient>>,
    pub(crate) table_dims: TableDimensions,
    pub(crate) chunk_size: usize,
    pub(crate) chunk_overlap: usize,
    pub(crate) min_section_size: usize,
    pub(crate) min_extract_tokens: usize,
    pub(crate) extract_batch_tokens: usize,
    pub(crate) pipeline: PipelineConfig,
    pub(crate) fingerprint_config: Option<FingerprintConfig>,
    pub(crate) statement_extractor: Option<Arc<dyn StatementExtractor>>,
    pub(crate) section_compiler: Option<Arc<dyn SectionCompiler>>,
    pub(crate) source_summary_compiler: Option<Arc<dyn SourceSummaryCompiler>>,
    /// Chat backend for generating semantic summaries of code entities.
    pub(crate) chat_backend: Option<Arc<dyn ChatBackend>>,
    /// Adapter service for config-driven domain classification.
    pub(crate) adapter_service: Option<Arc<AdapterService>>,
    /// Domain-specific extractors registered by extensions.
    ///
    /// Key is the domain name (e.g. "code"), value is an extractor
    /// that handles sources in that domain.
    pub(crate) domain_extractors: HashMap<String, Arc<dyn Extractor>>,
    /// Lifecycle hook service for pipeline extensibility.
    pub(crate) hook_service: Option<Arc<HookService>>,
}

impl SourceService {
    /// Default extraction concurrency when not configured.
    const DEFAULT_EXTRACT_CONCURRENCY: usize = 8;
    /// Default minimum section size for sibling merging.
    ///
    /// Sections below this threshold are merged with consecutive
    /// siblings sharing the same parent heading. Prevents tiny
    /// H3/H4 subsections in academic papers from producing chunks
    /// too small for meaningful retrieval.
    const DEFAULT_MIN_SECTION_SIZE: usize = 200;
    /// Default minimum token count for extraction.
    const DEFAULT_MIN_EXTRACT_TOKENS: usize = 30;
    /// Default token budget for extraction batching.
    const DEFAULT_EXTRACT_BATCH_TOKENS: usize = 2000;

    /// Create a new source service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self {
            repo,
            embedder: None,
            extractor: None,
            resolver: None,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver: None,
            converter_registry: None,
            coref_client: None,
            table_dims: TableDimensions::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: Self::DEFAULT_MIN_SECTION_SIZE,
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
            statement_extractor: None,
            section_compiler: None,
            chat_backend: None,
            source_summary_compiler: None,
            adapter_service: None,
            domain_extractors: HashMap::new(),
            hook_service: None,
        }
    }

    /// Create a new source service with optional AI components.
    pub fn with_ai(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver: None,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver: None,
            converter_registry: None,
            coref_client: None,
            table_dims: TableDimensions::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: Self::DEFAULT_MIN_SECTION_SIZE,
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
            statement_extractor: None,
            section_compiler: None,
            chat_backend: None,
            source_summary_compiler: None,
            adapter_service: None,
            domain_extractors: HashMap::new(),
            hook_service: None,
        }
    }

    /// Create a new source service with full AI pipeline.
    pub fn with_full_pipeline(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
        resolver: Option<Arc<dyn EntityResolver>>,
        rel_type_resolver: Option<Arc<PgResolver>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver,
            converter_registry: None,
            coref_client: None,
            table_dims: TableDimensions::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: Self::DEFAULT_MIN_SECTION_SIZE,
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
            statement_extractor: None,
            section_compiler: None,
            chat_backend: None,
            source_summary_compiler: None,
            adapter_service: None,
            domain_extractors: HashMap::new(),
            hook_service: None,
        }
    }

    /// Attach a converter registry for pre-processing content
    /// before the parser stage.
    ///
    /// When set, the `ingest` method will run incoming content
    /// through the matching converter to produce Markdown before
    /// passing it to the parser.
    pub fn with_converter_registry(mut self, registry: ConverterRegistry) -> Self {
        self.converter_registry = Some(registry);
        self
    }

    /// Attach a Fastcoref client for neural coreference resolution
    /// as a preprocessing step before entity extraction.
    pub fn with_coref_client(mut self, client: Arc<FastcorefClient>) -> Self {
        self.coref_client = Some(client);
        self
    }

    /// Set per-table embedding dimensions.
    pub fn with_table_dims(mut self, dims: TableDimensions) -> Self {
        self.table_dims = dims;
        self
    }

    /// Set chunk size, overlap, and minimum section size.
    pub fn with_chunk_config(mut self, size: usize, overlap: usize) -> Self {
        self.chunk_size = size;
        self.chunk_overlap = overlap;
        self
    }

    /// Set minimum section size for sibling merging.
    pub fn with_min_section_size(mut self, min_size: usize) -> Self {
        self.min_section_size = min_size;
        self
    }

    /// Set the maximum number of concurrent LLM extraction calls.
    pub fn with_extract_concurrency(mut self, concurrency: usize) -> Self {
        self.extract_concurrency = concurrency;
        self
    }

    /// Set per-stage pipeline configuration.
    pub fn with_pipeline_config(mut self, config: PipelineConfig) -> Self {
        self.pipeline = config;
        self
    }

    /// Set extraction batching parameters.
    ///
    /// - `min_tokens`: chunks with fewer tokens are skipped.
    /// - `batch_tokens`: adjacent small chunks are concatenated
    ///   up to this token budget for a single extraction call.
    pub fn with_extract_batch_config(mut self, min_tokens: usize, batch_tokens: usize) -> Self {
        self.min_extract_tokens = min_tokens;
        self.extract_batch_tokens = batch_tokens;
        self
    }

    /// Set the pipeline fingerprint configuration.
    ///
    /// When set, every `ingest()` and `reprocess()` call records
    /// the pipeline fingerprint in the source's metadata under
    /// the `"pipeline_fingerprint"` key.
    pub fn with_fingerprint_config(mut self, config: FingerprintConfig) -> Self {
        self.fingerprint_config = Some(config);
        self
    }

    /// Replace or set the entity/relationship extractor.
    pub fn with_extractor(mut self, extractor: Arc<dyn Extractor>) -> Self {
        self.extractor = Some(extractor);
        self
    }

    /// Set the statement extractor for the statement-first pipeline.
    pub fn with_statement_extractor(mut self, extractor: Arc<dyn StatementExtractor>) -> Self {
        self.statement_extractor = Some(extractor);
        self
    }

    /// Set the section compiler for clustering and compilation.
    pub fn with_section_compiler(mut self, compiler: Arc<dyn SectionCompiler>) -> Self {
        self.section_compiler = Some(compiler);
        self
    }

    /// Set the source summary compiler.
    pub fn with_source_summary_compiler(
        mut self,
        compiler: Arc<dyn SourceSummaryCompiler>,
    ) -> Self {
        self.source_summary_compiler = Some(compiler);
        self
    }

    /// Set the chat backend for generating semantic summaries of code
    /// entities (Spec 12, Stage 2).
    pub fn with_chat_backend(mut self, backend: Arc<dyn ChatBackend>) -> Self {
        self.chat_backend = Some(backend);
        self
    }

    /// Set the adapter service for config-driven domain classification.
    ///
    /// When present, [`derive_domain_via_adapter`](Self::derive_domain_via_adapter)
    /// queries adapter configs before falling back to hardcoded
    /// pattern matching.
    pub fn with_adapter_service(mut self, svc: Arc<AdapterService>) -> Self {
        self.adapter_service = Some(svc);
        self
    }

    /// Register a domain-specific extractor.
    ///
    /// Sources whose domain matches `domain` will use this extractor
    /// instead of the default. Registered by extensions via
    /// `ServiceDef.extractor_for`.
    pub fn with_domain_extractor(mut self, domain: String, extractor: Arc<dyn Extractor>) -> Self {
        self.domain_extractors.insert(domain, extractor);
        self
    }

    /// Set the lifecycle hook service for pipeline extensibility.
    pub fn with_hook_service(mut self, svc: Arc<HookService>) -> Self {
        self.hook_service = Some(svc);
        self
    }

    /// Derive the knowledge domain using adapters first, then
    /// hardcoded patterns as fallback.
    ///
    /// Priority:
    /// 1. Adapter match (domain → MIME → URI regex) — uses
    ///    `default_domain` from the matching adapter.
    /// 2. Hardcoded [`derive_domain()`] patterns.
    ///
    /// Deprecated: prefer [`derive_domains_via_adapter`] for
    /// multi-domain classification.
    #[allow(dead_code)]
    pub(crate) async fn derive_domain_via_adapter(
        &self,
        source_type: &str,
        uri: Option<&str>,
        mime: Option<&str>,
    ) -> Option<String> {
        self.derive_domains_via_adapter(source_type, uri, mime)
            .await
            .into_iter()
            .next()
    }

    /// Derive knowledge domains using adapters, then DB rules,
    /// then hardcoded patterns as fallback.
    ///
    /// Priority:
    /// 1. Adapter match → `[adapter.default_domain]`
    /// 2. DB domain_rules table (via DomainRuleRepo)
    /// 3. Hardcoded [`derive_domains_hardcoded()`] patterns
    pub(crate) async fn derive_domains_via_adapter(
        &self,
        source_type: &str,
        uri: Option<&str>,
        mime: Option<&str>,
    ) -> Vec<String> {
        // 1. Adapter match
        if let Some(ref adapter_svc) = self.adapter_service {
            match adapter_svc.match_adapter(uri, mime).await {
                Ok(Some(adapter)) => {
                    if let Some(ref domain) = adapter.default_domain {
                        tracing::debug!(
                            adapter = %adapter.name,
                            domain = %domain,
                            uri = uri.unwrap_or("-"),
                            "domain derived via adapter"
                        );
                        return vec![domain.clone()];
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "adapter lookup failed, falling back"
                    );
                }
            }
        }

        // 2. DB rule lookup
        use crate::storage::traits::DomainRuleRepo;
        match DomainRuleRepo::match_rules(&*self.repo, source_type, uri).await {
            Ok(domains) if !domains.is_empty() => {
                tracing::debug!(
                    domains = ?domains,
                    uri = uri.unwrap_or("-"),
                    "domains derived via DB rules"
                );
                return domains;
            }
            Ok(_) => {} // No matches, fall through
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "DB domain rule lookup failed, falling back"
                );
            }
        }

        // 3. Hardcoded fallback
        #[allow(deprecated)]
        derive_domains_hardcoded(source_type, uri)
    }

    /// Compute the current pipeline fingerprint (if configured).
    pub(crate) fn current_fingerprint(&self) -> Option<PipelineFingerprint> {
        self.fingerprint_config
            .as_ref()
            .map(PipelineFingerprint::compute)
    }

    /// Store the pipeline fingerprint in a source's metadata.
    ///
    /// Inserts or replaces the `"pipeline_fingerprint"` key in
    /// the metadata JSON object.
    pub(crate) fn stamp_fingerprint(
        metadata: &mut serde_json::Value,
        fingerprint: &PipelineFingerprint,
    ) {
        if let serde_json::Value::Object(map) = metadata {
            map.insert("pipeline_fingerprint".to_string(), fingerprint.to_json());
        }
    }
}

/// Run TMS epistemic cascade for entities affected by source
/// retraction.
///
/// Recalculates opinions for surviving nodes and edges from their
/// remaining active extractions. Nodes/edges that lost all support
/// get vacuous opinions (u=1.0). Those with remaining support get
/// re-fused opinions via cumulative fusion.
///
/// This implements the dependency-directed backtracking described
/// in spec 07 (Epistemic Model, §TMS Cascade).
pub(crate) async fn epistemic_cascade(
    repo: &PgRepo,
    surviving_node_ids: &[crate::types::ids::NodeId],
    affected_edge_ids: &[crate::types::ids::EdgeId],
) -> crate::error::Result<crate::epistemic::cascade::CascadeResult> {
    use crate::epistemic::cascade::{recalculate_edge_opinions, recalculate_node_opinions};

    let mut result = recalculate_node_opinions(repo, surviving_node_ids).await?;
    let edge_result = recalculate_edge_opinions(repo, affected_edge_ids).await?;
    result.merge(&edge_result);
    Ok(result)
}
