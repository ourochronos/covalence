//! Integration tests for SHA-256 content_hash (covalence#78).
//!
//! Verifies that:
//! (a) Ingesting a source populates `content_hash` with a non-empty hex string.
//! (b) Updating an article changes its `content_hash`.
//! (c) The hash is the correct SHA-256 of the content.
//! (d) `source_get` and `article_get` expose `content_hash` in their responses.

use serial_test::serial;
use sha2::{Digest, Sha256};

use covalence_engine::services::article_service::{
    ArticleService, CreateArticleRequest, UpdateArticleRequest,
};
use covalence_engine::services::source_service::{IngestRequest, SourceService};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Compute the expected SHA-256 hex digest of a string — mirrors what the
/// application does at write time.
fn expected_sha256(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

// ─── (a) source ingest populates content_hash ────────────────────────────────

/// Ingesting a source must set `content_hash` to the SHA-256 of the content.
#[tokio::test]
#[serial]
async fn test_source_ingest_populates_content_hash() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let content = "The mitochondria is the powerhouse of the cell.";
    let req = IngestRequest {
        content: content.to_string(),
        source_type: Some("document".to_string()),
        title: Some("content-hash-source-test".to_string()),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: None,
        facet_function: None,
        facet_scope: None,
    };

    let resp = svc.ingest(req).await.expect("ingest must succeed");

    // content_hash must be present and non-empty.
    let hash = resp
        .content_hash
        .as_deref()
        .expect("content_hash must be Some after ingest");
    assert!(!hash.is_empty(), "content_hash must not be empty");

    // Must be a valid 64-character lowercase hex string (SHA-256 = 32 bytes).
    assert_eq!(
        hash.len(),
        64,
        "SHA-256 hex digest must be 64 characters, got {}",
        hash.len()
    );
    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "content_hash must contain only hex characters, got: {hash}"
    );

    // Must match the expected SHA-256 of the content.
    let expected = expected_sha256(content);
    assert_eq!(
        hash, expected,
        "content_hash must equal SHA-256(content). expected={expected}, got={hash}"
    );

    fix.cleanup().await;
}

/// `source_get` must expose `content_hash` in the response.
#[tokio::test]
#[serial]
async fn test_source_get_exposes_content_hash() {
    let fix = TestFixture::new().await;
    let svc = SourceService::new(fix.pool.clone());

    let content = "Unique content for source_get content_hash test.";
    let req = IngestRequest {
        content: content.to_string(),
        source_type: Some("document".to_string()),
        title: Some("content-hash-get-test".to_string()),
        metadata: None,
        session_id: None,
        reliability: None,
        capture_method: None,
        facet_function: None,
        facet_scope: None,
    };

    let ingested = svc.ingest(req).await.expect("ingest must succeed");

    // Re-fetch via get() and verify content_hash is preserved.
    let fetched = svc.get(ingested.id).await.expect("get must succeed");
    assert_eq!(
        fetched.content_hash, ingested.content_hash,
        "source_get must return the same content_hash as ingest"
    );
    assert!(
        fetched.content_hash.is_some(),
        "content_hash must be Some on source_get"
    );
    let hash = fetched.content_hash.unwrap();
    assert_eq!(
        hash,
        expected_sha256(content),
        "fetched content_hash must equal SHA-256(content)"
    );

    fix.cleanup().await;
}

// ─── (b) article create populates content_hash ───────────────────────────────

/// Creating an article must set `content_hash` to the SHA-256 of the content.
#[tokio::test]
#[serial]
async fn test_article_create_populates_content_hash() {
    let fix = TestFixture::new().await;
    let svc = ArticleService::new(fix.pool.clone());

    let content = "Knowledge graphs enable structured semantic retrieval.";
    let req = CreateArticleRequest {
        content: content.to_string(),
        title: Some("content-hash-article-test".to_string()),
        domain_path: Some(vec!["knowledge".to_string()]),
        epistemic_type: None,
        source_ids: None,
        metadata: None,
        facet_function: None,
        facet_scope: None,
    };

    let resp = svc.create(req).await.expect("article create must succeed");

    let hash = resp
        .content_hash
        .as_deref()
        .expect("content_hash must be Some after article create");
    assert!(!hash.is_empty(), "content_hash must not be empty");
    assert_eq!(hash.len(), 64, "SHA-256 hex digest must be 64 characters");
    assert_eq!(
        hash,
        expected_sha256(content),
        "article content_hash must equal SHA-256(content)"
    );

    fix.cleanup().await;
}

// ─── (c) article update changes content_hash ─────────────────────────────────

/// Updating an article's content must change its `content_hash` to reflect
/// the new content's SHA-256.
#[tokio::test]
#[serial]
async fn test_article_update_changes_content_hash() {
    let fix = TestFixture::new().await;
    let svc = ArticleService::new(fix.pool.clone());

    let original_content = "Original article content — version one.";
    let create_req = CreateArticleRequest {
        content: original_content.to_string(),
        title: Some("content-hash-update-test".to_string()),
        domain_path: None,
        epistemic_type: None,
        source_ids: None,
        metadata: None,
        facet_function: None,
        facet_scope: None,
    };
    let created = svc
        .create(create_req)
        .await
        .expect("article create must succeed");

    let original_hash = created
        .content_hash
        .clone()
        .expect("content_hash must be set after create");
    assert_eq!(
        original_hash,
        expected_sha256(original_content),
        "initial content_hash must equal SHA-256(original_content)"
    );

    // Now update the content.
    let updated_content = "Updated article content — version two with different text.";
    let update_req = UpdateArticleRequest {
        content: Some(updated_content.to_string()),
        title: None,
        domain_path: None,
        metadata: None,
        pinned: None,
        facet_function: None,
        facet_scope: None,
    };
    let updated = svc
        .update(created.id, update_req)
        .await
        .expect("article update must succeed");

    let updated_hash = updated
        .content_hash
        .as_deref()
        .expect("content_hash must be Some after update");

    // Hash must have changed.
    assert_ne!(
        updated_hash, original_hash,
        "content_hash must change when content changes"
    );

    // New hash must equal SHA-256 of the new content.
    let expected_new_hash = expected_sha256(updated_content);
    assert_eq!(
        updated_hash, expected_new_hash,
        "updated content_hash must equal SHA-256(updated_content)"
    );

    fix.cleanup().await;
}

// ─── (d) article_get exposes content_hash ────────────────────────────────────

/// `article_get` must expose `content_hash` in the response after create.
#[tokio::test]
#[serial]
async fn test_article_get_exposes_content_hash() {
    let fix = TestFixture::new().await;
    let svc = ArticleService::new(fix.pool.clone());

    let content = "Unique content for article_get content_hash verification.";
    let req = CreateArticleRequest {
        content: content.to_string(),
        title: Some("content-hash-get-article-test".to_string()),
        domain_path: None,
        epistemic_type: None,
        source_ids: None,
        metadata: None,
        facet_function: None,
        facet_scope: None,
    };

    let created = svc.create(req).await.expect("article create must succeed");

    // Re-fetch via get() and verify content_hash is present and correct.
    let fetched = svc.get(created.id).await.expect("article get must succeed");
    assert_eq!(
        fetched.content_hash, created.content_hash,
        "article_get must return the same content_hash as create"
    );
    let hash = fetched
        .content_hash
        .as_deref()
        .expect("content_hash must be Some from article_get");
    assert_eq!(
        hash,
        expected_sha256(content),
        "article_get content_hash must equal SHA-256(content)"
    );

    fix.cleanup().await;
}
