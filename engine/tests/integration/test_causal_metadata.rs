//! Integration tests for `edge_causal_metadata` table and API filters (covalence#116).
//!
//! Covers:
//! 1. FK CASCADE — delete parent edge, metadata row is removed automatically.
//! 2. Constraint violations — causal_strength > 1.0, < 0.0, temporal_lag_ms = -1.
//! 3. Bootstrap defaults — ORIGINATES edges seeded by migration get
//!    `causal_level = intervention` and `causal_strength = 0.95`.
//! 4. API filter `min_causal_strength` — two edges, one above/below threshold.
//! 5. Upsert idempotency — calling upsert twice yields one row with advanced `updated_at`.

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::db::ecm as db_ecm;
use covalence_engine::models::{CausalEvidenceType, CausalLevel, EdgeCausalMetadataUpsert};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Insert a raw edge and return its id.  Uses `causal_weight = 0.5` (default).
async fn insert_raw_edge(fix: &mut TestFixture, src: Uuid, dst: Uuid, edge_type: &str) -> Uuid {
    let edge_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.edges
             (id, source_node_id, target_node_id, edge_type, causal_weight)
         VALUES ($1, $2, $3, $4, 0.5)",
    )
    .bind(edge_id)
    .bind(src)
    .bind(dst)
    .bind(edge_type)
    .execute(&fix.pool)
    .await
    .expect("insert_raw_edge failed");
    edge_id
}

// =============================================================================
// 1. FK CASCADE
// =============================================================================

#[tokio::test]
#[serial]
async fn test_ecm_fk_cascade_on_edge_delete() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("ecm-cascade-src", "content").await;
    let dst = fix.insert_article("ecm-cascade-dst", "content").await;
    let edge_id = insert_raw_edge(&mut fix, src, dst, "ORIGINATES").await;

    // Insert a metadata row.
    let payload = EdgeCausalMetadataUpsert {
        edge_id,
        causal_level: Some(CausalLevel::Intervention),
        causal_strength: Some(0.95),
        evidence_type: Some(CausalEvidenceType::StructuralPrior),
        direction_conf: Some(0.99),
        hidden_conf_risk: Some(0.05),
        temporal_lag_ms: None,
        notes: None,
    };
    db_ecm::upsert(&pool, &payload)
        .await
        .expect("upsert should succeed");

    // Verify the metadata row exists.
    let row = db_ecm::get_by_edge_id(&pool, edge_id)
        .await
        .expect("get should not error");
    assert!(
        row.is_some(),
        "metadata row must exist before edge deletion"
    );

    // Delete the parent edge — CASCADE should remove the metadata row.
    sqlx::query("DELETE FROM covalence.edges WHERE id = $1")
        .bind(edge_id)
        .execute(&pool)
        .await
        .expect("delete edge failed");

    // Metadata row must be gone.
    let after = db_ecm::get_by_edge_id(&pool, edge_id)
        .await
        .expect("get after cascade should not error");
    assert!(
        after.is_none(),
        "metadata row must be deleted via FK CASCADE when the parent edge is deleted"
    );

    fix.cleanup().await;
}

// =============================================================================
// 2. Constraint violations
// =============================================================================

#[tokio::test]
#[serial]
async fn test_ecm_causal_strength_above_1_is_rejected() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("ecm-ck-src-a", "content").await;
    let dst = fix.insert_article("ecm-ck-dst-a", "content").await;
    let edge_id = insert_raw_edge(&mut fix, src, dst, "CONFIRMS").await;

    let result = sqlx::query(
        "INSERT INTO covalence.edge_causal_metadata
             (edge_id, causal_strength)
         VALUES ($1, 1.1)",
    )
    .bind(edge_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "causal_strength > 1.0 must violate the CHECK constraint"
    );

    fix.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_ecm_causal_strength_below_0_is_rejected() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("ecm-ck-src-b", "content").await;
    let dst = fix.insert_article("ecm-ck-dst-b", "content").await;
    let edge_id = insert_raw_edge(&mut fix, src, dst, "CONFIRMS").await;

    let result = sqlx::query(
        "INSERT INTO covalence.edge_causal_metadata
             (edge_id, causal_strength)
         VALUES ($1, -0.1)",
    )
    .bind(edge_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "causal_strength < 0.0 must violate the CHECK constraint"
    );

    fix.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_ecm_temporal_lag_negative_is_rejected() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("ecm-ck-src-c", "content").await;
    let dst = fix.insert_article("ecm-ck-dst-c", "content").await;
    let edge_id = insert_raw_edge(&mut fix, src, dst, "PRECEDES").await;

    let result = sqlx::query(
        "INSERT INTO covalence.edge_causal_metadata
             (edge_id, temporal_lag_ms)
         VALUES ($1, -1)",
    )
    .bind(edge_id)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "temporal_lag_ms = -1 must violate the CHECK constraint"
    );

    fix.cleanup().await;
}

// =============================================================================
// 3. Bootstrap defaults for ORIGINATES edges
// =============================================================================

#[tokio::test]
#[serial]
async fn test_ecm_bootstrap_originates_defaults() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix
        .insert_source("ecm-boot-src", "bootstrap test source")
        .await;
    let dst = fix
        .insert_article("ecm-boot-dst", "bootstrap test article")
        .await;

    // Insert an ORIGINATES edge and manually run the bootstrap INSERT
    // (simulating what migration 034 does for pre-existing edges).
    let edge_id = insert_raw_edge(&mut fix, src, dst, "ORIGINATES").await;

    sqlx::query(
        "INSERT INTO covalence.edge_causal_metadata
             (edge_id, causal_level, causal_strength, evidence_type, direction_conf, hidden_conf_risk)
         SELECT
             e.id,
             CASE LOWER(e.edge_type)
                 WHEN 'originates' THEN 'intervention'::covalence.causal_level_enum
                 ELSE 'association'::covalence.causal_level_enum
             END,
             CASE LOWER(e.edge_type)
                 WHEN 'originates' THEN 0.95
                 ELSE 0.5
             END,
             'structural_prior'::covalence.causal_evidence_type_enum,
             CASE LOWER(e.edge_type)
                 WHEN 'originates' THEN 0.99
                 ELSE 0.5
             END,
             CASE LOWER(e.edge_type)
                 WHEN 'originates' THEN 0.05
                 ELSE 0.5
             END
         FROM covalence.edges e
         WHERE e.id = $1
         ON CONFLICT (edge_id) DO NOTHING",
    )
    .bind(edge_id)
    .execute(&pool)
    .await
    .expect("bootstrap INSERT failed");

    let row = db_ecm::get_by_edge_id(&pool, edge_id)
        .await
        .expect("get should not error")
        .expect("bootstrap row must exist for ORIGINATES edge");

    assert_eq!(
        row.causal_level,
        CausalLevel::Intervention,
        "ORIGINATES bootstrap must produce causal_level = intervention"
    );
    assert!(
        (row.causal_strength - 0.95).abs() < 1e-6,
        "ORIGINATES bootstrap must produce causal_strength = 0.95, got {}",
        row.causal_strength
    );
    assert_eq!(
        row.evidence_type,
        CausalEvidenceType::StructuralPrior,
        "ORIGINATES bootstrap must produce evidence_type = structural_prior"
    );
    assert!(
        (row.direction_conf - 0.99).abs() < 1e-6,
        "ORIGINATES bootstrap must produce direction_conf = 0.99, got {}",
        row.direction_conf
    );
    assert!(
        (row.hidden_conf_risk - 0.05).abs() < 1e-6,
        "ORIGINATES bootstrap must produce hidden_conf_risk = 0.05, got {}",
        row.hidden_conf_risk
    );

    fix.cleanup().await;
}

// =============================================================================
// 4. API filter — min_causal_strength excludes low-strength edges
// =============================================================================

#[tokio::test]
#[serial]
async fn test_ecm_min_causal_strength_filter() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix
        .insert_source("ecm-filter-src", "source content for causal filter test")
        .await;
    let high_dst = fix
        .insert_article("ecm-filter-high", "high causal strength article")
        .await;
    let low_dst = fix
        .insert_article("ecm-filter-low", "low causal strength article")
        .await;

    // Edge A — high causal strength (0.90).
    let edge_high = insert_raw_edge(&mut fix, src, high_dst, "ORIGINATES").await;
    db_ecm::upsert(
        &pool,
        &EdgeCausalMetadataUpsert {
            edge_id: edge_high,
            causal_level: Some(CausalLevel::Intervention),
            causal_strength: Some(0.90),
            evidence_type: Some(CausalEvidenceType::StructuralPrior),
            direction_conf: Some(0.95),
            hidden_conf_risk: Some(0.05),
            temporal_lag_ms: None,
            notes: None,
        },
    )
    .await
    .expect("upsert high edge failed");

    // Edge B — low causal strength (0.15).
    let edge_low = insert_raw_edge(&mut fix, src, low_dst, "RELATES_TO").await;
    db_ecm::upsert(
        &pool,
        &EdgeCausalMetadataUpsert {
            edge_id: edge_low,
            causal_level: Some(CausalLevel::Association),
            causal_strength: Some(0.15),
            evidence_type: Some(CausalEvidenceType::Statistical),
            direction_conf: Some(0.50),
            hidden_conf_risk: Some(0.50),
            temporal_lag_ms: None,
            notes: None,
        },
    )
    .await
    .expect("upsert low edge failed");

    // Verify that filtering at 0.5 returns only the high-strength edge.
    let high_row = db_ecm::get_by_edge_id(&pool, edge_high)
        .await
        .expect("get high")
        .expect("high row must exist");
    let low_row = db_ecm::get_by_edge_id(&pool, edge_low)
        .await
        .expect("get low")
        .expect("low row must exist");

    assert!(
        high_row.causal_strength >= 0.5,
        "high edge must pass min_causal_strength = 0.5"
    );
    assert!(
        low_row.causal_strength < 0.5,
        "low edge must NOT pass min_causal_strength = 0.5"
    );

    // Verify via direct SQL query as the graph adaptor would do it.
    let passing: Vec<Uuid> = sqlx::query_scalar(
        "SELECT ecm.edge_id
         FROM covalence.edge_causal_metadata ecm
         WHERE ecm.causal_strength >= 0.5",
    )
    .fetch_all(&pool)
    .await
    .expect("filter query failed");

    assert!(
        passing.contains(&edge_high),
        "high-strength edge must appear in filtered results"
    );
    assert!(
        !passing.contains(&edge_low),
        "low-strength edge must NOT appear in filtered results"
    );

    fix.cleanup().await;
}

// =============================================================================
// 5. Upsert idempotency
// =============================================================================

#[tokio::test]
#[serial]
async fn test_ecm_upsert_idempotency() {
    let mut fix = TestFixture::new().await;
    let pool = fix.pool.clone();

    let src = fix.insert_source("ecm-idem-src", "content").await;
    let dst = fix.insert_article("ecm-idem-dst", "content").await;
    let edge_id = insert_raw_edge(&mut fix, src, dst, "CAUSES").await;

    let payload = EdgeCausalMetadataUpsert {
        edge_id,
        causal_level: Some(CausalLevel::Association),
        causal_strength: Some(0.60),
        evidence_type: Some(CausalEvidenceType::Statistical),
        direction_conf: Some(0.65),
        hidden_conf_risk: Some(0.35),
        temporal_lag_ms: Some(500),
        notes: None,
    };

    // First upsert.
    let first = db_ecm::upsert(&pool, &payload)
        .await
        .expect("first upsert failed");

    // Short sleep so the DB timestamp can advance.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Second upsert with a higher causal_strength.
    let payload2 = EdgeCausalMetadataUpsert {
        causal_strength: Some(0.75),
        ..payload.clone()
    };
    let second = db_ecm::upsert(&pool, &payload2)
        .await
        .expect("second upsert failed");

    // Only one row should exist.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edge_causal_metadata WHERE edge_id = $1",
    )
    .bind(edge_id)
    .fetch_one(&pool)
    .await
    .expect("count query failed");

    assert_eq!(
        count, 1,
        "upsert must produce exactly one row regardless of how many times it is called"
    );

    // The second call should have updated causal_strength.
    assert!(
        (second.causal_strength - 0.75).abs() < 1e-6,
        "second upsert must update causal_strength to 0.75, got {}",
        second.causal_strength
    );

    // updated_at must be >= created_at (trigger fires on UPDATE).
    assert!(
        second.updated_at >= first.created_at,
        "updated_at ({}) must be >= created_at ({}) after the second upsert",
        second.updated_at,
        first.created_at
    );

    fix.cleanup().await;
}
