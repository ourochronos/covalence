//! Integration tests for Gap Registry Phase 2 — structural and horizon scanners
//! (covalence#120).
//!
//! Three tests:
//! 1. `structural_gaps_computed` — after inserting articles with varying edge
//!    connectivity, `compute_structural_gaps=true` upserts rows with sensible
//!    `structural_score` values into `gap_registry`.
//! 2. `horizon_gaps_computed` — after inserting an article with unresolvable
//!    capitalized multi-word phrases, `compute_horizon_gaps=true` upserts a
//!    row with `horizon_score > 0` for the matching domain.
//! 3. `gap_score_incorporates_structural_and_horizon` — when an existing
//!    `gap_registry` row already has non-zero `structural_score` and
//!    `horizon_score`, running `compute_gaps=true` produces a `gap_score`
//!    that incorporates all four formula components.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;
use uuid::Uuid;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ─── shared helpers ───────────────────────────────────────────────────────────

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

// ─── Test 4 ───────────────────────────────────────────────────────────────────

/// After inserting articles in a domain with known edge connectivity,
/// `compute_structural_gaps=true` must produce a `structural_score` row in
/// `gap_registry`.
#[tokio::test]
#[serial]
async fn structural_gaps_computed() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let domain = format!("structural-test-{}", Uuid::new_v4());

    // Insert two active articles in this domain with no edges (fully isolated).
    let art1: Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes \
         (id, node_type, status, title, content, domain_path) \
         VALUES (gen_random_uuid(), 'article', 'active', $1, 'Article one content', ARRAY[$2]) \
         RETURNING id",
    )
    .bind(format!("{domain} article one"))
    .bind(&domain)
    .fetch_one(&pool)
    .await
    .expect("insert article 1");

    let _art2: Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes \
         (id, node_type, status, title, content, domain_path) \
         VALUES (gen_random_uuid(), 'article', 'active', $1, 'Article two content', ARRAY[$2]) \
         RETURNING id",
    )
    .bind(format!("{domain} article two"))
    .bind(&domain)
    .fetch_one(&pool)
    .await
    .expect("insert article 2");

    // Connect art1 to itself via a self-edge so it has at least one edge.
    sqlx::query(
        "INSERT INTO covalence.edges \
         (id, source_node_id, target_node_id, edge_type) \
         VALUES (gen_random_uuid(), $1, $1, 'CONFIRMS')",
    )
    .bind(art1)
    .execute(&pool)
    .await
    .expect("insert edge");

    // Run structural gap computation.
    let resp = post_maintenance(&app, serde_json::json!({ "compute_structural_gaps": true })).await;
    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("compute_structural_gaps"),
        "maintenance response must mention compute_structural_gaps; got: {resp}"
    );

    // There should now be a gap_registry row for our domain with structural_score > 0.
    let structural_score: Option<f64> =
        sqlx::query_scalar("SELECT structural_score FROM covalence.gap_registry WHERE topic = $1")
            .bind(&domain)
            .fetch_optional(&pool)
            .await
            .expect("fetch structural_score");

    let score = structural_score.expect("gap_registry row must exist for domain");
    assert!(
        score >= 0.0 && score <= 1.0,
        "structural_score must be in [0, 1]; got {score}"
    );
    // With 1/2 articles connected, edge_density=0.5, article_count=2:
    // structural_score = 1.0 - (0.5*0.5 + min(2/10,1)*0.5) = 1.0 - (0.25+0.10) = 0.65
    assert!(
        score > 0.0,
        "at least one article is isolated so structural_score must be > 0"
    );
}

// ─── Test 5 ───────────────────────────────────────────────────────────────────

/// After inserting an article containing capitalized multi-word phrases that
/// don't appear in any article title, `compute_horizon_gaps=true` must produce
/// a `horizon_score > 0` for the article's domain.
#[tokio::test]
#[serial]
async fn horizon_gaps_computed() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let domain = format!("horizon-test-{}", Uuid::new_v4());
    let unique_entity = format!("Zymurgy Xylophone {}", &Uuid::new_v4().to_string()[..8]);

    // Insert an article whose content contains a unique capitalized phrase
    // that will NOT appear in any other article title.
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, status, title, content, domain_path) \
         VALUES (gen_random_uuid(), 'article', 'active', $1, $2, ARRAY[$3])",
    )
    .bind(format!("{domain} horizon article"))
    .bind(format!(
        "This article discusses {} which is a concept not covered elsewhere.",
        unique_entity
    ))
    .bind(&domain)
    .execute(&pool)
    .await
    .expect("insert article with entities");

    // Run horizon gap computation.
    let resp = post_maintenance(&app, serde_json::json!({ "compute_horizon_gaps": true })).await;
    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("compute_horizon_gaps"),
        "maintenance response must mention compute_horizon_gaps; got: {resp}"
    );

    // There should be a gap_registry row for our domain with horizon_score > 0.
    let horizon_score: Option<f64> =
        sqlx::query_scalar("SELECT horizon_score FROM covalence.gap_registry WHERE topic = $1")
            .bind(&domain)
            .fetch_optional(&pool)
            .await
            .expect("fetch horizon_score");

    let score = horizon_score.expect("gap_registry row must exist for domain");
    assert!(
        score > 0.0 && score <= 1.0,
        "horizon_score must be in (0, 1]; got {score}"
    );
}

// ─── Test 6 ───────────────────────────────────────────────────────────────────

/// When a `gap_registry` row already has non-zero `structural_score` and
/// `horizon_score`, running `compute_gaps=true` must produce a `gap_score`
/// that incorporates all four formula components:
///
/// `gap_score = 0.30*demand + 0.25*(1-quality) + 0.20*structural + 0.15*horizon`
#[tokio::test]
#[serial]
async fn gap_score_incorporates_structural_and_horizon() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let topic = format!("formula-test-{}", Uuid::new_v4());

    // Pre-insert a gap_registry row with known structural_score and horizon_score.
    let known_structural: f64 = 0.8;
    let known_horizon: f64 = 0.6;
    sqlx::query(
        "INSERT INTO covalence.gap_registry \
         (id, topic, namespace, query_count, avg_top_score, gap_score, \
          structural_score, horizon_score, status) \
         VALUES (gen_random_uuid(), $1, 'default', 0, NULL, 0.0, $2, $3, 'open')",
    )
    .bind(&topic)
    .bind(known_structural)
    .bind(known_horizon)
    .execute(&pool)
    .await
    .expect("pre-insert gap_registry row");

    // Insert enough gap_log rows to trigger compute_gaps (>= 3 queries).
    // Use low top_score (0.1) to make quality low → quality component should be high.
    for _ in 0..4 {
        sqlx::query(
            "INSERT INTO covalence.gap_log \
             (query, top_score, result_count, namespace) \
             VALUES ($1, 0.1, 1, 'default')",
        )
        .bind(&topic)
        .execute(&pool)
        .await
        .expect("insert gap_log row");
    }

    // Run compute_gaps.
    let resp = post_maintenance(&app, serde_json::json!({ "compute_gaps": true })).await;
    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("compute_gaps"),
        "maintenance response must mention compute_gaps; got: {resp}"
    );

    // Fetch the updated gap_score.
    let gap_score: f64 =
        sqlx::query_scalar("SELECT gap_score FROM covalence.gap_registry WHERE topic = $1")
            .bind(&topic)
            .fetch_one(&pool)
            .await
            .expect("fetch gap_score");

    // With avg_top_score=0.1, query_count=4 (only entry so demand=1.0):
    // demand_contribution   = 0.30 * 1.0 = 0.30
    // quality_contribution  = 0.25 * (1.0 - 0.1) = 0.225
    // structural_contribution = 0.20 * 0.8 = 0.16
    // horizon_contribution  = 0.15 * 0.6 = 0.09
    // expected gap_score ≈ 0.775
    let expected_min =
        0.30 * 0.9 + 0.25 * 0.8 + 0.20 * known_structural * 0.9 + 0.15 * known_horizon * 0.9;
    let expected_max =
        0.30 * 1.1 + 0.25 * 1.0 + 0.20 * known_structural * 1.1 + 0.15 * known_horizon * 1.1;

    assert!(
        gap_score > 0.0,
        "gap_score must be positive; got {gap_score}"
    );
    assert!(
        gap_score >= expected_min && gap_score <= expected_max,
        "gap_score {gap_score:.4} is outside expected range [{expected_min:.4}, {expected_max:.4}]. \
         structural and horizon components must be incorporated."
    );

    // Specifically, the gap_score must be higher than what demand+quality alone would give.
    let demand_quality_only = 0.30 * 1.0 + 0.25 * (1.0 - 0.1); // ≈ 0.525
    assert!(
        gap_score > demand_quality_only * 0.9,
        "gap_score {gap_score:.4} must incorporate structural ({known_structural}) \
         and horizon ({known_horizon}) components beyond demand+quality ({demand_quality_only:.4})"
    );
}
