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

// ── Cascading Source Delete ──────────────────────────────────────

#[tokio::test]
#[ignore]
async fn test_source_cascading_delete() {
    use std::sync::Arc;

    use crate::services::source::SourceService;

    let repo = make_repo().await;
    let repo = Arc::new(repo);
    let svc = SourceService::new(Arc::clone(&repo));

    // Set up a full provenance chain:
    // source -> chunk -> extraction -> node -> edge

    let source = Source::new(SourceType::Document, vec![99, 98, 97, 96]);
    SourceRepo::create(&*repo, &source)
        .await
        .expect("create source");

    let chunk = Chunk::new(
        source.id,
        ChunkLevel::Section,
        0,
        "Cascade delete test content.".to_string(),
        vec![88, 87, 86],
        5,
    );
    ChunkRepo::create(&*repo, &chunk)
        .await
        .expect("create chunk");

    let node_a = Node::new("Cascade Node A".to_string(), "concept".to_string());
    let node_b = Node::new("Cascade Node B".to_string(), "concept".to_string());
    NodeRepo::create(&*repo, &node_a).await.expect("create A");
    NodeRepo::create(&*repo, &node_b).await.expect("create B");

    // Extraction linking chunk -> node_a
    let ext = Extraction::new(
        chunk.id,
        ExtractedEntityType::Node,
        node_a.id.into_uuid(),
        "test_method".to_string(),
        0.9,
    );
    ExtractionRepo::create(&*repo, &ext)
        .await
        .expect("create extraction");

    // Edge between node_a and node_b
    let edge = Edge::new(node_a.id, node_b.id, "related_to".to_string());
    EdgeRepo::create(&*repo, &edge).await.expect("create edge");

    // Alias for node_a referencing the chunk
    let alias = NodeAlias {
        id: AliasId::new(),
        node_id: node_a.id,
        alias: "CNA".to_string(),
        source_chunk_id: Some(chunk.id),
    };
    NodeAliasRepo::create(&*repo, &alias)
        .await
        .expect("create alias");

    // --- Perform cascading delete ---
    let result = svc.delete(source.id).await.expect("cascading delete");

    assert!(result.deleted, "source should be deleted");
    assert_eq!(result.chunks_deleted, 1);
    assert_eq!(result.extractions_deleted, 1);
    assert_eq!(result.nodes_deleted, 1, "orphaned node_a should be deleted");
    assert_eq!(
        result.edges_deleted, 1,
        "edge involving node_a should be deleted"
    );

    // Verify everything is gone
    assert!(
        SourceRepo::get(&*repo, source.id)
            .await
            .expect("get source")
            .is_none()
    );
    assert!(
        ChunkRepo::get(&*repo, chunk.id)
            .await
            .expect("get chunk")
            .is_none()
    );
    assert!(
        NodeRepo::get(&*repo, node_a.id)
            .await
            .expect("get node_a")
            .is_none()
    );
    assert!(
        EdgeRepo::get(&*repo, edge.id)
            .await
            .expect("get edge")
            .is_none()
    );

    // node_b should still exist (no extractions from this source)
    assert!(
        NodeRepo::get(&*repo, node_b.id)
            .await
            .expect("get node_b")
            .is_some()
    );

    // Cleanup remaining
    NodeRepo::delete(&*repo, node_b.id)
        .await
        .expect("cleanup B");
}

#[tokio::test]
#[ignore]
async fn test_source_cascading_delete_preserves_shared_nodes() {
    use std::sync::Arc;

    use crate::services::source::SourceService;

    let repo = make_repo().await;
    let repo = Arc::new(repo);
    let svc = SourceService::new(Arc::clone(&repo));

    // Create two sources that share a node
    let source1 = Source::new(SourceType::Document, vec![70, 71, 72]);
    let source2 = Source::new(SourceType::Document, vec![73, 74, 75]);
    SourceRepo::create(&*repo, &source1)
        .await
        .expect("create s1");
    SourceRepo::create(&*repo, &source2)
        .await
        .expect("create s2");

    let chunk1 = Chunk::new(
        source1.id,
        ChunkLevel::Section,
        0,
        "Shared node test chunk 1".to_string(),
        vec![60, 61],
        4,
    );
    let chunk2 = Chunk::new(
        source2.id,
        ChunkLevel::Section,
        0,
        "Shared node test chunk 2".to_string(),
        vec![62, 63],
        4,
    );
    ChunkRepo::create(&*repo, &chunk1).await.expect("create c1");
    ChunkRepo::create(&*repo, &chunk2).await.expect("create c2");

    // Shared node referenced by both sources
    let shared_node = Node::new("Shared Node".to_string(), "concept".to_string());
    NodeRepo::create(&*repo, &shared_node)
        .await
        .expect("create shared");

    let ext1 = Extraction::new(
        chunk1.id,
        ExtractedEntityType::Node,
        shared_node.id.into_uuid(),
        "test_method".to_string(),
        0.9,
    );
    let ext2 = Extraction::new(
        chunk2.id,
        ExtractedEntityType::Node,
        shared_node.id.into_uuid(),
        "test_method".to_string(),
        0.85,
    );
    ExtractionRepo::create(&*repo, &ext1)
        .await
        .expect("create ext1");
    ExtractionRepo::create(&*repo, &ext2)
        .await
        .expect("create ext2");

    // Delete source1 — shared_node should survive
    let result = svc.delete(source1.id).await.expect("delete source1");

    assert!(result.deleted);
    assert_eq!(result.chunks_deleted, 1);
    assert_eq!(result.extractions_deleted, 1);
    assert_eq!(result.nodes_deleted, 0, "shared node should NOT be deleted");
    assert_eq!(result.edges_deleted, 0);

    // Verify shared_node still exists with updated mention_count
    let node = NodeRepo::get(&*repo, shared_node.id)
        .await
        .expect("get shared")
        .expect("shared node should still exist");
    assert_eq!(
        node.mention_count, 1,
        "mention_count should reflect one remaining extraction"
    );

    // Cleanup
    let result2 = svc.delete(source2.id).await.expect("delete source2");
    assert!(result2.deleted);
    assert_eq!(
        result2.nodes_deleted, 1,
        "shared node should now be deleted"
    );

    // Verify fully cleaned up
    assert!(
        NodeRepo::get(&*repo, shared_node.id)
            .await
            .expect("get shared")
            .is_none()
    );
}

#[tokio::test]
#[ignore]
async fn test_extraction_delete_by_source() {
    let repo = make_repo().await;

    let source = Source::new(SourceType::Document, vec![50, 51, 52]);
    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    let chunk = Chunk::new(
        source.id,
        ChunkLevel::Paragraph,
        0,
        "Delete by source test".to_string(),
        vec![53, 54],
        3,
    );
    ChunkRepo::create(&repo, &chunk)
        .await
        .expect("create chunk");

    let node = Node::new("Del Test Node".to_string(), "concept".to_string());
    NodeRepo::create(&repo, &node).await.expect("create node");

    let ext = Extraction::new(
        chunk.id,
        ExtractedEntityType::Node,
        node.id.into_uuid(),
        "test".to_string(),
        0.8,
    );
    ExtractionRepo::create(&repo, &ext)
        .await
        .expect("create ext");

    // Test list_node_ids_by_source
    let node_ids = ExtractionRepo::list_node_ids_by_source(&repo, source.id)
        .await
        .expect("list node ids");
    assert_eq!(node_ids.len(), 1);
    assert_eq!(node_ids[0], node.id);

    // Test count_active_by_entity
    let count = ExtractionRepo::count_active_by_entity(&repo, "node", node.id.into_uuid())
        .await
        .expect("count active");
    assert_eq!(count, 1);

    // Test delete_by_source
    let deleted = ExtractionRepo::delete_by_source(&repo, source.id)
        .await
        .expect("delete by source");
    assert_eq!(deleted, 1);

    // Verify extraction is gone
    let count = ExtractionRepo::count_active_by_entity(&repo, "node", node.id.into_uuid())
        .await
        .expect("count after delete");
    assert_eq!(count, 0);

    // Cleanup
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

#[tokio::test]
#[ignore]
async fn test_edge_delete_by_node() {
    let repo = make_repo().await;

    let node_a = Node::new("Edge Del A".to_string(), "concept".to_string());
    let node_b = Node::new("Edge Del B".to_string(), "concept".to_string());
    let node_c = Node::new("Edge Del C".to_string(), "concept".to_string());
    NodeRepo::create(&repo, &node_a).await.expect("create A");
    NodeRepo::create(&repo, &node_b).await.expect("create B");
    NodeRepo::create(&repo, &node_c).await.expect("create C");

    let edge_ab = Edge::new(node_a.id, node_b.id, "knows".to_string());
    let edge_ca = Edge::new(node_c.id, node_a.id, "references".to_string());
    let edge_bc = Edge::new(node_b.id, node_c.id, "contains".to_string());
    EdgeRepo::create(&repo, &edge_ab).await.expect("create AB");
    EdgeRepo::create(&repo, &edge_ca).await.expect("create CA");
    EdgeRepo::create(&repo, &edge_bc).await.expect("create BC");

    // Delete edges involving node_a
    let deleted = EdgeRepo::delete_by_node(&repo, node_a.id)
        .await
        .expect("delete by node");
    assert_eq!(deleted, 2, "should delete AB and CA edges");

    // edge_bc should survive
    assert!(
        EdgeRepo::get(&repo, edge_bc.id)
            .await
            .expect("get BC")
            .is_some()
    );

    // Cleanup
    EdgeRepo::delete(&repo, edge_bc.id)
        .await
        .expect("cleanup BC");
    NodeRepo::delete(&repo, node_a.id).await.expect("cleanup A");
    NodeRepo::delete(&repo, node_b.id).await.expect("cleanup B");
    NodeRepo::delete(&repo, node_c.id).await.expect("cleanup C");
}

#[tokio::test]
#[ignore]
async fn test_node_alias_delete_by_node() {
    let repo = make_repo().await;

    let node = Node::new("Alias Del Node".to_string(), "concept".to_string());
    NodeRepo::create(&repo, &node).await.expect("create node");

    let a1 = NodeAlias {
        id: AliasId::new(),
        node_id: node.id,
        alias: "ADN1".to_string(),
        source_chunk_id: None,
    };
    let a2 = NodeAlias {
        id: AliasId::new(),
        node_id: node.id,
        alias: "ADN2".to_string(),
        source_chunk_id: None,
    };
    NodeAliasRepo::create(&repo, &a1).await.expect("create a1");
    NodeAliasRepo::create(&repo, &a2).await.expect("create a2");

    let deleted = NodeAliasRepo::delete_by_node(&repo, node.id)
        .await
        .expect("delete by node");
    assert_eq!(deleted, 2);

    let remaining = NodeAliasRepo::list_by_node(&repo, node.id)
        .await
        .expect("list after delete");
    assert!(remaining.is_empty());

    NodeRepo::delete(&repo, node.id).await.expect("cleanup");
}

#[tokio::test]
#[ignore]
async fn test_node_alias_clear_source_chunks() {
    let repo = make_repo().await;

    let source = Source::new(SourceType::Document, vec![80, 81, 82]);
    SourceRepo::create(&repo, &source)
        .await
        .expect("create source");

    let chunk = Chunk::new(
        source.id,
        ChunkLevel::Paragraph,
        0,
        "clear source chunks test".to_string(),
        vec![83, 84],
        3,
    );
    ChunkRepo::create(&repo, &chunk)
        .await
        .expect("create chunk");

    let node = Node::new("Clear Chunks Node".to_string(), "concept".to_string());
    NodeRepo::create(&repo, &node).await.expect("create node");

    let alias = NodeAlias {
        id: AliasId::new(),
        node_id: node.id,
        alias: "CCN".to_string(),
        source_chunk_id: Some(chunk.id),
    };
    NodeAliasRepo::create(&repo, &alias)
        .await
        .expect("create alias");

    // Verify source_chunk_id is set
    let before = NodeAliasRepo::get(&repo, alias.id)
        .await
        .expect("get alias")
        .expect("alias should exist");
    assert!(before.source_chunk_id.is_some());

    // Clear source chunks
    let cleared = NodeAliasRepo::clear_source_chunks(&repo, source.id)
        .await
        .expect("clear source chunks");
    assert_eq!(cleared, 1);

    // Verify source_chunk_id is now NULL
    let after = NodeAliasRepo::get(&repo, alias.id)
        .await
        .expect("get alias after clear")
        .expect("alias should still exist");
    assert!(after.source_chunk_id.is_none());

    // Cleanup
    NodeAliasRepo::delete(&repo, alias.id)
        .await
        .expect("cleanup alias");
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
