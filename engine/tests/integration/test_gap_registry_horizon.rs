//! Integration tests for the calibrated NER horizon-score scanner (covalence#121).
//!
//! These tests verify that after the NER regex fix (≥ 3 capitalised words,
//! ≥ 2 article minimum occurrence), `compute_horizon_gaps` produces meaningful,
//! non-trivial horizon scores rather than uniformly 1.0.
//!
//! Two tests:
//! 1. `horizon_score_not_universally_one` — with a mix of resolved and
//!    unresolved entities, at least one domain scores < 1.0 and at least one
//!    scores > 0.0.
//! 2. `horizon_score_varies_by_domain` — two domains with different entity
//!    resolution rates must produce different horizon scores.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ─── shared helper ────────────────────────────────────────────────────────────

async fn post_maintenance(app: &axum::Router, payload: serde_json::Value) -> serde_json::Value {
    let req = Request::builder()
        .method("POST")
        .uri("/admin/maintenance")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("build maintenance request");
    let resp = app.clone().oneshot(req).await.expect("maintenance oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "POST /admin/maintenance must return 200"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read maintenance body");
    serde_json::from_slice(&bytes).expect("maintenance response JSON")
}

// ─── Test 1 ───────────────────────────────────────────────────────────────────

/// After ingesting articles with a mix of resolved and unresolved named entities,
/// `compute_horizon_gaps` must produce horizon scores that are neither universally
/// 0.0 nor universally 1.0.  The NER calibration fix (#121) must take effect.
///
/// Setup:
/// - Domain **alpha**: 3 articles all containing a 3-word entity
///   "Deep Learning Neural" (derived from a longer name). A dedicated article
///   whose title contains the entity is also inserted, so the entity IS resolved.
///   → All alpha articles are flagged as resolved → no gap_registry row
///   (or row with horizon_score = 0).
/// - Domain **beta**: 3 articles all containing entity "Xylophone Zymurgy
///   Framework" (no dedicated article) → entity is unresolved for every article.
///   → horizon_score > 0.
///
/// Assertions:
/// * beta horizon_score > 0.0
/// * at least one domain has horizon_score < 1.0 (alpha = 0.0 satisfies this)
/// * at least one domain has horizon_score > 0.0 (beta satisfies this)
#[tokio::test]
#[serial]
async fn horizon_score_not_universally_one() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let suffix = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let alpha = format!("alpha-{suffix}");
    let beta = format!("beta-{suffix}");

    // ── Domain alpha: resolved entity ─────────────────────────────────────────
    // The phrase "Deep Learning Neural Architecture" contains 4 Title-Case words;
    // the regex (≥ 3 words) will capture "Deep Learning Neural" as entity_lower
    // "deep learning neural".  The dedicated article's title begins with the same
    // string, so the LIKE check resolves it.
    let resolved_name = format!("Deep Learning Neural Architecture {suffix}");

    // Dedicated article whose title IS the resolved entity (triggers resolution).
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, status, title, content, domain_path) \
         VALUES (gen_random_uuid(), 'article', 'active', $1, 'Definition article.', ARRAY[$2])",
    )
    .bind(&resolved_name)
    .bind(&alpha)
    .execute(&pool)
    .await
    .expect("insert dedicated article for alpha");

    // 3 articles referencing the resolved entity (satisfies ≥ 2 occurrence filter).
    for i in 0..3u32 {
        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, domain_path) \
             VALUES (gen_random_uuid(), 'article', 'active', $1, $2, ARRAY[$3])",
        )
        .bind(format!("{alpha} article {i}"))
        .bind(format!("This article discusses {resolved_name}."))
        .bind(&alpha)
        .execute(&pool)
        .await
        .expect("insert alpha article");
    }

    // ── Domain beta: unresolved entity ────────────────────────────────────────
    // "Xylophone Zymurgy Framework" — 3 Title-Case words, no dedicated article.
    let unresolved_name = format!("Xylophone Zymurgy Framework {suffix}");

    // 3 articles referencing the unresolved entity (satisfies ≥ 2 occurrence filter).
    for i in 0..3u32 {
        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, domain_path) \
             VALUES (gen_random_uuid(), 'article', 'active', $1, $2, ARRAY[$3])",
        )
        .bind(format!("{beta} article {i}"))
        .bind(format!("This article discusses {unresolved_name}."))
        .bind(&beta)
        .execute(&pool)
        .await
        .expect("insert beta article");
    }

    // ── Run compute_horizon_gaps ──────────────────────────────────────────────
    let resp = post_maintenance(&app, serde_json::json!({ "compute_horizon_gaps": true })).await;
    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("compute_horizon_gaps"),
        "maintenance response must mention compute_horizon_gaps; got: {resp}"
    );

    // ── Fetch horizon scores ──────────────────────────────────────────────────
    let alpha_score: Option<f64> =
        sqlx::query_scalar("SELECT horizon_score FROM covalence.gap_registry WHERE topic = $1")
            .bind(&alpha)
            .fetch_optional(&pool)
            .await
            .expect("fetch alpha horizon_score");

    let beta_score: Option<f64> =
        sqlx::query_scalar("SELECT horizon_score FROM covalence.gap_registry WHERE topic = $1")
            .bind(&beta)
            .fetch_optional(&pool)
            .await
            .expect("fetch beta horizon_score");

    // Alpha resolved all entities → no flagged articles → no row (score = 0.0).
    let alpha_val = alpha_score.unwrap_or(0.0);
    // Beta has all entities unresolved → flagged all 3 articles → score > 0.
    let beta_val = beta_score.expect("beta domain must have a gap_registry row");

    assert!(
        beta_val > 0.0,
        "beta domain (unresolved entities) must have horizon_score > 0; got {beta_val}"
    );
    assert!(
        alpha_val < 1.0 || beta_val < 1.0,
        "horizon scores must not both be 1.0 after calibration fix (#121); \
         alpha={alpha_val}, beta={beta_val}"
    );
    // Belt-and-suspenders: at least one domain must have score > 0.
    assert!(
        alpha_val > 0.0 || beta_val > 0.0,
        "at least one domain must have horizon_score > 0.0; \
         alpha={alpha_val}, beta={beta_val}"
    );
}

// ─── Test 2 ───────────────────────────────────────────────────────────────────

/// Two domains with different entity resolution rates must produce
/// horizon scores that differ — lower for the well-resolved domain,
/// higher for the unresolved domain.
///
/// Setup:
/// - Domain **resolved-domain**: entity "Machine Learning Pipeline" has a
///   dedicated article → resolved → horizon_score = 0 (or absent).
/// - Domain **unresolved-domain**: entity "Quantum Entanglement Protocol" has
///   NO dedicated article → unresolved → horizon_score > 0.
///
/// Assertions:
/// * unresolved_domain horizon_score > resolved_domain horizon_score
/// * unresolved_domain horizon_score > 0
#[tokio::test]
#[serial]
async fn horizon_score_varies_by_domain() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let suffix = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let resolved_domain = format!("resolved-domain-{suffix}");
    let unresolved_domain = format!("unresolved-domain-{suffix}");

    // ── Resolved domain ───────────────────────────────────────────────────────
    // Entity "Machine Learning Pipeline" appears in content; the dedicated
    // article's title starts with "Machine Learning Pipeline …" so the
    // LIKE '%machine learning pipeline%' check resolves it.
    let known_entity = format!("Machine Learning Pipeline {suffix}");

    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, status, title, content, domain_path) \
         VALUES (gen_random_uuid(), 'article', 'active', $1, 'Definition.', ARRAY[$2])",
    )
    .bind(&known_entity)
    .bind(&resolved_domain)
    .execute(&pool)
    .await
    .expect("insert dedicated article for resolved entity");

    // 3 articles referencing the known entity (satisfies ≥ 2 occurrence filter).
    for i in 0..3u32 {
        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, domain_path) \
             VALUES (gen_random_uuid(), 'article', 'active', $1, $2, ARRAY[$3])",
        )
        .bind(format!("{resolved_domain} article {i}"))
        .bind(format!("This covers {known_entity}."))
        .bind(&resolved_domain)
        .execute(&pool)
        .await
        .expect("insert resolved domain article");
    }

    // ── Unresolved domain ─────────────────────────────────────────────────────
    // Entity "Quantum Entanglement Protocol" — no dedicated article.
    let unknown_entity = format!("Quantum Entanglement Protocol {suffix}");

    // 3 articles referencing the unknown entity (satisfies ≥ 2 occurrence filter).
    for i in 0..3u32 {
        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, domain_path) \
             VALUES (gen_random_uuid(), 'article', 'active', $1, $2, ARRAY[$3])",
        )
        .bind(format!("{unresolved_domain} article {i}"))
        .bind(format!("This covers {unknown_entity}."))
        .bind(&unresolved_domain)
        .execute(&pool)
        .await
        .expect("insert unresolved domain article");
    }

    // ── Run compute_horizon_gaps ──────────────────────────────────────────────
    let resp = post_maintenance(&app, serde_json::json!({ "compute_horizon_gaps": true })).await;
    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("compute_horizon_gaps"),
        "maintenance response must mention compute_horizon_gaps; got: {resp}"
    );

    // ── Fetch horizon scores ──────────────────────────────────────────────────
    // resolved_domain should have no flagged articles → no row → score 0.0.
    let resolved_score: f64 = sqlx::query_scalar(
        "SELECT COALESCE(horizon_score, 0.0) \
         FROM covalence.gap_registry WHERE topic = $1",
    )
    .bind(&resolved_domain)
    .fetch_optional(&pool)
    .await
    .expect("fetch resolved horizon_score")
    .unwrap_or(0.0_f64);

    // unresolved_domain must have a row with horizon_score > 0.
    let unresolved_score: f64 = sqlx::query_scalar(
        "SELECT COALESCE(horizon_score, 0.0) \
         FROM covalence.gap_registry WHERE topic = $1",
    )
    .bind(&unresolved_domain)
    .fetch_optional(&pool)
    .await
    .expect("fetch unresolved horizon_score")
    .unwrap_or(0.0_f64);

    assert!(
        unresolved_score > resolved_score,
        "unresolved domain (horizon={unresolved_score}) must have a higher horizon_score \
         than resolved domain (horizon={resolved_score})"
    );
    assert!(
        unresolved_score > 0.0,
        "unresolved domain must have horizon_score > 0; got {unresolved_score}"
    );
}
