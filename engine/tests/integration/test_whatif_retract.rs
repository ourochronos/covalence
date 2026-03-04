//! Integration tests for `POST /admin/whatif/retract` (covalence#119).
//!
//! Verifies non-destructive source retraction preview using the HypoPG
//! pattern (transaction always rolled back).

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;

use covalence_engine::api::routes;

use super::helpers::{TestFixture, make_test_state};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn make_app(pool: sqlx::PgPool) -> axum::Router {
    let state = make_test_state(pool).await;
    routes::router().with_state(state)
}

async fn post_whatif_retract(
    app: axum::Router,
    source_id: uuid::Uuid,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "source_id": source_id });
    let req = Request::builder()
        .method("POST")
        .uri("/admin/whatif/retract")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request builder failed");

    let resp = app.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("failed to read body");
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

// ── test 1: orphan case ───────────────────────────────────────────────────────

/// A single source compiled into a single article.  Retracting the source
/// should mark the article as "orphaned" and set `safe_to_remove = false`.
#[tokio::test]
#[serial]
async fn test_whatif_retract_orphan_case() {
    let mut fix = TestFixture::new().await;

    // Insert source with explicit reliability
    let source_id = fix
        .insert_source("orphan-source", "Sole provenance for this article")
        .await;
    sqlx::query("UPDATE covalence.nodes SET reliability = 0.7 WHERE id = $1")
        .bind(source_id)
        .execute(&fix.pool)
        .await
        .expect("set reliability");

    // Insert article
    let article_id = fix
        .insert_article("orphan-article", "Article with only one source")
        .await;

    // Link source → article via ORIGINATES edge
    fix.insert_originates_edge(source_id, article_id).await;

    let app = make_app(fix.pool.clone()).await;
    let (status, body) = post_whatif_retract(app, source_id).await;

    assert_eq!(status, StatusCode::OK, "expected 200: {body}");

    let data = &body["data"];
    assert_eq!(
        data["safe_to_remove"].as_bool(),
        Some(false),
        "single-source retraction must not be safe: {body}"
    );
    assert_eq!(
        data["orphaned_count"].as_i64(),
        Some(1),
        "should have 1 orphaned article: {body}"
    );
    assert_eq!(
        data["degraded_count"].as_i64(),
        Some(0),
        "degraded_count should be 0: {body}"
    );

    let articles = data["affected_articles"]
        .as_array()
        .expect("affected_articles must be an array");
    assert_eq!(articles.len(), 1, "exactly one affected article");

    let impact = &articles[0];
    assert_eq!(
        impact["id"].as_str(),
        Some(article_id.to_string().as_str()),
        "article id mismatch"
    );
    assert_eq!(
        impact["survivability"].as_str(),
        Some("orphaned"),
        "survivability must be 'orphaned': {impact}"
    );
    assert_eq!(
        impact["remaining_source_count"].as_i64(),
        Some(0),
        "no remaining sources"
    );
    let delta = impact["confidence_delta"]
        .as_f64()
        .expect("confidence_delta must be numeric");
    assert!(
        delta <= 0.0,
        "confidence_delta must be non-positive: {delta}"
    );

    // Verify the article is unchanged in the DB (ROLLBACK was honoured)
    let article_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM covalence.nodes WHERE id = $1 AND status = 'active')",
    )
    .bind(article_id)
    .fetch_one(&fix.pool)
    .await
    .expect("check article");
    assert!(
        article_exists,
        "article must still exist (ROLLBACK was used)"
    );

    fix.cleanup().await;
}

// ── test 2: survives case ─────────────────────────────────────────────────────

/// Two sources compiled into one article.  Retracting one source should leave
/// the other (with reliability > 0.3) in place — the article "survives" and
/// `safe_to_remove = true`.
#[tokio::test]
#[serial]
async fn test_whatif_retract_survives_case() {
    let mut fix = TestFixture::new().await;

    // Insert two sources
    let source_a = fix
        .insert_source("survives-src-a", "Primary source content")
        .await;
    let source_b = fix
        .insert_source("survives-src-b", "Secondary source content")
        .await;

    // Give source_b high reliability so the article survives when source_a is retracted
    sqlx::query("UPDATE covalence.nodes SET reliability = 0.8 WHERE id = $1")
        .bind(source_b)
        .execute(&fix.pool)
        .await
        .expect("set reliability for source_b");

    // Insert article
    let article_id = fix
        .insert_article("survives-article", "Article backed by two sources")
        .await;

    // Link both sources → article
    fix.insert_originates_edge(source_a, article_id).await;
    fix.insert_originates_edge(source_b, article_id).await;

    let app = make_app(fix.pool.clone()).await;

    // Retract source_a — source_b (reliability=0.8 > 0.3) remains
    let (status, body) = post_whatif_retract(app, source_a).await;

    assert_eq!(status, StatusCode::OK, "expected 200: {body}");

    let data = &body["data"];
    assert_eq!(
        data["safe_to_remove"].as_bool(),
        Some(true),
        "retraction should be safe when another reliable source remains: {body}"
    );
    assert_eq!(
        data["orphaned_count"].as_i64(),
        Some(0),
        "no orphaned articles expected: {body}"
    );
    assert_eq!(
        data["degraded_count"].as_i64(),
        Some(0),
        "no degraded articles expected: {body}"
    );

    let articles = data["affected_articles"]
        .as_array()
        .expect("affected_articles must be an array");
    assert_eq!(articles.len(), 1, "exactly one affected article");

    let impact = &articles[0];
    assert_eq!(
        impact["survivability"].as_str(),
        Some("survives"),
        "article must survive when a reliable source remains: {impact}"
    );
    assert_eq!(
        impact["remaining_source_count"].as_i64(),
        Some(1),
        "one source remains after retraction"
    );

    // Verify nothing was actually deleted (ROLLBACK honoured)
    let source_a_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM covalence.nodes WHERE id = $1)")
            .bind(source_a)
            .fetch_one(&fix.pool)
            .await
            .expect("check source_a");
    assert!(
        source_a_exists,
        "source_a must still exist (ROLLBACK was used)"
    );

    fix.cleanup().await;
}
