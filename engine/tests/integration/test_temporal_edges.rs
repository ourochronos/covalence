//! Integration tests for temporal edge semantics (covalence#60).
//!
//! Verifies that:
//! 1. Newly created edges are active (`valid_to IS NULL`).
//! 2. `supersede_edge` closes the edge (`valid_to` is set) without deleting it.
//! 3. Default `list_edges` queries exclude superseded edges.
//! 4. `include_superseded = true` surfaces superseded edges.
//! 5. `SharedGraph` rebuild loads only active edges.

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::graph::{CovalenceGraph, GraphRepository, SqlGraphRepository};
use covalence_engine::models::{EdgeType, TraversalDirection};

use super::helpers::TestFixture;

// ─────────────────────────────────────────────────────────────────────────────
// 1. Newly created edge is active (valid_to IS NULL)
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_new_edge_is_active() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("src-active", "source content").await;
    let dst = fix.insert_article("dst-active", "article content").await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            dst,
            EdgeType::Originates,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge should succeed");

    // valid_to must be NULL — edge is active.
    assert!(
        edge.valid_to.is_none(),
        "newly created edge must have valid_to = NULL (active)"
    );

    // valid_from must be set (non-epoch).
    assert!(
        edge.valid_from.timestamp() > 0,
        "valid_from must be a real timestamp"
    );

    // The edge must appear in the default (active-only) list.
    let active = graph
        .list_edges(src, TraversalDirection::Outbound, None, 100, false)
        .await
        .expect("list_edges should succeed");

    assert!(
        active.iter().any(|e| e.id == edge.id),
        "active edge must appear in default list_edges"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 2 & 3. Superseded edge is excluded from default queries
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_supersede_edge_excludes_from_default_queries() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("src-supersede", "source content").await;
    let dst = fix.insert_article("dst-supersede", "article content").await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            dst,
            EdgeType::Confirms,
            0.9,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge should succeed");

    // Supersede the edge.
    graph
        .supersede_edge(edge.id)
        .await
        .expect("supersede_edge should succeed");

    // Verify the DB row has valid_to set.
    let valid_to: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT valid_to FROM covalence.edges WHERE id = $1")
            .bind(edge.id)
            .fetch_one(&pool)
            .await
            .expect("edge row should still exist");

    assert!(
        valid_to.is_some(),
        "valid_to must be set after supersession"
    );

    // Default list_edges must NOT return the superseded edge.
    let active = graph
        .list_edges(src, TraversalDirection::Outbound, None, 100, false)
        .await
        .expect("list_edges should succeed");

    assert!(
        !active.iter().any(|e| e.id == edge.id),
        "superseded edge must be excluded from default list_edges"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. include_superseded = true surfaces the superseded edge
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_include_superseded_returns_history() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("src-history", "source content").await;
    let dst = fix.insert_article("dst-history", "article content").await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            dst,
            EdgeType::RelatesTo,
            0.8,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge should succeed");

    graph
        .supersede_edge(edge.id)
        .await
        .expect("supersede_edge should succeed");

    // With include_superseded=true the edge must be present.
    let all_edges = graph
        .list_edges(src, TraversalDirection::Outbound, None, 100, true)
        .await
        .expect("list_edges (include_superseded) should succeed");

    let found = all_edges.iter().find(|e| e.id == edge.id);
    assert!(
        found.is_some(),
        "superseded edge must appear when include_superseded = true"
    );

    let found = found.unwrap();
    assert!(
        found.valid_to.is_some(),
        "superseded edge must carry a non-NULL valid_to"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. SharedGraph rebuild excludes superseded edges
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_shared_graph_rebuild_excludes_superseded() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("src-rebuild", "source content").await;
    let dst = fix.insert_article("dst-rebuild", "article content").await;
    let dst2 = fix.insert_article("dst2-rebuild", "other content").await;

    let graph_repo = SqlGraphRepository::new(pool.clone());

    // Create one active and one superseded edge.
    let active_edge = graph_repo
        .create_edge(
            src,
            dst,
            EdgeType::Originates,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create active edge");

    let superseded_edge = graph_repo
        .create_edge(
            src,
            dst2,
            EdgeType::Confirms,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create edge to be superseded");

    graph_repo
        .supersede_edge(superseded_edge.id)
        .await
        .expect("supersede edge");

    // Simulate the main.rs SharedGraph rebuild: only load valid_to IS NULL edges.
    let rows = sqlx::query(
        "SELECT source_node_id, target_node_id, edge_type \
         FROM covalence.edges \
         WHERE valid_to IS NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("query active edges");

    let mut rebuilt = CovalenceGraph::new();
    for row in &rows {
        use sqlx::Row as _;
        let source: Uuid = row.try_get("source_node_id").unwrap();
        let target: Uuid = row.try_get("target_node_id").unwrap();
        let edge_type: String = row.try_get("edge_type").unwrap();
        rebuilt.add_edge(source, target, edge_type);
    }

    // Active edge must be in the in-memory graph.
    let neighbors = rebuilt.neighbors(&src);
    assert!(
        neighbors.contains(&dst),
        "active edge target must be reachable in rebuilt SharedGraph"
    );

    // Superseded edge target must NOT be in the in-memory graph
    // (unless dst2 happens to be reachable via a different active edge, which it isn't here).
    assert!(
        !neighbors.contains(&dst2),
        "superseded edge target must not appear in rebuilt SharedGraph"
    );

    // Confirm we still track the active edge ID (sanity check).
    assert!(active_edge.valid_to.is_none());

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Superseding an already-superseded edge returns EdgeNotFound
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_supersede_already_superseded_returns_error() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("src-idempotent", "source content").await;
    let dst = fix
        .insert_article("dst-idempotent", "article content")
        .await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            dst,
            EdgeType::Extends,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge should succeed");

    // First supersession succeeds.
    graph
        .supersede_edge(edge.id)
        .await
        .expect("first supersession should succeed");

    // Second supersession must fail with EdgeNotFound.
    let result = graph.supersede_edge(edge.id).await;
    assert!(
        result.is_err(),
        "superseding an already-superseded edge must return an error"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. valid_from equals created_at for new edges
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_valid_from_matches_created_at() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("src-ts", "source content").await;
    let dst = fix.insert_article("dst-ts", "article content").await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            dst,
            EdgeType::Precedes,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge should succeed");

    // Fetch directly from DB to get both timestamps.
    let (created_at, valid_from): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT created_at, valid_from FROM covalence.edges WHERE id = $1")
            .bind(edge.id)
            .fetch_one(&pool)
            .await
            .expect("edge row should exist");

    // They should be within 1 second of each other (same transaction clock).
    let diff = (valid_from - created_at).num_seconds().abs();
    assert!(
        diff <= 1,
        "valid_from ({valid_from}) and created_at ({created_at}) must be within 1s"
    );

    fix.cleanup().await;
}
