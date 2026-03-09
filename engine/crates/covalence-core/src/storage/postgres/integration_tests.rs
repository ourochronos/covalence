//! Integration tests for PostgreSQL repository implementations.
//!
//! These tests require a running PostgreSQL instance on port 5435.
//! Run with: `cargo test -- --ignored`

use crate::models::audit::{AuditAction, AuditLog};
use crate::models::chunk::{Chunk, ChunkLevel};
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::models::source::{Source, SourceType};
use crate::storage::traits::{
    AuditLogRepo, ChunkRepo, EdgeRepo, ExtractionRepo, NodeAliasRepo, NodeRepo, SourceRepo,
};
use crate::types::ids::AliasId;

use super::PgRepo;

const DEFAULT_DB_URL: &str = "postgres://covalence:covalence@localhost:5435/covalence_dev";

async fn make_repo() -> PgRepo {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DB_URL.to_string());
    PgRepo::new(&url).await.expect("failed to connect to PG")
}

// ── Source ───────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_source_crud() {
    let repo = make_repo().await;

    let mut source = Source::new(SourceType::Document, vec![1, 2, 3, 4]);
    source.title = Some("Integration Test Source".to_string());
    source.uri = Some("https://example.com/test".to_string());

    // Create
    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    // Get
    let fetched = SourceRepo::get(&repo, source.id)
        .await
        .expect("get source")
        .expect("source should exist");
    assert_eq!(fetched.id, source.id);
    assert_eq!(fetched.title.as_deref(), Some("Integration Test Source"));
    assert_eq!(fetched.source_type, "document");

    // List
    let list = SourceRepo::list(&repo, 100, 0).await.expect("list sources");
    assert!(list.iter().any(|s| s.id == source.id));

    // Delete
    let deleted = SourceRepo::delete(&repo, source.id)
        .await
        .expect("delete source");
    assert!(deleted);

    // Confirm gone
    let gone = SourceRepo::get(&repo, source.id)
        .await
        .expect("get after delete");
    assert!(gone.is_none());
}

#[tokio::test]
#[ignore]
async fn test_source_get_by_hash() {
    let repo = make_repo().await;

    let hash = vec![10, 20, 30, 40, 50];
    let source = Source::new(SourceType::WebPage, hash.clone());

    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    let found = SourceRepo::get_by_hash(&repo, &hash)
        .await
        .expect("get by hash")
        .expect("should find by hash");
    assert_eq!(found.id, source.id);

    // Cleanup
    SourceRepo::delete(&repo, source.id).await.expect("cleanup");
}

// ── Chunk ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_chunk_crud() {
    let repo = make_repo().await;

    // Need a source first (FK constraint)
    let source = Source::new(SourceType::Manual, vec![5, 6, 7, 8]);
    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    let chunk = Chunk::new(
        source.id,
        ChunkLevel::Paragraph,
        0,
        "Test paragraph content.".to_string(),
        vec![11, 12, 13],
        5,
    );

    // Create
    ChunkRepo::create(&repo, &chunk)
        .await
        .expect("create chunk");

    // Get
    let fetched = ChunkRepo::get(&repo, chunk.id)
        .await
        .expect("get chunk")
        .expect("chunk should exist");
    assert_eq!(fetched.id, chunk.id);
    assert_eq!(fetched.content, "Test paragraph content.");
    assert_eq!(fetched.level, "paragraph");
    assert_eq!(fetched.ordinal, 0);

    // List by source
    let chunks = ChunkRepo::list_by_source(&repo, source.id)
        .await
        .expect("list by source");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].id, chunk.id);

    // Cleanup
    ChunkRepo::delete(&repo, chunk.id)
        .await
        .expect("delete chunk");
    SourceRepo::delete(&repo, source.id)
        .await
        .expect("cleanup source");
}

#[tokio::test]
#[ignore]
async fn test_chunk_list_by_source_empty() {
    let repo = make_repo().await;

    let source = Source::new(SourceType::Manual, vec![20, 21, 22]);
    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    let chunks = ChunkRepo::list_by_source(&repo, source.id)
        .await
        .expect("list by source");
    assert!(chunks.is_empty());

    SourceRepo::delete(&repo, source.id).await.expect("cleanup");
}

// ── Node ─────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_node_crud() {
    let repo = make_repo().await;

    let node = Node::new("Integration Test Person".to_string(), "person".to_string());

    // Create
    NodeRepo::create(&repo, &node).await.expect("create node");

    // Get
    let fetched = NodeRepo::get(&repo, node.id)
        .await
        .expect("get node")
        .expect("node should exist");
    assert_eq!(fetched.id, node.id);
    assert_eq!(fetched.canonical_name, "Integration Test Person");
    assert_eq!(fetched.node_type, "person");
    assert_eq!(fetched.mention_count, 1);

    // Find by name (case-insensitive)
    let found = NodeRepo::find_by_name(&repo, "integration test person")
        .await
        .expect("find by name")
        .expect("should find case-insensitively");
    assert_eq!(found.id, node.id);

    // List by type
    let nodes = NodeRepo::list_by_type(&repo, "person", 100, 0)
        .await
        .expect("list by type");
    assert!(nodes.iter().any(|n| n.id == node.id));

    // Update
    let mut updated = fetched;
    updated.description = Some("A test person for integration tests.".to_string());
    updated.mention_count = 5;
    NodeRepo::update(&repo, &updated)
        .await
        .expect("update node");

    let after_update = NodeRepo::get(&repo, node.id)
        .await
        .expect("get after update")
        .expect("should still exist");
    assert_eq!(
        after_update.description.as_deref(),
        Some("A test person for integration tests.")
    );
    assert_eq!(after_update.mention_count, 5);

    // Delete
    let deleted = NodeRepo::delete(&repo, node.id).await.expect("delete node");
    assert!(deleted);

    let gone = NodeRepo::get(&repo, node.id)
        .await
        .expect("get after delete");
    assert!(gone.is_none());
}

#[tokio::test]
#[ignore]
async fn test_node_find_by_name_not_found() {
    let repo = make_repo().await;

    let result = NodeRepo::find_by_name(&repo, "nonexistent node name 12345")
        .await
        .expect("find by name");
    assert!(result.is_none());
}

// ── Edge ─────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_edge_crud() {
    let repo = make_repo().await;

    // Create two nodes for the edge
    let node_a = Node::new("Edge Test Node A".to_string(), "concept".to_string());
    let node_b = Node::new("Edge Test Node B".to_string(), "concept".to_string());
    NodeRepo::create(&repo, &node_a).await.expect("create A");
    NodeRepo::create(&repo, &node_b).await.expect("create B");

    let edge = Edge::new(node_a.id, node_b.id, "related_to".to_string());

    // Create
    EdgeRepo::create(&repo, &edge).await.expect("create edge");

    // Get
    let fetched = EdgeRepo::get(&repo, edge.id)
        .await
        .expect("get edge")
        .expect("edge should exist");
    assert_eq!(fetched.id, edge.id);
    assert_eq!(fetched.source_node_id, node_a.id);
    assert_eq!(fetched.target_node_id, node_b.id);
    assert_eq!(fetched.rel_type, "related_to");
    assert!((fetched.weight - 1.0).abs() < f64::EPSILON);

    // List from node
    let from_a = EdgeRepo::list_from_node(&repo, node_a.id)
        .await
        .expect("list from A");
    assert_eq!(from_a.len(), 1);
    assert_eq!(from_a[0].id, edge.id);

    // List to node
    let to_b = EdgeRepo::list_to_node(&repo, node_b.id)
        .await
        .expect("list to B");
    assert_eq!(to_b.len(), 1);
    assert_eq!(to_b[0].id, edge.id);

    // List from B should be empty
    let from_b = EdgeRepo::list_from_node(&repo, node_b.id)
        .await
        .expect("list from B");
    assert!(from_b.is_empty());

    // Cleanup
    EdgeRepo::delete(&repo, edge.id).await.expect("delete edge");
    NodeRepo::delete(&repo, node_a.id).await.expect("cleanup A");
    NodeRepo::delete(&repo, node_b.id).await.expect("cleanup B");
}

// ── Extraction ───────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_extraction_crud() {
    let repo = make_repo().await;

    // Create prerequisite source, chunk, and node
    let source = Source::new(SourceType::Code, vec![30, 31, 32]);
    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    let chunk = Chunk::new(
        source.id,
        ChunkLevel::Section,
        0,
        "Extraction test content.".to_string(),
        vec![40, 41, 42],
        4,
    );
    ChunkRepo::create(&repo, &chunk)
        .await
        .expect("create chunk");

    let node = Node::new("Extraction Test Entity".to_string(), "concept".to_string());
    NodeRepo::create(&repo, &node).await.expect("create node");

    let extraction = Extraction::new(
        chunk.id,
        ExtractedEntityType::Node,
        node.id.into_uuid(),
        "test_method".to_string(),
        0.95,
    );

    // Create
    ExtractionRepo::create(&repo, &extraction)
        .await
        .expect("create extraction");

    // List active for entity
    let active = ExtractionRepo::list_active_for_entity(&repo, "node", node.id.into_uuid())
        .await
        .expect("list active");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, extraction.id);
    assert!(!active[0].is_superseded);
    assert!((active[0].confidence - 0.95).abs() < f64::EPSILON);

    // Cleanup
    sqlx::query("DELETE FROM extractions WHERE id = $1")
        .bind(extraction.id)
        .execute(repo.pool())
        .await
        .expect("cleanup extraction");
    NodeRepo::delete(&repo, node.id)
        .await
        .expect("cleanup node");
    ChunkRepo::delete(&repo, chunk.id)
        .await
        .expect("cleanup chunk");
    SourceRepo::delete(&repo, source.id)
        .await
        .expect("cleanup source");
}

// ── NodeAlias ────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_node_alias_crud() {
    let repo = make_repo().await;

    let node = Node::new("Alias Test Node".to_string(), "organization".to_string());
    NodeRepo::create(&repo, &node).await.expect("create node");

    let alias = NodeAlias {
        id: AliasId::new(),
        node_id: node.id,
        alias: "ATN".to_string(),
        source_chunk_id: None,
    };

    // Create
    NodeAliasRepo::create(&repo, &alias)
        .await
        .expect("create alias");

    // List by node
    let aliases = NodeAliasRepo::list_by_node(&repo, node.id)
        .await
        .expect("list by node");
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].alias, "ATN");
    assert_eq!(aliases[0].node_id, node.id);

    // Create a second alias
    let alias2 = NodeAlias {
        id: AliasId::new(),
        node_id: node.id,
        alias: "Alias Test Network".to_string(),
        source_chunk_id: None,
    };
    NodeAliasRepo::create(&repo, &alias2)
        .await
        .expect("create alias2");

    let aliases = NodeAliasRepo::list_by_node(&repo, node.id)
        .await
        .expect("list by node again");
    assert_eq!(aliases.len(), 2);

    // Cleanup
    NodeAliasRepo::delete(&repo, alias.id)
        .await
        .expect("delete alias");
    NodeAliasRepo::delete(&repo, alias2.id)
        .await
        .expect("delete alias2");
    NodeRepo::delete(&repo, node.id)
        .await
        .expect("cleanup node");
}

// ── AuditLog ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_audit_log_crud() {
    let repo = make_repo().await;

    let log = AuditLog::new(
        AuditAction::SourceIngest,
        "system:integration_test".to_string(),
        serde_json::json!({"test": true}),
    );

    // Create
    AuditLogRepo::create(&repo, &log)
        .await
        .expect("create audit log");

    // Get
    let fetched = AuditLogRepo::get(&repo, log.id)
        .await
        .expect("get audit log")
        .expect("log should exist");
    assert_eq!(fetched.id, log.id);
    assert_eq!(fetched.action, "SOURCE_INGEST");
    assert_eq!(fetched.actor, "system:integration_test");

    // List recent
    let recent = AuditLogRepo::list_recent(&repo, 100)
        .await
        .expect("list recent");
    assert!(recent.iter().any(|l| l.id == log.id));

    // Cleanup
    sqlx::query("DELETE FROM audit_logs WHERE id = $1")
        .bind(log.id)
        .execute(repo.pool())
        .await
        .expect("cleanup audit log");
}

#[tokio::test]
#[ignore]
async fn test_audit_log_with_target() {
    let repo = make_repo().await;

    let target_id = uuid::Uuid::new_v4();
    let log = AuditLog::new(
        AuditAction::TrustUpdate,
        "system:integration_test".to_string(),
        serde_json::json!({"before": 0.5, "after": 0.8}),
    )
    .with_target("source", target_id);

    AuditLogRepo::create(&repo, &log)
        .await
        .expect("create audit log");

    let fetched = AuditLogRepo::get(&repo, log.id)
        .await
        .expect("get audit log")
        .expect("log should exist");
    assert_eq!(fetched.target_type.as_deref(), Some("source"));
    assert_eq!(fetched.target_id, Some(target_id));

    // List by target
    let by_target = AuditLogRepo::list_by_target(&repo, "source", target_id, 10)
        .await
        .expect("list by target");
    assert!(by_target.iter().any(|l| l.id == log.id));

    // Cleanup
    sqlx::query("DELETE FROM audit_logs WHERE id = $1")
        .bind(log.id)
        .execute(repo.pool())
        .await
        .expect("cleanup audit log");
}
