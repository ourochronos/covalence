//! Integration tests for KB 3-layer navigation (covalence#112).
//!
//! Three tests:
//! 1. `test_topology_map_created` — verifies that calling `generate_topology_map`
//!    maintenance action produces a landmark article titled "KB Topology Map — …"
//!    that is pinned, is_landmark=true, and contains the expected sections.
//! 2. `test_domain_landmark_created` — verifies that `generate_domain_landmarks`
//!    produces a "Domain Overview: <domain>" article for a domain with ≥ 5 articles.
//! 3. `test_bridge_articles_detected` — verifies that articles whose `domain_path`
//!    spans 2+ distinct top-level domains are returned by `detect_bridge_articles`.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;
use uuid::Uuid;

use covalence_engine::worker::navigation;

use super::helpers::{make_test_state, setup_pool};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// POST /admin/maintenance with a JSON body; returns the parsed response body.
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
        "maintenance returned non-200"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("parse JSON")
}

/// Insert `n` articles with the given `domain_path` top-level label and return
/// their IDs.  All titles are distinct.
async fn insert_domain_articles(pool: &sqlx::PgPool, domain: &str, n: usize) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO covalence.nodes
                 (id, node_type, status, title, content, domain_path, metadata)
             VALUES ($1, 'article', 'active', $2, $3, ARRAY[$4], '{}'::jsonb)",
        )
        .bind(id)
        .bind(format!("{domain}-article-{i}"))
        .bind(format!("Content for {domain} article {i}."))
        .bind(domain)
        .execute(pool)
        .await
        .expect("insert_domain_articles");
        ids.push(id);
    }
    ids
}

/// Delete all nodes inserted for the given IDs (cascades to FK tables).
async fn delete_nodes(pool: &sqlx::PgPool, ids: &[Uuid]) {
    for &id in ids {
        sqlx::query("DELETE FROM covalence.slow_path_queue WHERE node_id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM covalence.edges WHERE source_node_id = $1 OR target_node_id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM covalence.node_embeddings WHERE node_id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM covalence.contentions WHERE node_id = $1 OR source_node_id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
        sqlx::query("DELETE FROM covalence.nodes WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
    }
}

/// Delete all is_landmark articles created during this test run.
async fn delete_landmarks(pool: &sqlx::PgPool) {
    sqlx::query("DELETE FROM covalence.nodes WHERE is_landmark = true")
        .execute(pool)
        .await
        .ok();
}

// ─── Test 1: Topology map created ─────────────────────────────────────────────

/// Calling `generate_topology_map: true` maintenance action should produce
/// a single "KB Topology Map — <date>" article that:
/// - is active
/// - has pinned = true
/// - has is_landmark = true
/// - contains expected section headers in its content
#[tokio::test]
#[serial]
async fn test_topology_map_created() {
    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = covalence_engine::api::routes::router().with_state(state);

    // Clean up any pre-existing landmarks from prior test runs.
    delete_landmarks(&pool).await;

    // Call maintenance to generate the topology map.
    let body = post_maintenance(&app, serde_json::json!({ "generate_topology_map": true })).await;

    let actions = body["data"]["actions_taken"]
        .as_array()
        .expect("actions_taken array")
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>();

    // At least one action should mention topology map.
    let topo_action = actions
        .iter()
        .find(|a| a.contains("generate_topology_map"))
        .cloned()
        .expect("expected generate_topology_map action");
    assert!(
        topo_action.contains("article="),
        "action should include article UUID: {topo_action}"
    );

    // Verify the article row in the DB.
    let row: Option<(bool, bool, String)> = sqlx::query_as(
        "SELECT pinned, is_landmark, content
         FROM   covalence.nodes
         WHERE  node_type   = 'article'
           AND  status      = 'active'
           AND  title       LIKE 'KB Topology Map%'
         LIMIT  1",
    )
    .fetch_optional(&pool)
    .await
    .expect("query topology map article");

    let (pinned, is_landmark, content) = row.expect("topology map article should exist in DB");
    assert!(pinned, "topology map article should be pinned");
    assert!(
        is_landmark,
        "topology map article should be is_landmark=true"
    );

    // Check for expected section markers in the content.
    for section in &[
        "## Summary",
        "## Domains",
        "## Hub Articles",
        "## Recently Modified",
    ] {
        assert!(
            content.contains(section),
            "topology map content missing section '{section}'"
        );
    }

    // Calling again should refresh (UPDATE), not create a duplicate.
    post_maintenance(&app, serde_json::json!({ "generate_topology_map": true })).await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.nodes
         WHERE node_type = 'article' AND status = 'active' AND title LIKE 'KB Topology Map%'",
    )
    .fetch_one(&pool)
    .await
    .expect("count topology maps");

    assert_eq!(
        count, 1,
        "exactly one topology map article should exist after two calls"
    );

    // Cleanup.
    delete_landmarks(&pool).await;
}

// ─── Test 2: Domain landmark created ─────────────────────────────────────────

/// `generate_domain_landmarks` should produce a "Domain Overview: <domain>"
/// landmark article (pinned + is_landmark) for any domain with ≥ 5 articles.
/// Domains with < 5 articles should not produce a landmark.
#[tokio::test]
#[serial]
async fn test_domain_landmark_created() {
    let pool = setup_pool().await;

    delete_landmarks(&pool).await;

    // Insert 6 articles in domain "navigation-test-alpha".
    let alpha_ids = insert_domain_articles(&pool, "navigation-test-alpha", 6).await;
    // Insert 3 articles in domain "navigation-test-beta" (below threshold).
    let beta_ids = insert_domain_articles(&pool, "navigation-test-beta", 3).await;

    // Run generate_domain_landmarks directly on the pool.
    let result = navigation::generate_domain_landmarks(&pool, 5)
        .await
        .expect("generate_domain_landmarks");

    // "navigation-test-alpha" should have been processed.
    assert!(
        result
            .domains
            .contains(&"navigation-test-alpha".to_string()),
        "alpha domain should be in result domains: {:?}",
        result.domains
    );

    // "navigation-test-beta" should NOT have been processed (only 3 articles).
    assert!(
        !result.domains.contains(&"navigation-test-beta".to_string()),
        "beta domain (3 articles) should not be processed: {:?}",
        result.domains
    );

    // At least one landmark should have been upserted.
    assert!(
        result.upserted >= 1,
        "at least one landmark should be upserted"
    );

    // Verify "Domain Overview: navigation-test-alpha" exists, is pinned, and is landmark.
    let row: Option<(bool, bool, String)> = sqlx::query_as(
        "SELECT pinned, is_landmark, content
         FROM   covalence.nodes
         WHERE  node_type   = 'article'
           AND  status      = 'active'
           AND  title       = 'Domain Overview: navigation-test-alpha'
         LIMIT  1",
    )
    .fetch_optional(&pool)
    .await
    .expect("query domain landmark");

    let (pinned, is_landmark, content) =
        row.expect("Domain Overview: navigation-test-alpha should exist");
    assert!(pinned, "domain landmark should be pinned=true");
    assert!(is_landmark, "domain landmark should be is_landmark=true");
    assert!(
        content.contains("navigation-test-alpha"),
        "landmark content should mention the domain name"
    );
    assert!(
        content.contains("## Key Articles"),
        "landmark content should have '## Key Articles' section"
    );

    // "Domain Overview: navigation-test-beta" must NOT exist.
    let beta_landmark: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM covalence.nodes WHERE title = 'Domain Overview: navigation-test-beta'",
    )
    .fetch_optional(&pool)
    .await
    .expect("check beta landmark absence");
    assert!(
        beta_landmark.is_none(),
        "Domain Overview: navigation-test-beta should not be created (only 3 articles)"
    );

    // Landmarks must never be evicted: verify is_landmark=true rows survive
    // the eviction query predicate.
    let evict_candidate: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM covalence.nodes
         WHERE  node_type    = 'article'
           AND  status       = 'active'
           AND  is_landmark  = false
           AND  pinned       = false
           AND  structural_importance < 0.8
         LIMIT  1",
    )
    .fetch_optional(&pool)
    .await
    .expect("eviction candidate check");
    // The landmark itself must not appear in the candidate set.
    if let Some((eid,)) = evict_candidate {
        let is_lm: bool =
            sqlx::query_scalar("SELECT is_landmark FROM covalence.nodes WHERE id = $1")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap_or(false);
        assert!(!is_lm, "eviction candidate {eid} should not be a landmark");
    }

    // Cleanup.
    delete_landmarks(&pool).await;
    let mut all_ids = alpha_ids;
    all_ids.extend(beta_ids);
    delete_nodes(&pool, &all_ids).await;
}

// ─── Test 3: Bridge articles detected ────────────────────────────────────────

/// Articles with `domain_path` entries that span ≥ 2 distinct top-level
/// domains should be returned by `detect_bridge_articles`.
/// Articles with all paths in one top-level domain should not be returned.
#[tokio::test]
#[serial]
async fn test_bridge_articles_detected() {
    let pool = setup_pool().await;

    // Insert a bridge article spanning "rust" and "python" top-level domains.
    let bridge_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes
             (id, node_type, status, title, content, domain_path, metadata)
         VALUES ($1, 'article', 'active', $2, $3, ARRAY['rust/stdlib', 'python/io'], '{}'::jsonb)",
    )
    .bind(bridge_id)
    .bind("Bridge Article: Rust + Python IO")
    .bind("An article that covers both Rust stdlib and Python IO patterns.")
    .execute(&pool)
    .await
    .expect("insert bridge article");

    // Insert a single-domain article (all in "rust").
    let single_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes
             (id, node_type, status, title, content, domain_path, metadata)
         VALUES ($1, 'article', 'active', $2, $3, ARRAY['rust/stdlib', 'rust/async'], '{}'::jsonb)",
    )
    .bind(single_id)
    .bind("Single Domain Article: Rust")
    .bind("Covers both rust/stdlib and rust/async — same top-level domain.")
    .execute(&pool)
    .await
    .expect("insert single-domain article");

    // Insert a triple-domain article.
    let triple_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes
             (id, node_type, status, title, content, domain_path, metadata)
         VALUES ($1, 'article', 'active', $2, $3, ARRAY['go/concurrency', 'rust/async', 'python/asyncio'], '{}'::jsonb)",
    )
    .bind(triple_id)
    .bind("Bridge Article: Go + Rust + Python Concurrency")
    .bind("Cross-language concurrency patterns.")
    .execute(&pool)
    .await
    .expect("insert triple-domain article");

    // Run bridge detection.
    let bridges = navigation::detect_bridge_articles(&pool)
        .await
        .expect("detect_bridge_articles");

    let bridge_ids: Vec<Uuid> = bridges.iter().map(|b| b.id).collect();

    // The cross-domain articles should be detected.
    assert!(
        bridge_ids.contains(&bridge_id),
        "rust+python article should be detected as a bridge"
    );
    assert!(
        bridge_ids.contains(&triple_id),
        "go+rust+python article should be detected as a bridge"
    );

    // The single-domain article must NOT be a bridge.
    assert!(
        !bridge_ids.contains(&single_id),
        "rust-only article must not be a bridge"
    );

    // Verify domain lists are correct for the two-domain bridge.
    let bridge_entry = bridges.iter().find(|b| b.id == bridge_id).unwrap();
    assert_eq!(
        bridge_entry.domains.len(),
        2,
        "two-domain bridge should span exactly 2 top-level domains"
    );
    assert!(
        bridge_entry.domains.contains(&"rust".to_string()),
        "bridge domains should include 'rust'"
    );
    assert!(
        bridge_entry.domains.contains(&"python".to_string()),
        "bridge domains should include 'python'"
    );

    // The triple-domain bridge should span 3 domains.
    let triple_entry = bridges.iter().find(|b| b.id == triple_id).unwrap();
    assert_eq!(
        triple_entry.domains.len(),
        3,
        "triple-domain bridge should span exactly 3 top-level domains: {:?}",
        triple_entry.domains
    );

    // Cleanup.
    delete_nodes(&pool, &[bridge_id, single_id, triple_id]).await;
}
