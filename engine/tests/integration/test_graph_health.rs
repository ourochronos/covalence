//! Integration tests for graph health features introduced in covalence#74:
//!
//! 1. **Bridge-node eviction protection** — a node with low `usage_score`
//!    that structurally bridges two knowledge clusters must survive eviction
//!    because its betweenness centrality is high.
//!
//! 2. **PPR query expansion** — a node that is not found by the initial 4-D
//!    search but is a direct graph neighbour of a top-k seed is discovered
//!    and appended to results via Personalized PageRank expansion.

use axum::body::Body;
use http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt as _;
use uuid::Uuid;

use covalence_engine::api::routes;
use covalence_engine::graph::CovalenceGraph;
use covalence_engine::services::admin_service::{AdminService, MaintenanceRequest};

use super::helpers::{make_test_state, setup_pool};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// POST /sources with the given content and return the node UUID string.
async fn create_source(app: &axum::Router, title: &str, content: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/sources")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "content": content,
                "source_type": "document",
                "title": title
            })
            .to_string(),
        ))
        .expect("source request");

    let resp = app.clone().oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("bytes");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    json["data"]["id"].as_str().expect("id missing").to_string()
}

/// POST /edges with a CONFIRMS relationship and return the edge id.
async fn create_edge(app: &axum::Router, from: &str, to: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/edges")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "from_node_id": from,
                "to_node_id":   to,
                "label":        "CONFIRMS"
            })
            .to_string(),
        ))
        .expect("edge request");

    let resp = app.clone().oneshot(req).await.expect("oneshot");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "edge creation failed: {from} -> {to}"
    );

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("bytes");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    json["data"]["id"]
        .as_str()
        .expect("edge id missing")
        .to_string()
}

/// Helper: fetch status column for a node by ID.
async fn node_status(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM covalence.nodes WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|_| panic!("node {id} not found"))
}

// ─── Test 1: Bridge node survives eviction ────────────────────────────────────

/// A node with the *lowest* `usage_score` in the corpus must survive eviction
/// if it structurally bridges two disjoint knowledge clusters.
///
/// Graph topology:
/// ```
///   a1 → a2 → [BRIDGE] → b1 → b2
///                             extra (disconnected)
/// ```
/// The BRIDGE node has betweenness centrality = 4/20 = 0.20 (highest in the
/// graph), so `structural_importance()` marks it for protection.
/// All other nodes have usage_score = 0.5; BRIDGE has usage_score = 0.001.
/// Without protection BRIDGE would be evicted first.  With protection it
/// must survive.
#[tokio::test]
#[serial]
async fn bridge_node_survives_eviction() {
    // Lower the eviction threshold so we don't need to create 1 000 articles.
    // SAFETY: tests run with --test-threads=1 so env mutation is serialised.
    unsafe { std::env::set_var("COVALENCE_MAX_ARTICLES", "5") };

    let pool = setup_pool().await;

    // Build the in-memory graph manually so structural_importance() can see
    // the bridge topology.  (In production this is rebuilt from DB on each
    // edge write; here we populate it directly for isolation.)
    let a1_id = Uuid::new_v4();
    let a2_id = Uuid::new_v4();
    let bridge_id = Uuid::new_v4();
    let b1_id = Uuid::new_v4();
    let b2_id = Uuid::new_v4();
    let extra_id = Uuid::new_v4();

    // Persist the 6 article nodes in the test DB.
    for (id, title) in [
        (a1_id, "Cluster A article one"),
        (a2_id, "Cluster A article two"),
        (
            bridge_id,
            "Bridge article connecting cluster A and cluster B",
        ),
        (b1_id, "Cluster B article one"),
        (b2_id, "Cluster B article two"),
        (extra_id, "Disconnected extra article"),
    ] {
        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, metadata) \
             VALUES ($1, 'article', 'active', $2, $2, '{}'::jsonb)",
        )
        .bind(id)
        .bind(title)
        .execute(&pool)
        .await
        .expect("insert article");
    }

    // Set usage scores: BRIDGE gets the lowest score so it would normally be
    // evicted first under a pure usage_score ordering.
    sqlx::query("UPDATE covalence.nodes SET usage_score = 0.001 WHERE id = $1")
        .bind(bridge_id)
        .execute(&pool)
        .await
        .expect("set bridge usage_score");

    sqlx::query("UPDATE covalence.nodes SET usage_score = 0.5 WHERE id = ANY($1::uuid[])")
        .bind(&[a1_id, a2_id, b1_id, b2_id, extra_id] as &[Uuid])
        .execute(&pool)
        .await
        .expect("set other usage_scores");

    // Build the shared in-memory graph with the bridge topology:
    //   a1 → a2 → bridge → b1 → b2   (extra is isolated)
    let mut cov_graph = CovalenceGraph::new();
    cov_graph.add_edge(a1_id, a2_id, "CONFIRMS".to_string());
    cov_graph.add_edge(a2_id, bridge_id, "CONFIRMS".to_string());
    cov_graph.add_edge(bridge_id, b1_id, "CONFIRMS".to_string());
    cov_graph.add_edge(b1_id, b2_id, "CONFIRMS".to_string());
    cov_graph.add_node(extra_id);

    let shared_graph = std::sync::Arc::new(tokio::sync::RwLock::new(cov_graph));

    // Create AdminService with the populated shared graph.
    let admin = AdminService::new(pool.clone()).with_graph(shared_graph.clone());

    // Trigger eviction: active_count (6) > COVALENCE_MAX_ARTICLES (5).
    let result = admin
        .maintenance(MaintenanceRequest {
            recompute_scores: None,
            process_queue: None,
            evict_if_over_capacity: Some(true),
            evict_count: Some(2),
            recompute_graph_embeddings: None,
            graph_embeddings_method: None,
            scan_due_consolidations: None,
            refresh_inference: None,
            compute_gaps: None,
            compute_structural_importance: None,
        })
        .await
        .expect("maintenance call failed");

    // Eviction must have fired.
    assert!(
        result.actions_taken.iter().any(|a| a.contains("evicted")),
        "expected eviction to run; actions: {:?}",
        result.actions_taken
    );

    // ── KEY ASSERTION: bridge node must still be active ───────────────────
    let bridge_status = node_status(&pool, bridge_id).await;
    assert_eq!(
        bridge_status, "active",
        "bridge node (id={bridge_id}) should survive eviction due to high \
         betweenness centrality, but got status '{bridge_status}'"
    );

    // At least one other article must have been evicted (evict_count = 2).
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.nodes WHERE status = 'active' AND node_type = 'article'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert!(
        active_count < 6,
        "expected fewer than 6 active articles after eviction, got {active_count}"
    );

    // Restore env.
    unsafe { std::env::remove_var("COVALENCE_MAX_ARTICLES") };
}

// ─── Test 2: PPR expansion finds graph neighbour ─────────────────────────────

/// A query that directly matches source A must also surface source B when B
/// is a graph neighbour of A — even though B's content has no textual overlap
/// with the query.  PPR expansion from A propagates relevance mass to B via
/// the in-memory graph edge A → B.
#[tokio::test]
#[serial]
async fn ppr_expansion_finds_graph_neighbor() {
    // Enable PPR expansion for this test.
    unsafe { std::env::set_var("COVALENCE_PPR_EXPANSION", "true") };

    let pool = setup_pool().await;
    let state = make_test_state(pool.clone()).await;
    let app = routes::router().with_state(state);

    // Source A: content directly matches the search query.
    let a_id_str = create_source(
        &app,
        "Quantum Computing Overview",
        "quantum computing algorithms optimize combinatorial search problems efficiently",
    )
    .await;

    // Source B: completely unrelated content — would never match the query on
    // its own via lexical or vector search.
    let b_id_str = create_source(
        &app,
        "Xylophone Notation Guide",
        "xylophone percussion notation musical staff theory fundamentals",
    )
    .await;

    // Create edge A → B.  This triggers `reload_shared_graph` in the route
    // handler, so the in-memory graph will contain both nodes and the edge.
    create_edge(&app, &a_id_str, &b_id_str).await;

    // Small pause to let any async graph reload settle (typically synchronous
    // in the test router, but defensive).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Run a search that should directly match A.
    let search_req = Request::builder()
        .method("POST")
        .uri("/search")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "query": "quantum computing algorithms",
                "limit": 10
            })
            .to_string(),
        ))
        .expect("search request");

    let resp = app.clone().oneshot(search_req).await.expect("search");
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("bytes");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

    let result_ids: Vec<String> = json["data"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|r| r["node_id"].as_str().map(|s| s.to_string()))
        .collect();

    // A must appear (direct lexical match).
    assert!(
        result_ids.contains(&a_id_str),
        "source A should appear in results via direct match; results: {result_ids:?}"
    );

    // B must appear via PPR expansion from A.
    assert!(
        result_ids.contains(&b_id_str),
        "source B should appear in results via PPR expansion from A; results: {result_ids:?}"
    );

    // Restore env.
    unsafe { std::env::remove_var("COVALENCE_PPR_EXPANSION") };
}
