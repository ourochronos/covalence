//! Ingestion pipeline — transforms raw sources into structured graph elements.
//!
//! Pipeline: accept → parse → normalize → chunk/statement → embed →
//! extract → resolve.

pub mod accept;
pub mod ast_extractor;
pub mod chat_backend;
pub mod chunker;
pub mod code_chunker;
pub mod converter;
pub mod coreference;
pub mod embedder;
pub mod extractor;
pub mod fingerprint;
pub mod gliner_extractor;
pub mod http_extractor;
pub mod llm_extractor;
pub mod llm_statement_extractor;
pub mod normalize;
pub mod openai_embedder;
pub mod parser;
pub mod pg_resolver;
pub mod pii;
pub mod projection;
pub mod resolver;
pub mod section_compiler;
pub mod service_extractor;
pub mod service_registry;
pub mod source_profile;
pub mod statement_cluster;
pub mod statement_extractor;
pub mod stdio_transport;
pub mod takedown;
pub mod two_pass_extractor;
pub mod url_fetcher;
pub mod utils;
pub mod voyage;

pub use accept::{AcceptResult, compute_content_hash};
pub use ast_extractor::AstExtractor;
pub use chat_backend::{
    ChainChatBackend, ChatBackend, ChatResponse, CliChatBackend, FallbackChatBackend,
    HttpChatBackend, StreamChunk,
};
pub use chunker::{ChunkLevel, ChunkOutput, chunk_document, chunk_document_with_merge};
pub use code_chunker::{CodeLanguage, code_to_markdown, detect_code_language};
pub use converter::{
    CodeConverter, ConverterRegistry, HtmlConverter, MarkdownConverter, PdfConverter,
    PlainTextConverter, ReaderLmConverter, SourceConverter, linearize_tables,
};
pub use coreference::{CorefLink, CorefMutation, CorefResolver, CorefResult, FastcorefClient};
pub use embedder::{Embedder, MockEmbedder};
pub use extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionContext, ExtractionResult, Extractor,
    MockExtractor,
};
pub use fingerprint::{
    FingerprintConfig, FingerprintDrift, PipelineFingerprint, fingerprint_config_from,
};
pub use gliner_extractor::GlinerExtractor;
pub use http_extractor::HttpExtractor;
pub use llm_extractor::{ChatBackendExtractor, LlmExtractor};
pub use llm_statement_extractor::LlmStatementExtractor;
pub use normalize::{
    ArtifactLinePass, BlankLineCollapsePass, ControlCharPass, InlineArtifactPass, MathJaxPass,
    NormalizeChain, NormalizePass, TrimPass, UnicodeNfcPass, WhitespacePass, normalize,
    strip_artifacts,
};
pub use openai_embedder::OpenAiEmbedder;
pub use parser::{ParsedDocument, parse};
pub use pg_resolver::PgResolver;
pub use pii::{PiiDetector, PiiMatch, RegexPiiDetector};
pub use projection::{reverse_project, reverse_project_batch, sort_ledger};
pub use resolver::{EntityResolver, MatchType, MockResolver, ResolvedEntity};
pub use section_compiler::{
    CompilationOutput, LlmSectionCompiler, MockSectionCompiler, SectionCompilationInput,
    SectionCompilationOutput, SectionCompiler, SectionSummaryEntry, SourceSummaryCompiler,
    SourceSummaryInput,
};
pub use service_extractor::ServiceExtractor;
pub use service_registry::{ServiceHealth, ServiceRegistry};
pub use source_profile::{ProfileRegistry, SourceProfile};
pub use statement_cluster::{ClusterAssignments, ClusterConfig, cluster_statements};
pub use statement_extractor::{
    ExtractedStatement, MockStatementExtractor, StatementExtractionResult, StatementExtractor,
};
pub use stdio_transport::{ServiceTransport, StdioTransport};
pub use takedown::TakedownResult;
pub use two_pass_extractor::TwoPassExtractor;
pub use url_fetcher::{FetchResult, fetch_url};
pub use utils::cosine_similarity;
pub use voyage::{VoyageConfig, VoyageEmbedder};
