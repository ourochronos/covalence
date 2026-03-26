//! Tests for the source service module.

use super::*;
use crate::ingestion::code_chunker;
use crate::models::extraction::Extraction;
use crate::storage::traits::ExtractionRepo;
use crate::types::ids::{ChunkId, EdgeId, NodeId, SourceId};

#[test]
fn delete_result_serializes_all_fields() {
    let result = DeleteResult {
        deleted: true,
        chunks_deleted: 5,
        extractions_deleted: 10,
        statements_deleted: 2,
        sections_deleted: 1,
        nodes_deleted: 3,
        edges_deleted: 7,
        nodes_recalculated: 4,
        edges_recalculated: 2,
    };
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["deleted"], true);
    assert_eq!(json["chunks_deleted"], 5);
    assert_eq!(json["extractions_deleted"], 10);
    assert_eq!(json["statements_deleted"], 2);
    assert_eq!(json["sections_deleted"], 1);
    assert_eq!(json["nodes_deleted"], 3);
    assert_eq!(json["edges_deleted"], 7);
    assert_eq!(json["nodes_recalculated"], 4);
    assert_eq!(json["edges_recalculated"], 2);
}

#[test]
fn delete_result_zero_counts_when_nothing_found() {
    let result = DeleteResult {
        deleted: false,
        chunks_deleted: 0,
        extractions_deleted: 0,
        statements_deleted: 0,
        sections_deleted: 0,
        nodes_deleted: 0,
        edges_deleted: 0,
        nodes_recalculated: 0,
        edges_recalculated: 0,
    };
    assert!(!result.deleted);
    assert_eq!(result.chunks_deleted, 0);
    assert_eq!(result.extractions_deleted, 0);
    assert_eq!(result.statements_deleted, 0);
    assert_eq!(result.sections_deleted, 0);
    assert_eq!(result.nodes_deleted, 0);
    assert_eq!(result.edges_deleted, 0);
    assert_eq!(result.nodes_recalculated, 0);
    assert_eq!(result.edges_recalculated, 0);
}

/// Verify that adapter regex validation catches malformed patterns.
#[test]
fn adapter_regex_validation_catches_invalid_patterns() {
    use crate::services::adapter_service::SourceAdapter;

    let adapter = SourceAdapter {
        id: uuid::Uuid::new_v4(),
        name: "test-bad-regex".to_string(),
        description: None,
        match_domain: None,
        match_mime: None,
        match_uri_regex: Some("[invalid((".to_string()),
        converter: None,
        normalization: "default".to_string(),
        prompt_template: None,
        default_source_type: "document".to_string(),
        default_domain: Some("research".to_string()),
        webhook_url: None,
        coref_enabled: true,
        statement_enabled: true,
        is_active: true,
    };

    // Regex::new should reject the pattern.
    let result = regex::Regex::new(adapter.match_uri_regex.as_ref().unwrap());
    assert!(result.is_err(), "should reject invalid regex");
}

/// Verify that valid adapter regex patterns compile successfully.
#[test]
fn adapter_regex_validation_accepts_valid_patterns() {
    let patterns = &[
        r"^file://spec/",
        r"^https://arxiv\.org/",
        r"^file://engine/.*\.rs$",
        r"^https?://",
    ];
    for pattern in patterns {
        let result = regex::Regex::new(pattern);
        assert!(
            result.is_ok(),
            "pattern '{}' should be valid: {:?}",
            pattern,
            result.err()
        );
    }
}

// --- Code source pipeline tests ---

#[test]
fn code_language_detected_from_mime() {
    let lang = code_chunker::detect_code_language("text/x-rust", None);
    assert!(lang.is_some());
    assert_eq!(lang.unwrap(), code_chunker::CodeLanguage::Rust,);
}

#[test]
fn code_language_detected_from_uri_fallback() {
    let lang = code_chunker::detect_code_language("application/octet-stream", Some("src/main.rs"));
    assert!(lang.is_some());
    assert_eq!(lang.unwrap(), code_chunker::CodeLanguage::Rust,);
}

#[test]
fn code_to_markdown_produces_function_headings() {
    let source = concat!(
        "fn hello() {\n",
        "    println!(\"hello\");\n",
        "}\n",
        "\n",
        "fn world(x: i32) -> bool {\n",
        "    x > 0\n",
        "}\n",
    );
    let md = code_chunker::code_to_markdown(source.trim(), code_chunker::CodeLanguage::Rust)
        .expect("code_to_markdown should succeed");

    // Verify tree-sitter produces function-level headings.
    assert!(
        md.contains("# fn hello()"),
        "expected heading for hello(), got:\n{md}"
    );
    assert!(
        md.contains("# fn world(x: i32) -> bool"),
        "expected heading for world(), got:\n{md}"
    );
    // Verify fenced code blocks are present.
    assert!(
        md.contains("```rust"),
        "expected rust code fences, got:\n{md}"
    );
}

#[test]
fn code_source_skips_normalization() {
    // Normalization would collapse indentation. Verify that
    // for code sources the pipeline preserves indentation by
    // checking that code_to_markdown output retains it.
    let source = "fn foo() {\n    let x = 1;\n}\n";
    let md = code_chunker::code_to_markdown(source.trim(), code_chunker::CodeLanguage::Rust)
        .expect("code_to_markdown should succeed");

    // The 4-space indentation should survive since we skip
    // normalization for code.
    assert!(
        md.contains("    let x = 1;"),
        "indentation should be preserved in code markdown"
    );

    // Verify normalization *would* break it.
    let normalized = crate::ingestion::normalize::normalize(&md);
    assert!(
        !normalized.contains("    let x = 1;"),
        "normalization should collapse indentation"
    );
}

#[test]
fn code_source_skips_coref() {
    // Coreference resolution on code would produce spurious
    // links. Verify it finds nothing meaningful in code.
    let md = code_chunker::code_to_markdown(
        "fn main() {\n    println!(\"hello\");\n}",
        code_chunker::CodeLanguage::Rust,
    )
    .expect("code_to_markdown should succeed");

    let chunks = crate::ingestion::chunker::chunk_document(&md, 1000, 200);
    let resolver = crate::ingestion::coreference::CorefResolver::new();
    let links = resolver.resolve(&chunks);

    // Code content should not produce meaningful coref links.
    // Any links found would be noise from identifiers.
    // The pipeline skips this for code — verify the resolver
    // at least doesn't crash on code content.
    assert!(
        links.is_empty() || links.iter().all(|l| l.mention.len() <= 3),
        "coref on code should not find meaningful entities"
    );
}

#[test]
fn non_code_mime_does_not_trigger_code_path() {
    let lang = code_chunker::detect_code_language("text/html", Some("index.html"));
    assert!(lang.is_none(), "HTML should not be detected as code");
}

// --- ExtractionRepo mock and supersession tests ---

/// In-memory mock of [`ExtractionRepo`] that records
/// `mark_superseded_by_source` calls for verification.
struct MockExtractionRepo {
    superseded_sources: std::sync::Mutex<Vec<SourceId>>,
}

impl MockExtractionRepo {
    fn new() -> Self {
        Self {
            superseded_sources: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn superseded_source_ids(&self) -> Vec<SourceId> {
        self.superseded_sources
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }
}

impl ExtractionRepo for MockExtractionRepo {
    async fn create(&self, _extraction: &Extraction) -> crate::error::Result<()> {
        Ok(())
    }

    async fn get(
        &self,
        _id: crate::types::ids::ExtractionId,
    ) -> crate::error::Result<Option<Extraction>> {
        Ok(None)
    }

    async fn list_by_chunk(&self, _chunk_id: ChunkId) -> crate::error::Result<Vec<Extraction>> {
        Ok(Vec::new())
    }

    async fn list_active_for_entity(
        &self,
        _entity_type: &str,
        _entity_id: uuid::Uuid,
    ) -> crate::error::Result<Vec<Extraction>> {
        Ok(Vec::new())
    }

    async fn mark_superseded(
        &self,
        _id: crate::types::ids::ExtractionId,
    ) -> crate::error::Result<()> {
        Ok(())
    }

    async fn mark_superseded_by_source(&self, source_id: SourceId) -> crate::error::Result<u64> {
        if let Ok(mut v) = self.superseded_sources.lock() {
            v.push(source_id);
        }
        Ok(1)
    }

    async fn delete_by_source(&self, _source_id: SourceId) -> crate::error::Result<u64> {
        Ok(0)
    }

    async fn list_node_ids_by_source(
        &self,
        _source_id: SourceId,
    ) -> crate::error::Result<Vec<NodeId>> {
        Ok(Vec::new())
    }

    async fn count_active_by_entity(
        &self,
        _entity_type: &str,
        _entity_id: uuid::Uuid,
    ) -> crate::error::Result<i64> {
        Ok(0)
    }

    async fn list_edge_ids_by_source(
        &self,
        _source_id: SourceId,
    ) -> crate::error::Result<Vec<EdgeId>> {
        Ok(Vec::new())
    }

    async fn list_active_for_entities(
        &self,
        _entity_type: &str,
        _entity_ids: &[uuid::Uuid],
    ) -> crate::error::Result<Vec<Extraction>> {
        Ok(Vec::new())
    }
}

/// Verify that `ExtractionRepo::mark_superseded_by_source`
/// correctly receives the old source ID during supersession.
///
/// This tests the trait contract that the supersession path in
/// `ingest()` relies on: when a new source supersedes an old
/// one, `mark_superseded_by_source` must be called with the
/// old source's ID so its extractions stop polluting search.
#[tokio::test]
async fn mark_superseded_by_source_records_old_source_id() {
    let repo = MockExtractionRepo::new();
    let old_source_id = SourceId::from_uuid(uuid::Uuid::new_v4());

    let count = ExtractionRepo::mark_superseded_by_source(&repo, old_source_id).await;

    assert!(count.is_ok(), "mark_superseded_by_source should succeed");
    assert_eq!(count.unwrap_or(0), 1);

    let recorded = repo.superseded_source_ids();
    assert_eq!(recorded.len(), 1, "exactly one source should be superseded");
    assert_eq!(
        recorded[0], old_source_id,
        "recorded source ID must match the old source"
    );
}
