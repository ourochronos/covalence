//! Integration tests for KG inference rules (covalence#99).
//!
//! Verifies:
//! 1. Writing A CONTRADICTS B automatically creates the inverse B CONTRADICTS A.
//! 2. A CONFIRMS B + B CONTRADICTS C → (A, C) appears in contends_derived after refresh.
//! 3. Contradicts-symmetry enforcement is idempotent (no duplicate inverses).

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::edge_service::{CreateEdgeRequest, EdgeService};

use super::helpers::TestFixture;

// ─────────────────────────────────────────────────────────────────────────────
// Helper: refresh the contends_derived materialized view.
// ─────────────────────────────────────────────────────────────────────────────
async fn refresh_contends_derived(pool: &sqlx::PgPool) {
    sqlx::query("REFRESH MATERIALIZED VIEW CONCURRENTLY covalence.contends_derived")
        .execute(pool)
        .await
        .expect("REFRESH MATERIALIZED VIEW CONCURRENTLY should succeed");
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Write A CONTRADICTS B → inverse B CONTRADICTS A is auto-created.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_contradicts_symmetry_auto_inverse() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let node_a = fix
        .insert_source("kg-infer-a", "claim that conflicts with b")
        .await;
    let node_b = fix
        .insert_source("kg-infer-b", "claim that conflicts with a")
        .await;

    let svc = EdgeService::new(pool.clone());

    // Write A → B (CONTRADICTS).
    let req = CreateEdgeRequest {
        from_node_id: node_a,
        to_node_id: node_b,
        label: "CONTRADICTS".to_string(),
        confidence: Some(0.9),
        method: Some("test".to_string()),
        notes: None,
    };
    svc.create(req)
        .await
        .expect("EdgeService::create CONTRADICTS should succeed");

    // Verify forward edge A → B exists.
    let fwd: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'CONTRADICTS' \
           AND valid_to IS NULL",
    )
    .bind(node_a)
    .bind(node_b)
    .fetch_one(&pool)
    .await
    .expect("forward edge query");

    assert_eq!(fwd, 1, "forward A→B CONTRADICTS edge must exist");

    // Verify inverse edge B → A was auto-created.
    let inv: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'CONTRADICTS' \
           AND valid_to IS NULL",
    )
    .bind(node_b)
    .bind(node_a)
    .fetch_one(&pool)
    .await
    .expect("inverse edge query");

    assert_eq!(
        inv, 1,
        "inverse B→A CONTRADICTS edge must be auto-created by symmetry enforcement"
    );

    // Verify the inverse edge carries the expected metadata.
    let inferred_by: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'inferred_by' FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'CONTRADICTS'",
    )
    .bind(node_b)
    .bind(node_a)
    .fetch_optional(&pool)
    .await
    .expect("metadata query");

    assert_eq!(
        inferred_by.as_deref(),
        Some("symmetric_edge"),
        "inverse edge metadata must carry inferred_by = symmetric_edge (generalized label, covalence#173 wave 5)"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. A CONFIRMS B + B CONTRADICTS C → (A, C) appears in contends_derived.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_contends_derived_view_populated_after_refresh() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let node_a = fix.insert_source("kg-cd-a", "source that confirms b").await;
    let node_b = fix
        .insert_article("kg-cd-b", "article that contradicts c")
        .await;
    let node_c = fix
        .insert_article("kg-cd-c", "article contradicted by b")
        .await;

    let svc = EdgeService::new(pool.clone());

    // Write A CONFIRMS B.
    let confirms_req = CreateEdgeRequest {
        from_node_id: node_a,
        to_node_id: node_b,
        label: "CONFIRMS".to_string(),
        confidence: Some(0.95),
        method: Some("test".to_string()),
        notes: None,
    };
    svc.create(confirms_req)
        .await
        .expect("EdgeService::create CONFIRMS should succeed");

    // Write B CONTRADICTS C.
    let contradicts_req = CreateEdgeRequest {
        from_node_id: node_b,
        to_node_id: node_c,
        label: "CONTRADICTS".to_string(),
        confidence: Some(0.85),
        method: Some("test".to_string()),
        notes: None,
    };
    svc.create(contradicts_req)
        .await
        .expect("EdgeService::create CONTRADICTS should succeed");

    // Refresh the materialized view.
    refresh_contends_derived(&pool).await;

    // Assert (A, C) tuple is present in contends_derived.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.contends_derived \
         WHERE node_a_id = $1 AND node_c_id = $2",
    )
    .bind(node_a)
    .bind(node_c)
    .fetch_one(&pool)
    .await
    .expect("contends_derived query");

    assert_eq!(
        count, 1,
        "contends_derived must contain (A, C) after A CONFIRMS B and B CONTRADICTS C"
    );

    // Also verify the source edge IDs are populated correctly.
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT source_edge_1_id, source_edge_2_id \
         FROM covalence.contends_derived \
         WHERE node_a_id = $1 AND node_c_id = $2",
    )
    .bind(node_a)
    .bind(node_c)
    .fetch_optional(&pool)
    .await
    .expect("source edge IDs query");

    assert!(
        row.is_some(),
        "contends_derived row must have non-null source edge IDs"
    );

    let (edge1_id, edge2_id) = row.unwrap();

    // edge1 must be a CONFIRMS edge from A to B.
    let edge1_type: Option<String> = sqlx::query_scalar(
        "SELECT edge_type FROM covalence.edges \
         WHERE id = $1 AND source_node_id = $2 AND target_node_id = $3",
    )
    .bind(edge1_id)
    .bind(node_a)
    .bind(node_b)
    .fetch_optional(&pool)
    .await
    .expect("edge1 lookup");

    assert_eq!(
        edge1_type.as_deref(),
        Some("CONFIRMS"),
        "source_edge_1 must be the A→B CONFIRMS edge"
    );

    // edge2 must be a CONTRADICTS edge from B to C.
    let edge2_type: Option<String> = sqlx::query_scalar(
        "SELECT edge_type FROM covalence.edges \
         WHERE id = $1 AND source_node_id = $2 AND target_node_id = $3",
    )
    .bind(edge2_id)
    .bind(node_b)
    .bind(node_c)
    .fetch_optional(&pool)
    .await
    .expect("edge2 lookup");

    assert_eq!(
        edge2_type.as_deref(),
        Some("CONTRADICTS"),
        "source_edge_2 must be the B→C CONTRADICTS edge"
    );

    fix.cleanup().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Symmetry enforcement is idempotent — no duplicate inverse edges.
// ─────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial]
async fn test_contradicts_symmetry_idempotent() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let node_a = fix
        .insert_source("kg-idem-a", "claim alpha conflicts with beta")
        .await;
    let node_b = fix
        .insert_source("kg-idem-b", "claim beta conflicts with alpha")
        .await;

    let svc = EdgeService::new(pool.clone());

    // Write A → B CONTRADICTS a first time.
    let req1 = CreateEdgeRequest {
        from_node_id: node_a,
        to_node_id: node_b,
        label: "CONTRADICTS".to_string(),
        confidence: Some(0.9),
        method: Some("test".to_string()),
        notes: None,
    };
    svc.create(req1)
        .await
        .expect("first CONTRADICTS create should succeed");

    // At this point both A→B and B→A exist.
    // Now explicitly write B → A CONTRADICTS (the inverse that already exists).
    let req2 = CreateEdgeRequest {
        from_node_id: node_b,
        to_node_id: node_a,
        label: "CONTRADICTS".to_string(),
        confidence: Some(0.9),
        method: Some("test".to_string()),
        notes: None,
    };

    // The second write should succeed (ON CONFLICT DO NOTHING for the auto-inverse)
    // rather than fail with a unique-constraint violation.
    let result = svc.create(req2).await;
    assert!(
        result.is_ok(),
        "writing the explicit inverse of an existing CONTRADICTS edge must not error: {:?}",
        result
    );

    // There must be exactly one A→B CONTRADICTS edge (no duplicates).
    let fwd_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'CONTRADICTS' \
           AND valid_to IS NULL",
    )
    .bind(node_a)
    .bind(node_b)
    .fetch_one(&pool)
    .await
    .expect("forward edge count query");

    assert_eq!(
        fwd_count, 1,
        "A→B CONTRADICTS must not be duplicated (idempotency)"
    );

    // There must be exactly one B→A CONTRADICTS edge (no duplicates).
    let inv_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'CONTRADICTS' \
           AND valid_to IS NULL",
    )
    .bind(node_b)
    .bind(node_a)
    .fetch_one(&pool)
    .await
    .expect("inverse edge count query");

    assert_eq!(
        inv_count, 1,
        "B→A CONTRADICTS must not be duplicated (idempotency)"
    );

    // Total CONTRADICTS edges between A and B must be exactly 2.
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE edge_type = 'CONTRADICTS' AND valid_to IS NULL \
           AND ((source_node_id = $1 AND target_node_id = $2) \
             OR (source_node_id = $2 AND target_node_id = $1))",
    )
    .bind(node_a)
    .bind(node_b)
    .fetch_one(&pool)
    .await
    .expect("total CONTRADICTS count query");

    assert_eq!(
        total, 2,
        "exactly 2 CONTRADICTS edges between A and B: one forward, one inverse"
    );

    fix.cleanup().await;
}
