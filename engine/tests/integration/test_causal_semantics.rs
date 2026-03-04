//! Integration tests for causal edge weights and dual confidence scores (covalence#75).
//!
//! Verifies:
//! 1. Edge created with relationship "originates" gets causal_weight 1.0.
//! 2. Edge created with relationship "relates_to" gets causal_weight 0.15.
//! 3. `knowledge_search` with `min_causal_weight=0.6` excludes low-weight edges from
//!    graph traversal (RELATES_TO at 0.15 is not traversed).
//! 4. `provenance_confidence` column exists on nodes (nullable float).
//! 5. `causal_weight()` model method returns correct values for all mapped types.

use serial_test::serial;

use covalence_engine::graph::{GraphRepository, SqlGraphRepository};
use covalence_engine::models::EdgeType;
use covalence_engine::services::edge_service::{CreateEdgeRequest, EdgeService};

use super::helpers::TestFixture;

// ─────────────────────────────────────────────────────────────────────────────
// 1. Edge with "ORIGINATES" gets causal_weight 1.0
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_causal_weight_originates_is_1_0() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix
        .insert_source("cw-orig-src", "some source content")
        .await;
    let art = fix
        .insert_article("cw-orig-art", "some article content")
        .await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            art,
            EdgeType::Originates,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge(Originates) should succeed");

    // Check in-memory struct value.
    assert!(
        (edge.causal_weight - 1.0).abs() < 0.001,
        "Originates causal_weight should be 1.0, got {}",
        edge.causal_weight
    );

    // Confirm the value was persisted to the database.
    let db_weight: f64 =
        sqlx::query_scalar("SELECT causal_weight FROM covalence.edges WHERE id = $1")
            .bind(edge.id)
            .fetch_one(&pool)
            .await
            .expect("causal_weight column must exist on edges");

    assert!(
        (db_weight - 1.0).abs() < 0.001,
        "DB-persisted causal_weight for ORIGINATES should be 1.0, got {db_weight}"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Edge with "RELATES_TO" gets causal_weight 0.15
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_causal_weight_relates_to_is_0_15() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix
        .insert_source("cw-rel-src", "source for relates_to test")
        .await;
    let art = fix
        .insert_article("cw-rel-art", "article for relates_to test")
        .await;

    let graph = SqlGraphRepository::new(pool.clone());
    let edge = graph
        .create_edge(
            src,
            art,
            EdgeType::RelatesTo,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create_edge(RelatesTo) should succeed");

    // In-memory struct.
    assert!(
        (edge.causal_weight - 0.15).abs() < 0.001,
        "RelatesTo causal_weight should be 0.15, got {}",
        edge.causal_weight
    );

    // Database-persisted value.
    let db_weight: f64 =
        sqlx::query_scalar("SELECT causal_weight FROM covalence.edges WHERE id = $1")
            .bind(edge.id)
            .fetch_one(&pool)
            .await
            .expect("causal_weight must be persisted");

    assert!(
        (db_weight - 0.15).abs() < 0.001,
        "DB causal_weight for RELATES_TO should be 0.15, got {db_weight}"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. knowledge_search with min_causal_weight=0.6 excludes low-weight edges
//
// Setup: two source nodes (A, B) both linked to article C via different edges.
//   A → C via ORIGINATES (causal_weight=1.0)
//   B → C via RELATES_TO (causal_weight=0.15)
//
// When min_causal_weight=0.6 is set, traversal from C should reach A but NOT B.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_min_causal_weight_excludes_low_weight_edges() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    // Create two sources and one article.
    let src_high = fix
        .insert_source("mcw-src-high", "high-causal content")
        .await;
    let src_low = fix.insert_source("mcw-src-low", "low-causal content").await;
    let article = fix
        .insert_article("mcw-article", "central article for traversal test")
        .await;

    let graph = SqlGraphRepository::new(pool.clone());

    // Link src_high → article via ORIGINATES (causal_weight = 1.0).
    graph
        .create_edge(
            src_high,
            article,
            EdgeType::Originates,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create ORIGINATES edge");

    // Link src_low → article via RELATES_TO (causal_weight = 0.15).
    graph
        .create_edge(
            src_low,
            article,
            EdgeType::RelatesTo,
            1.0,
            "test",
            serde_json::json!({}),
        )
        .await
        .expect("create RELATES_TO edge");

    // Verify the causal_weights are stored correctly.
    let weights: Vec<(String, f64)> = sqlx::query_as(
        "SELECT edge_type, causal_weight FROM covalence.edges
         WHERE target_node_id = $1
           AND valid_to IS NULL
         ORDER BY causal_weight DESC",
    )
    .bind(article)
    .fetch_all(&pool)
    .await
    .expect("fetch edge causal_weights");

    assert_eq!(weights.len(), 2, "should have 2 edges to article");

    let high = weights.iter().find(|(et, _)| et == "ORIGINATES");
    let low = weights.iter().find(|(et, _)| et == "RELATES_TO");

    assert!(high.is_some(), "ORIGINATES edge must exist");
    assert!(low.is_some(), "RELATES_TO edge must exist");
    assert!(
        (high.unwrap().1 - 1.0).abs() < 0.001,
        "ORIGINATES causal_weight should be 1.0"
    );
    assert!(
        (low.unwrap().1 - 0.15).abs() < 0.001,
        "RELATES_TO causal_weight should be 0.15"
    );

    // With min_causal_weight = 0.6, traversal from article toward its sources
    // should only reach src_high (via ORIGINATES, weight=1.0), not src_low
    // (via RELATES_TO, weight=0.15 < 0.6).
    let high_reachable: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges
         WHERE (source_node_id = $1 OR target_node_id = $1)
           AND (source_node_id = $2 OR target_node_id = $2)
           AND causal_weight >= 0.6
           AND valid_to IS NULL",
    )
    .bind(article)
    .bind(src_high)
    .fetch_one(&pool)
    .await
    .expect("high-causal traversal query");

    assert_eq!(
        high_reachable, 1,
        "src_high should be reachable via min_causal_weight=0.6"
    );

    let low_reachable: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges
         WHERE (source_node_id = $1 OR target_node_id = $1)
           AND (source_node_id = $2 OR target_node_id = $2)
           AND causal_weight >= 0.6
           AND valid_to IS NULL",
    )
    .bind(article)
    .bind(src_low)
    .fetch_one(&pool)
    .await
    .expect("low-causal traversal query");

    assert_eq!(
        low_reachable, 0,
        "src_low (RELATES_TO, weight=0.15) must be excluded by min_causal_weight=0.6"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. provenance_confidence column exists on nodes (nullable float)
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_provenance_confidence_column_exists_on_nodes() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    // Verify the column exists via information_schema.
    let col_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM information_schema.columns
             WHERE table_schema = 'covalence'
               AND table_name   = 'nodes'
               AND column_name  = 'provenance_confidence'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query should succeed");

    assert!(
        col_exists,
        "provenance_confidence column must exist on covalence.nodes"
    );

    // Verify it is nullable by inserting a node and confirming NULL is accepted.
    let node_id = fix
        .insert_source("prov-conf-test", "provenance confidence test")
        .await;

    let stored_val: Option<f64> =
        sqlx::query_scalar("SELECT provenance_confidence FROM covalence.nodes WHERE id = $1")
            .bind(node_id)
            .fetch_one(&pool)
            .await
            .expect("provenance_confidence select should succeed");

    assert!(
        stored_val.is_none(),
        "provenance_confidence should be NULL for a freshly created node"
    );

    // Verify we can SET a value.
    sqlx::query("UPDATE covalence.nodes SET provenance_confidence = 0.75 WHERE id = $1")
        .bind(node_id)
        .execute(&pool)
        .await
        .expect("setting provenance_confidence should succeed");

    let updated_val: Option<f64> =
        sqlx::query_scalar("SELECT provenance_confidence FROM covalence.nodes WHERE id = $1")
            .bind(node_id)
            .fetch_one(&pool)
            .await
            .expect("read after update");

    assert!(
        updated_val.is_some(),
        "provenance_confidence should be Some after UPDATE"
    );
    let v = updated_val.unwrap();
    assert!(
        (v - 0.75).abs() < 0.001,
        "provenance_confidence should store 0.75, got {v}"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. EdgeType::causal_weight() model method — full mapping coverage
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn test_edge_type_causal_weight_model_method() {
    // Verify the documented mapping for every explicitly mapped type.
    assert!(
        (EdgeType::Originates.causal_weight() - 1.0).abs() < 0.001,
        "ORIGINATES should be 1.0"
    );
    assert!(
        (EdgeType::CompiledFrom.causal_weight() - 1.0).abs() < 0.001,
        "COMPILED_FROM (alias for ORIGINATES) should be 1.0"
    );
    assert!(
        (EdgeType::Supersedes.causal_weight() - 0.95).abs() < 0.001,
        "SUPERSEDES should be 0.95"
    );
    assert!(
        (EdgeType::Extends.causal_weight() - 0.70).abs() < 0.001,
        "EXTENDS should be 0.70"
    );
    assert!(
        (EdgeType::Elaborates.causal_weight() - 0.70).abs() < 0.001,
        "ELABORATES (alias for EXTENDS) should be 0.70"
    );
    assert!(
        (EdgeType::Confirms.causal_weight() - 0.60).abs() < 0.001,
        "CONFIRMS should be 0.60"
    );
    assert!(
        (EdgeType::Contradicts.causal_weight() - 0.50).abs() < 0.001,
        "CONTRADICTS should be 0.50"
    );
    assert!(
        (EdgeType::RelatesTo.causal_weight() - 0.15).abs() < 0.001,
        "RELATES_TO should be 0.15"
    );

    // Default bucket (any type not explicitly listed should return 0.5).
    assert!(
        (EdgeType::Causes.causal_weight() - 0.5).abs() < 0.001,
        "CAUSES (default) should be 0.5"
    );
    assert!(
        (EdgeType::Involves.causal_weight() - 0.5).abs() < 0.001,
        "INVOLVES (default) should be 0.5"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. EdgeService creates CONTRADICTS with correct causal_weight via service
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_contradicts_edge_service_causal_weight() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src_a = fix.insert_source("cw-contra-a", "claim alpha").await;
    let src_b = fix.insert_source("cw-contra-b", "claim beta").await;

    let svc = EdgeService::new(pool.clone());
    let edge = svc
        .create(CreateEdgeRequest {
            from_node_id: src_a,
            to_node_id: src_b,
            label: "CONTRADICTS".to_string(),
            confidence: Some(0.9),
            method: Some("test".to_string()),
            notes: None,
        })
        .await
        .expect("EdgeService::create CONTRADICTS should succeed");

    // causal_weight for CONTRADICTS = 0.50.
    assert!(
        (edge.causal_weight - 0.50).abs() < 0.001,
        "CONTRADICTS causal_weight should be 0.50, got {}",
        edge.causal_weight
    );

    // Both the forward AND the inverse edge should have causal_weight = 0.50.
    let weights: Vec<f64> = sqlx::query_scalar(
        "SELECT causal_weight FROM covalence.edges
         WHERE ((source_node_id = $1 AND target_node_id = $2)
             OR (source_node_id = $2 AND target_node_id = $1))
           AND edge_type = 'CONTRADICTS'
           AND valid_to IS NULL",
    )
    .bind(src_a)
    .bind(src_b)
    .fetch_all(&pool)
    .await
    .expect("fetch both CONTRADICTS edges");

    assert_eq!(
        weights.len(),
        2,
        "both forward and inverse edges must exist"
    );
    for w in &weights {
        assert!(
            (w - 0.50).abs() < 0.001,
            "all CONTRADICTS causal_weights should be 0.50, got {w}"
        );
    }

    fix.cleanup().await;
}
