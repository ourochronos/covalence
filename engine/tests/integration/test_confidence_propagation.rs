//! Integration tests for Phase-1 confidence propagation (covalence#137).
//!
//! Covers the seven required scenarios:
//!
//! 1. Single ORIGINATES source (reliability=0.75) → trust_score=0.75
//! 2. Three sources ([0.8,0.7,0.6]) → trust_score=1−(0.2×0.3×0.4)=0.976
//! 3. Zero sources → trust_score=CONF_FLOOR=0.02, flags=["no_sources"]
//! 4. Single CONTRADICTS attacker (ds_base=0.9, trust=0.8, w=1.0) → 0.18
//! 5. CONTENDS (w=0.3) vs CONTRADICTS (w=1.0) same strength → CONTENDS leaves more
//! 6. Full supersession (B.trust=1.0, w=1.0) → clamped to CONF_FLOOR=0.02
//! 7. Composed: sources=[0.9,0.7]→ds=0.97; CONTRADICTS B(0.5,w=1.0)→0.485;
//!    SUPERSEDES C(0.6,w=0.8)→≈0.252

use sqlx::PgPool;
use uuid::Uuid;

use covalence_engine::confidence::{
    constants::{CONF_FLOOR, CONTENDS_DEFAULT_WEIGHT, CONTRADICTS_DEFAULT_WEIGHT},
    recompute_article_confidence,
};

#[path = "helpers.rs"]
mod helpers;

// ── helper: get trust_score from db ──────────────────────────────────────────

async fn get_trust_score(pool: &PgPool, article_id: Uuid) -> f64 {
    sqlx::query_scalar::<_, f64>(
        "SELECT COALESCE(confidence, 0.5) FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(pool)
    .await
    .expect("article not found")
}

async fn get_confidence_breakdown(pool: &PgPool, article_id: Uuid) -> serde_json::Value {
    sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT confidence_breakdown FROM covalence.nodes WHERE id = $1",
    )
    .bind(article_id)
    .fetch_one(pool)
    .await
    .expect("article not found")
    .unwrap_or(serde_json::json!({}))
}

/// Insert a minimal source node with a given reliability score.
async fn insert_source_with_reliability(pool: &PgPool, reliability: f64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, reliability, confidence, namespace) \
         VALUES ($1, 'source', 'test source', 'content', 'active', $2, $2, 'default')",
    )
    .bind(id)
    .bind(reliability)
    .execute(pool)
    .await
    .expect("insert source");
    id
}

/// Insert a minimal article node.
async fn insert_article(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'article', 'test article', 'content', 'active', 0.5, 'default')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("insert article");
    id
}

/// Link a source to an article in article_sources.
async fn link_source(
    pool: &PgPool,
    article_id: Uuid,
    source_id: Uuid,
    relationship: &str,
    causal_weight: f32,
    confidence: f32,
) {
    sqlx::query(
        "INSERT INTO covalence.article_sources \
         (article_id, source_id, relationship, causal_weight, confidence) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT DO NOTHING",
    )
    .bind(article_id)
    .bind(source_id)
    .bind(relationship)
    .bind(causal_weight)
    .bind(confidence)
    .execute(pool)
    .await
    .expect("link_source");
}

/// Set the confidence (trust_score) of a node directly.
async fn set_node_confidence(pool: &PgPool, node_id: Uuid, confidence: f64) {
    sqlx::query("UPDATE covalence.nodes SET confidence = $1 WHERE id = $2")
        .bind(confidence)
        .bind(node_id)
        .execute(pool)
        .await
        .expect("set_node_confidence");
}

// ── Test 1: Single ORIGINATES source → trust_score == reliability ─────────

#[tokio::test]
#[serial_test::serial]
async fn test_single_originates_source() {
    let pool = helpers::setup_pool().await;

    let article_id = insert_article(&pool).await;
    let source_id = insert_source_with_reliability(&pool, 0.75).await;
    link_source(&pool, article_id, source_id, "originates", 1.0, 1.0).await;

    let mut conn = pool.acquire().await.expect("acquire conn");
    let result = recompute_article_confidence(article_id, &mut conn)
        .await
        .expect("recompute");

    assert!(
        (result.final_score - 0.75).abs() < 1e-6,
        "expected 0.75, got {}",
        result.final_score
    );
    assert!(
        result.flags.is_empty(),
        "unexpected flags: {:?}",
        result.flags
    );

    let db_score = get_trust_score(&pool, article_id).await;
    assert!(
        (db_score - 0.75).abs() < 1e-6,
        "DB score mismatch: {db_score}"
    );

    // contribution_weight should be written back
    let cw: Option<f32> = sqlx::query_scalar(
        "SELECT contribution_weight FROM covalence.article_sources \
         WHERE article_id=$1 AND source_id=$2 AND relationship='originates'",
    )
    .bind(article_id)
    .bind(source_id)
    .fetch_one(&pool)
    .await
    .expect("fetch contribution_weight");
    assert!(cw.is_some(), "contribution_weight should be written");
    assert!(
        (cw.unwrap() as f64 - 0.75).abs() < 1e-5,
        "contribution_weight mismatch: {:?}",
        cw
    );
}

// ── Test 2: Three sources → complement-product formula ────────────────────

#[tokio::test]
#[serial_test::serial]
async fn test_three_sources_ds_fusion() {
    let pool = helpers::setup_pool().await;

    let article_id = insert_article(&pool).await;
    // effective_rel = source.reliability × 1.0 × 1.0 = reliability
    for rel in [0.8_f64, 0.7, 0.6] {
        let sid = insert_source_with_reliability(&pool, rel).await;
        link_source(&pool, article_id, sid, "originates", 1.0, 1.0).await;
    }

    let mut conn = pool.acquire().await.expect("acquire conn");
    let result = recompute_article_confidence(article_id, &mut conn)
        .await
        .expect("recompute");

    // 1 − (0.2 × 0.3 × 0.4) = 1 − 0.024 = 0.976
    let expected = 1.0 - (0.2 * 0.3 * 0.4);
    assert!(
        (result.final_score - expected).abs() < 1e-6,
        "expected {expected:.6}, got {:.6}",
        result.final_score
    );
}

// ── Test 3: Zero sources → CONF_FLOOR + "no_sources" flag ─────────────────

#[tokio::test]
#[serial_test::serial]
async fn test_zero_sources_floor_and_flag() {
    let pool = helpers::setup_pool().await;

    let article_id = insert_article(&pool).await;
    // No article_sources rows inserted.

    let mut conn = pool.acquire().await.expect("acquire conn");
    let result = recompute_article_confidence(article_id, &mut conn)
        .await
        .expect("recompute");

    assert!(
        (result.final_score - CONF_FLOOR).abs() < 1e-9,
        "expected CONF_FLOOR={CONF_FLOOR}, got {}",
        result.final_score
    );
    assert!(
        result.flags.contains(&"no_sources".to_string()),
        "expected 'no_sources' flag, got {:?}",
        result.flags
    );

    // DB should also show breakdown with flags
    let bd = get_confidence_breakdown(&pool, article_id).await;
    let flags = bd["flags"].as_array().expect("flags array");
    assert!(
        flags.iter().any(|f| f.as_str() == Some("no_sources")),
        "breakdown missing 'no_sources' flag"
    );
}

// ── Test 4: CONTRADICTS attacker reduces confidence ───────────────────────

#[tokio::test]
#[serial_test::serial]
async fn test_contradicts_attacker() {
    let pool = helpers::setup_pool().await;

    let article_id = insert_article(&pool).await;

    // Set up DS base of 0.9 via one ORIGINATES source with rel=0.9
    let orig_src = insert_source_with_reliability(&pool, 0.9).await;
    link_source(&pool, article_id, orig_src, "originates", 1.0, 1.0).await;

    // Attacker: trust_score=0.8, w=1.0 (CONTRADICTS default)
    let attacker_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'source', 'attacker', 'attack content', 'active', 0.8, 'default')",
    )
    .bind(attacker_id)
    .execute(&pool)
    .await
    .expect("insert attacker");

    // causal_weight=1.0 → CONTRADICTS default
    link_source(&pool, article_id, attacker_id, "contradicts", 1.0, 1.0).await;

    let mut conn = pool.acquire().await.expect("acquire conn");
    let result = recompute_article_confidence(article_id, &mut conn)
        .await
        .expect("recompute");

    // conf_ds=0.9; attack=0.8×1.0=0.8; total_attack=0.8; result=0.9×0.2=0.18
    let expected = 0.9 * (1.0 - 0.8);
    assert!(
        (result.final_score - expected).abs() < 1e-6,
        "expected {expected:.6}, got {:.6}",
        result.final_score
    );
}

// ── Test 5: CONTENDS leaves more confidence than CONTRADICTS ──────────────

#[tokio::test]
#[serial_test::serial]
async fn test_contends_less_damaging_than_contradicts() {
    let pool = helpers::setup_pool().await;

    // Article A — has a CONTRADICTS attacker
    let article_contradicts = insert_article(&pool).await;
    let orig_c = insert_source_with_reliability(&pool, 0.9).await;
    link_source(&pool, article_contradicts, orig_c, "originates", 1.0, 1.0).await;

    let attacker_c = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'source', 'attacker', 'content', 'active', 0.8, 'default')",
    )
    .bind(attacker_c)
    .execute(&pool)
    .await
    .expect("insert");
    link_source(
        &pool,
        article_contradicts,
        attacker_c,
        "contradicts",
        CONTRADICTS_DEFAULT_WEIGHT as f32,
        1.0,
    )
    .await;

    // Article B — same base, same attacker strength, but CONTENDS
    let article_contends = insert_article(&pool).await;
    let orig_d = insert_source_with_reliability(&pool, 0.9).await;
    link_source(&pool, article_contends, orig_d, "originates", 1.0, 1.0).await;

    let attacker_d = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'source', 'attacker2', 'content', 'active', 0.8, 'default')",
    )
    .bind(attacker_d)
    .execute(&pool)
    .await
    .expect("insert");
    link_source(
        &pool,
        article_contends,
        attacker_d,
        "contends",
        CONTENDS_DEFAULT_WEIGHT as f32,
        1.0,
    )
    .await;

    let mut conn = pool.acquire().await.expect("acquire conn");
    let r_contradicts = recompute_article_confidence(article_contradicts, &mut conn)
        .await
        .expect("recompute contradicts");
    let r_contends = recompute_article_confidence(article_contends, &mut conn)
        .await
        .expect("recompute contends");

    assert!(
        r_contends.final_score > r_contradicts.final_score,
        "CONTENDS ({}) should leave more confidence than CONTRADICTS ({})",
        r_contends.final_score,
        r_contradicts.final_score
    );
}

// ── Test 6: Full supersession → clamped to CONF_FLOOR ────────────────────

#[tokio::test]
#[serial_test::serial]
async fn test_full_supersession_clamped_to_floor() {
    let pool = helpers::setup_pool().await;

    let article_id = insert_article(&pool).await;
    let orig_src = insert_source_with_reliability(&pool, 0.9).await;
    link_source(&pool, article_id, orig_src, "originates", 1.0, 1.0).await;

    // Superseder with trust_score=1.0, causal_weight=1.0 → total_supersede=1.0
    let superseder_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'source', 'superseder', 'new content', 'active', 1.0, 'default')",
    )
    .bind(superseder_id)
    .execute(&pool)
    .await
    .expect("insert superseder");
    // causal_weight=1.0 → supersede_factor=1.0×1.0=1.0
    link_source(&pool, article_id, superseder_id, "supersedes", 1.0, 1.0).await;

    let mut conn = pool.acquire().await.expect("acquire conn");
    let result = recompute_article_confidence(article_id, &mut conn)
        .await
        .expect("recompute");

    assert!(
        (result.final_score - CONF_FLOOR).abs() < 1e-9,
        "expected CONF_FLOOR={CONF_FLOOR}, got {}",
        result.final_score
    );
    assert!(
        result.flags.contains(&"floor_clamped".to_string()),
        "expected 'floor_clamped' flag, got {:?}",
        result.flags
    );
}

// ── Test 7: Full composed pipeline ───────────────────────────────────────

#[tokio::test]
#[serial_test::serial]
async fn test_composed_pipeline() {
    let pool = helpers::setup_pool().await;

    let article_id = insert_article(&pool).await;

    // ORIGINATES sources: reliabilities 0.9 and 0.7
    // DS: 1 − (0.1 × 0.3) = 1 − 0.03 = 0.97
    for rel in [0.9_f64, 0.7] {
        let sid = insert_source_with_reliability(&pool, rel).await;
        link_source(&pool, article_id, sid, "originates", 1.0, 1.0).await;
    }

    // CONTRADICTS: B with trust=0.5, w=1.0
    // total_attack=0.5; conf after penalty=0.97×0.5=0.485
    let contradicts_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'source', 'contradicts_b', 'content', 'active', 0.5, 'default')",
    )
    .bind(contradicts_id)
    .execute(&pool)
    .await
    .expect("insert");
    link_source(
        &pool,
        article_id,
        contradicts_id,
        "contradicts",
        1.0, // w=1.0
        1.0,
    )
    .await;

    // SUPERSEDES: C with trust=0.6, causal_weight=0.8
    // supersede_factor=0.6×0.8=0.48; total_supersede=0.48
    // conf after decay=0.485×(1−0.48)=0.485×0.52=0.2522
    let supersedes_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, title, content, status, confidence, namespace) \
         VALUES ($1, 'source', 'supersedes_c', 'content', 'active', 0.6, 'default')",
    )
    .bind(supersedes_id)
    .execute(&pool)
    .await
    .expect("insert");
    link_source(
        &pool,
        article_id,
        supersedes_id,
        "supersedes",
        0.8, // w=0.8
        1.0,
    )
    .await;

    let mut conn = pool.acquire().await.expect("acquire conn");
    let result = recompute_article_confidence(article_id, &mut conn)
        .await
        .expect("recompute");

    // Expected: 0.485 × 0.52 = 0.2522
    let conf_ds = 0.97_f64;
    let conf_after_attack = conf_ds * 0.5;
    let conf_after_decay = conf_after_attack * (1.0 - 0.48);
    let expected = conf_after_decay.max(CONF_FLOOR);

    assert!(
        (result.final_score - expected).abs() < 1e-4,
        "expected {expected:.6}, got {:.6}",
        result.final_score
    );

    // Verify breakdown JSON is populated correctly
    let bd = get_confidence_breakdown(&pool, article_id).await;
    assert_eq!(bd["ds_fusion"]["source_count"], 2);
    assert!((bd["ds_fusion"]["raw_score"].as_f64().unwrap() - conf_ds).abs() < 1e-4);
    assert_eq!(bd["contradicts_penalty"]["attacker_count"], 1);
    assert_eq!(bd["supersedes_decay"]["superseder_count"], 1);
}
