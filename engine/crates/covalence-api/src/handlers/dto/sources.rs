//! Source-related DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// Request body for source ingestion.
///
/// Supply either `content` (base64-encoded bytes) or `url` (fetched
/// by the server). When `url` is provided, `source_type` and `mime`
/// are auto-detected from the response if not explicitly set.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct CreateSourceRequest {
    /// Base64-encoded content bytes. Required unless `url` is provided.
    pub content: Option<String>,
    /// URL to fetch content from. The server performs the HTTP GET,
    /// detects MIME type, auto-classifies source type from URL
    /// patterns, and extracts metadata (title, author, date) from
    /// the response.
    #[validate(length(max = 2048))]
    pub url: Option<String>,
    /// Source type (document, web_page, conversation, code, api,
    /// manual, tool_output, observation). Auto-detected from URL
    /// patterns when `url` is used and this field is omitted.
    #[validate(length(max = 50))]
    pub source_type: Option<String>,
    /// MIME type of the content (e.g. "text/markdown", "text/plain").
    /// Auto-detected from Content-Type header when `url` is used.
    /// Defaults to "text/plain" for direct content upload.
    #[validate(length(max = 100))]
    pub mime: Option<String>,
    /// Optional URI of the original material. When `url` is used,
    /// the URL is stored as the URI automatically.
    #[validate(length(max = 2048))]
    pub uri: Option<String>,
    /// Title for the source. Overrides auto-extracted title from
    /// HTML `<title>` or markdown `# heading`.
    #[validate(length(max = 1000))]
    pub title: Option<String>,
    /// Author of the source. Overrides auto-extracted author from
    /// HTML meta tags.
    #[validate(length(max = 500))]
    pub author: Option<String>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
    /// Original file format before conversion (e.g. "pdf", "html",
    /// "markdown", "docx"). Stored in metadata.format_origin.
    #[validate(length(max = 50))]
    pub format_origin: Option<String>,
    /// List of authors. First entry is used as the primary author.
    /// Stored in metadata.authors.
    pub authors: Option<Vec<String>>,
}

/// Response after successful source creation.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateSourceResponse {
    /// ID of the created (or deduplicated) source.
    pub id: Uuid,
    /// Processing status: accepted (processing enqueued), complete (dedup match).
    pub status: String,
}

/// Response for a source entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceResponse {
    pub id: Uuid,
    pub source_type: String,
    pub uri: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub ingested_at: String,
    pub reliability_score: f64,
    pub clearance_level: i32,
    pub content_version: i32,
}

/// Response for source deletion.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteSourceResponse {
    pub deleted: bool,
    pub chunks_deleted: u64,
    pub extractions_deleted: u64,
    pub statements_deleted: u64,
    pub sections_deleted: u64,
    pub nodes_deleted: u64,
    pub edges_deleted: u64,
    /// Number of surviving nodes whose epistemic opinions were
    /// recalculated after losing extraction support (TMS cascade).
    pub nodes_recalculated: usize,
    /// Number of surviving edges whose epistemic opinions were
    /// recalculated after losing extraction support (TMS cascade).
    pub edges_recalculated: usize,
}

/// Response for source reprocessing.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReprocessSourceResponse {
    /// Source ID that was reprocessed.
    pub source_id: Uuid,
    /// Number of old extractions marked as superseded.
    pub extractions_superseded: u64,
    /// Number of old chunks deleted.
    pub chunks_deleted: u64,
    /// Number of new chunks created.
    pub chunks_created: usize,
    /// New content version after reprocessing.
    pub content_version: i32,
}

/// Response for a chunk entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChunkResponse {
    pub id: Uuid,
    pub source_id: Uuid,
    pub level: String,
    pub ordinal: i32,
    pub content: String,
    pub token_count: i32,
}

/// Response for enqueuing a source reprocess.
#[derive(Debug, Serialize, ToSchema)]
pub struct EnqueueReprocessResponse {
    /// Whether a new job was created (false if already queued).
    pub enqueued: bool,
    /// The job ID (present when enqueued is true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
}
