//! Integration tests for EWC structural importance scoring and eviction guard
//! (covalence#101).
//!
//! Three tests:
//! 1. `test_structural_importance_computation` — verifies that running
//!    compute_structural_importance updates scores so that a hub node (high
//!    in-degree) has a higher score than leaf nodes (in-degree 0).
//! 2. `test_high_structural_importance_eviction_guard` — verifies that articles
//!    with structural_importance >= 0.8 are protected from eviction.
//! 3. `test_evict_score_ordering` — verifies that the article with the lowest
//!    combined evict_score is evicted first when multiple candidates exist.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;
use uuid::Uuid;

use covalence_engine::api::routes;

use super::helpers::{make_test_state, setup_pool};

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn post_maintenance(app: &axum::Router, payload: serde_json::Value) -> serde_json::Value {
    let req = Request::builder()
        .method("POST")
        .uri("/admin/maintenance")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("build maintenance request");
    let resp = app.clone().oneshot(req).await.expect("maintenance oneshot");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read maintenance body");
    serde_json::from_slice(&bytes).expect("maintenance response JSON")
}

/// GET /articles/{id} — returns the HTTP status code.
async fn article_status(app: &axum::Router, id: Uuid) -> StatusCode {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/articles/{id}"))
        .body(Body::empty())
        .expect("build article request");
    app.clone()
        .oneshot(req)
        .await
        .expect("article oneshot")
        .status()
}

/// Insert an active article with the given usage_score.
async fn insert_article(pool: &sqlx::PgPool, title: &str, usage_score: f64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
         (id, node_type, status, title, content, usage_score, metadata) \
         VALUES ($1, 'article', 'active', $2, $2, $3, '{}'::jsonb)",
    )
    .bind(id)
    .bind(title)
    .bind(usage_score)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("insert_article({title}) failed: {e}"));
    id
}

/// Insert a CONFIRMS edge from `src` → `dst`.
async fn insert_confirms_edge(pool: &sqlx::PgPool, src: Uuid, dst: Uuid) {
    sqlx::query(
        "INSERT INTO covalence.edges \
         (source_node_id, target_node_id, edge_type) \
         VALUES ($1, $2, 'CONFIRMS')",
    )
    .bind(src)
    .bind(dst)
    .execute(pool)
    .await
    .expect("insert_confirms_edge failed");
}

/// Fetch structural_importance for a node.
async fn get_structural_importance(pool: &sqlx::PgPool, id: Uuid) -> f64 {
    sqlx::query_scalar::<_, f64>("SELECT structural_importance FROM covalence.nodes WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|_| panic!("node {id} not found"))
}

/// Set structural_importance directly via SQL (used to set up eviction scenarios).
async fn set_structural_importance(pool: &sqlx::PgPool, id: Uuid, value: f64) {
    sqlx::query("UPDATE covalence.nodes SET structural_importance = $1 WHERE id = $2")
        .bind(value)
        .bind(id)
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("set_structural_importance({id}) failed: {e}"));
}

/// Archive every active article whose ID is **not** in `keep_ids`.
///
/// Call this after inserting the test's own fixture articles and before
/// triggering eviction.  It removes any DB pollution left by earlier tests
/// or by concurrently-running non-serial tests, so that the eviction logic
/// sees exactly the articles that belong to this test.
async fn archive_preexisting_articles(pool: &sqlx::PgPool, keep_ids: &[Uuid]) {
    sqlx::query(
        "UPDATE covalence.nodes \
         SET status = 'archived' \
         WHERE node_type = 'article' \
           AND status   = 'active' \
           AND id != ALL($1::uuid[])",
    )
    .bind(keep_ids)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("archive_preexisting_articles failed: {e}"));
}

/// Returns true iff the article exists and is active.
async fn article_is_active(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM covalence.nodes WHERE id = $1 AND status = 'active')",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

// ─── Test 1: structural_importance computation ────────────────────────────────

/// Create 3 articles (A, B, C).  B→A CONFIRMS and C→A CONFIRMS (A has
/// in_degree=2, B and C have in_degree=0).  After running
/// compute_structural_importance=true, A should have a higher score than B.
#[tokio::test]
#[serial]
async fn test_structural_importance_computation() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Insert 3 articles.
    let a = insert_article(&pool, "hub-article-A", 0.5).await;
    let b = insert_article(&pool, "leaf-article-B", 0.5).await;
    let c = insert_article(&pool, "leaf-article-C", 0.5).await;

    // Remove any articles left by earlier tests or by concurrently-running
    // tests so that structural importance scores are computed only over our
    // known topology.
    archive_preexisting_articles(&pool, &[a, b, c]).await;

    // Create edges: B→A and C→A  (A has in_degree=2).
    insert_confirms_edge(&pool, b, a).await;
    insert_confirms_edge(&pool, c, a).await;

    // Trigger structural importance computation.
    let resp = post_maintenance(
        &app,
        serde_json::json!({ "compute_structural_importance": true }),
    )
    .await;

    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("compute_structural_importance"),
        "maintenance response must mention compute_structural_importance; got: {resp}"
    );

    // Fetch scores.
    let score_a = get_structural_importance(&pool, a).await;
    let score_b = get_structural_importance(&pool, b).await;

    assert!(
        score_a > 0.0,
        "hub article A must have structural_importance > 0.0; got {score_a}"
    );
    assert!(
        score_a > score_b,
        "hub article A (in_degree=2) must have higher structural_importance \
         than leaf B (in_degree=0); A={score_a}, B={score_b}"
    );
}

// ─── Test 2: eviction guard for high structural_importance ────────────────────

/// Articles with structural_importance >= 0.8 must not be evicted, even if
/// their usage_score is very low.
#[tokio::test]
#[serial]
async fn test_high_structural_importance_eviction_guard() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Create two articles with very low usage scores.
    let high = insert_article(&pool, "high-importance-article", 0.01).await;
    let low = insert_article(&pool, "low-importance-article", 0.01).await;

    // HIGH gets structural_importance = 0.9 (protected).
    // LOW gets structural_importance = 0.0 (eligible).
    set_structural_importance(&pool, high, 0.9).await;
    set_structural_importance(&pool, low, 0.0).await;

    // Archive any articles that do not belong to this test so that the
    // eviction count (active_articles - max) is exactly 1 regardless of
    // what other tests may have left in the DB.
    archive_preexisting_articles(&pool, &[high, low]).await;

    // Force capacity limit to trigger eviction: set max to 1 but we have 2.
    // The maintenance handler reads COVALENCE_MAX_ARTICLES at call time.
    // SAFETY: integration tests run single-threaded (serial), so mutating the
    // environment is safe here.
    unsafe {
        std::env::set_var("COVALENCE_MAX_ARTICLES", "1");
    }

    let resp = post_maintenance(
        &app,
        serde_json::json!({
            "evict_if_over_capacity": true,
            "evict_count": 1
        }),
    )
    .await;

    // Reset env var to avoid polluting other tests.
    unsafe {
        std::env::remove_var("COVALENCE_MAX_ARTICLES");
    }

    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("evicted") || actions.contains("no eviction"),
        "unexpected maintenance response: {resp}"
    );

    // The HIGH article (structural_importance=0.9) must still be active.
    assert!(
        article_is_active(&pool, high).await,
        "article with structural_importance=0.9 must NOT be evicted"
    );

    // The LOW article (structural_importance=0.0) must have been archived.
    assert!(
        !article_is_active(&pool, low).await,
        "article with structural_importance=0.0 must be evicted when over capacity"
    );
}

// ─── Test 3: evict_score ordering ────────────────────────────────────────────

/// Create 3 articles:
///   A: structural=0.8  → protected (>= 0.8 guard)
///   B: structural=0.1, usage low
///   C: structural=0.0, usage low
///
/// After evicting 1, C should be gone (lowest evict_score) and A must remain.
#[tokio::test]
#[serial]
async fn test_evict_score_ordering() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    let a = insert_article(&pool, "structural-0.8-article", 0.01).await;
    let b = insert_article(&pool, "structural-0.1-article", 0.01).await;
    let c = insert_article(&pool, "structural-0.0-article", 0.01).await;

    set_structural_importance(&pool, a, 0.8).await;
    set_structural_importance(&pool, b, 0.1).await;
    set_structural_importance(&pool, c, 0.0).await;

    // Archive any articles that were left by prior tests or inserted by
    // concurrently-running tests, so the eviction logic sees exactly {a, b, c}
    // and the MAX_ARTICLES=2 threshold triggers exactly one eviction of c.
    archive_preexisting_articles(&pool, &[a, b, c]).await;

    // Force eviction with max=2 (we have 3 active articles).
    // SAFETY: integration tests run single-threaded (serial).
    unsafe {
        std::env::set_var("COVALENCE_MAX_ARTICLES", "2");
    }

    let resp = post_maintenance(
        &app,
        serde_json::json!({
            "evict_if_over_capacity": true,
            "evict_count": 1
        }),
    )
    .await;

    unsafe {
        std::env::remove_var("COVALENCE_MAX_ARTICLES");
    }

    let actions = resp["data"]["actions_taken"].to_string();
    assert!(
        actions.contains("evicted"),
        "expected eviction to occur; got: {resp}"
    );

    // A must survive (structural_importance=0.8 → >= 0.8 guard).
    assert!(
        article_is_active(&pool, a).await,
        "article A (structural=0.8) must be protected by eviction guard"
    );

    // C must be gone (structural=0.0, lowest evict_score).
    assert!(
        !article_is_active(&pool, c).await,
        "article C (structural=0.0) must be evicted (lowest evict_score)"
    );
}
