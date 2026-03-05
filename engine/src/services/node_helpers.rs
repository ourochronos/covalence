//! Shared helpers for node creation — used by article_service and (soon) claim_service.
//!
//! Extracted from `article_service.rs` as part of covalence#173 wave 5 (changes 4.2 + 4.3)
//! so that `ClaimService::create()` can reuse the same SHA-256 hashing, token estimation,
//! and embed-task enqueueing logic without verbatim duplication.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::errors::AppError;

// =============================================================================
// Change 4.2 — node-creation helpers
// =============================================================================

/// Compute the hex-encoded SHA-256 digest of `content`.
///
/// Used for tamper detection and federation trust verification (covalence#78).
pub(crate) fn compute_content_hash(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// Estimate a token count from raw content using the heuristic word_count / 0.75
/// (≈ 1.33 words per token, matching GPT-family tokenisation at prose density).
pub(crate) fn estimate_size_tokens(content: &str) -> i32 {
    (content.split_whitespace().count() as f64 / 0.75) as i32
}

/// Queue an embedding task for `node_id` with a contextual preamble (covalence#73).
///
/// The preamble is injected into the `embed_preamble` field of the slow-path
/// queue payload so the embedding worker can prepend it to the content before
/// vectorising.
///
/// # Parameters
/// * `pool`          — database connection pool
/// * `node_id`       — UUID of the node to embed
/// * `title`         — optional node title (used in preamble context string)
/// * `node_type_str` — human-readable node type (e.g. `"article"`, `"claim"`)
/// * `domain_path`   — domain tags joined with `/` for the preamble
pub(crate) async fn enqueue_embed_with_preamble(
    pool: &PgPool,
    node_id: Uuid,
    title: Option<&str>,
    node_type_str: &str,
    domain_path: &[String],
) -> Result<(), AppError> {
    let preamble = format!(
        "[Context: {}. Source type: {}. Domain: {}.]",
        title.unwrap_or(""),
        node_type_str,
        domain_path.join("/"),
    );
    let embed_payload = serde_json::json!({ "embed_preamble": preamble });

    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
         (id, task_type, node_id, payload, priority, status) \
         VALUES ($1, 'embed', $2, $3, 3, 'pending')",
    )
    .bind(Uuid::new_v4())
    .bind(node_id)
    .bind(&embed_payload)
    .execute(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(())
}

// =============================================================================
// Change 4.3 — NodeCore + node_core_from_row
// =============================================================================

/// Common node fields shared by every node type's row-mapping helper.
///
/// Captures the 12+ fields that appear in every `SELECT … FROM covalence.nodes`
/// projection so that type-specific services (`article_from_row`,
/// `claim_from_row`, …) can delegate the shared extraction here and only
/// handle their own additional columns.
pub(crate) struct NodeCore {
    pub id: Uuid,
    pub title: Option<String>,
    pub content: Option<String>,
    pub status: String,
    pub confidence: f32,
    pub epistemic_type: Option<String>,
    pub domain_path: Vec<String>,
    pub metadata: serde_json::Value,
    pub version: i32,
    pub pinned: bool,
    pub usage_score: f32,
    pub content_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

/// Extract the common node fields from a Postgres row.
///
/// Callers should compose this with their own type-specific field extractions:
///
/// ```ignore
/// fn article_from_row(row: &PgRow) -> ArticleResponse {
///     let core = node_core_from_row(row);
///     ArticleResponse {
///         id: core.id,
///         // … common fields …
///         facet_function: row.get("facet_function"), // article-specific
///     }
/// }
/// ```
pub(crate) fn node_core_from_row(row: &PgRow) -> NodeCore {
    NodeCore {
        id: row.get("id"),
        title: row.get("title"),
        content: row.get("content"),
        status: row.get("status"),
        confidence: row.get::<Option<f64>, _>("confidence").unwrap_or(0.5) as f32,
        epistemic_type: row.get("epistemic_type"),
        domain_path: row
            .get::<Option<Vec<String>>, _>("domain_path")
            .unwrap_or_default(),
        metadata: row.get::<serde_json::Value, _>("metadata"),
        version: row.get::<Option<i32>, _>("version").unwrap_or(1),
        pinned: row.get::<Option<bool>, _>("pinned").unwrap_or(false),
        usage_score: row.get::<Option<f64>, _>("usage_score").unwrap_or(0.0) as f32,
        content_hash: row.get("content_hash"),
        created_at: row.get("created_at"),
        modified_at: row.get("modified_at"),
    }
}
