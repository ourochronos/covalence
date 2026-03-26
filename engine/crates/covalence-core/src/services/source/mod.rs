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
use crate::extensions::metadata::EnforcementLevel;
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
use crate::services::ontology_service::OntologyService;
use crate::storage::postgres::PgRepo;

pub use crud::DeleteResult;
pub use reprocess::ReprocessResult;

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
    /// Ontology service for metadata schema lookups.
    pub(crate) ontology_service: Option<Arc<OntologyService>>,
    /// Metadata schema enforcement level.
    pub(crate) metadata_enforcement: EnforcementLevel,
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
            ontology_service: None,
            metadata_enforcement: EnforcementLevel::default(),
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
            ontology_service: None,
            metadata_enforcement: EnforcementLevel::default(),
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
            ontology_service: None,
            metadata_enforcement: EnforcementLevel::default(),
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
    /// When present, [`derive_domains_via_adapter`](Self::derive_domains_via_adapter)
    /// queries adapter configs and DB rules for domain classification.
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

    /// Set the ontology service for metadata schema lookups.
    pub fn with_ontology(mut self, svc: Arc<OntologyService>) -> Self {
        self.ontology_service = Some(svc);
        self
    }

    /// Set the metadata schema enforcement level from a string.
    ///
    /// Unrecognized values default to `Warn`.
    pub fn with_metadata_enforcement(mut self, level: &str) -> Self {
        self.metadata_enforcement =
            EnforcementLevel::from_str_opt(level).unwrap_or(EnforcementLevel::Warn);
        self
    }

    /// Derive knowledge domains using adapters, then DB rules.
    ///
    /// Priority:
    /// 1. Adapter match → `[adapter.default_domain]`
    /// 2. DB domain_rules table (via DomainRuleRepo)
    ///
    /// Returns empty Vec if neither adapters nor DB rules match.
    /// The caller handles the empty case.
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
                domains
            }
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "DB domain rule lookup failed"
                );
                Vec::new()
            }
        }
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
