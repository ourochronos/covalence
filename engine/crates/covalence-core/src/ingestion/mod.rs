//! Ingestion pipeline — transforms raw sources into structured graph elements.
//!
//! Eight-stage pipeline: accept → parse → normalize → chunk → embed →
//! extract → landscape → resolve.

pub mod accept;
pub mod chunker;
pub mod converter;
pub mod coreference;
pub mod embedder;
pub mod extractor;
pub mod gliner_extractor;
pub mod landscape;
pub mod llm_extractor;
pub mod normalize;
pub mod openai_embedder;
pub mod parser;
pub mod pg_resolver;
pub mod pii;
pub mod resolver;
pub mod takedown;
pub mod voyage;

pub use accept::{AcceptResult, compute_content_hash};
pub use chunker::{ChunkLevel, ChunkOutput, chunk_document};
pub use converter::{
    ConverterRegistry, HtmlConverter, MarkdownConverter, PlainTextConverter, SourceConverter,
};
pub use coreference::{CorefLink, CorefResolver};
pub use embedder::{Embedder, MockEmbedder};
pub use extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionResult, Extractor, MockExtractor,
};
pub use gliner_extractor::GlinerExtractor;
pub use landscape::{
    ChunkLandscapeResult, ExtractionMethod, LandscapeMetrics, ModelCalibration, analyze_landscape,
    cosine_similarity,
};
pub use llm_extractor::LlmExtractor;
pub use normalize::normalize;
pub use openai_embedder::OpenAiEmbedder;
pub use parser::{ParsedDocument, parse};
pub use pg_resolver::{PgResolver, normalize_rel_type};
pub use pii::{PiiDetector, PiiMatch, RegexPiiDetector};
pub use resolver::{EntityResolver, MatchType, MockResolver, ResolvedEntity};
pub use takedown::TakedownResult;
pub use voyage::{VoyageConfig, VoyageEmbedder};
