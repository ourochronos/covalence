//! Integration tests for ACT-R spreading activation (covalence#86).
//!
//! ## Architecture of the tests
//!
//! The graph dimension already traverses first-degree neighbours of nodes in
//! the lexical/vector candidate set.  So to create a node (B) that is found
//! ONLY by spreading activation and NOT by the normal 4-D search, we use a
//! 2-hop chain and pin `max_hops=1`:
//!
//! ```text
//!   Source S  ──CONFIRMS──►  Article A  ──CONFIRMS──►  Article B
//!     (matches query)        (found by graph, hop 1)   (hop 2 — NOT found
//!                                                        unless spreading
//!                                                        activation is on)
//! ```
//!
//! With `max_hops=1` (the default for `Balanced` strategy):
//! - Vector/lexical finds S.
//! - Graph dimension (1 hop from S) finds A.
//! - B is at hop-2 from S → NOT found by graph.
//! - Spreading activation runs over the top-5 final results, which include A,
//!   and boosts A's first-degree neighbour B.
//!
//! ## Tests
//!
//! * [`spreading_activation_surfaces_connected_nodes`] — with
//!   `spreading_activation=true`, article B appears in results because A
//!   (which IS found by graph) spreads activation to B.
//! * [`spreading_activation_disabled_by_default`] — without the flag, B does
//!   NOT appear (it is at hop-2, beyond the default max_hops=1).

use serial_test::serial;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Build a [`SearchRequest`] with spreading activation explicitly set.
/// `max_hops` is pinned to 1 so the graph dimension stops at hop 1 and
/// does not reach the hop-2 node B.
fn make_req_spreading(query: &str, spreading_activation: bool) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 20,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: Some(1), // pin to 1 hop so graph stops before reaching B
        after: None,
        before: None,
        min_score: None,
        spreading_activation: Some(spreading_activation),
        facet_function: None,
        facet_scope: None,
    }
}

/// Insert a CONFIRMS edge from `src` to `dst`.
async fn insert_confirms_edge(fix: &TestFixture, src: uuid::Uuid, dst: uuid::Uuid) {
    sqlx::query(
        "INSERT INTO covalence.edges \
             (source_node_id, target_node_id, edge_type) \
         VALUES ($1, $2, 'CONFIRMS')",
    )
    .bind(src)
    .bind(dst)
    .execute(&fix.pool)
    .await
    .expect("insert CONFIRMS edge failed");
}

// ─── test 1 ───────────────────────────────────────────────────────────────────

/// With `spreading_activation=true`, article B (2 hops from the query anchor)
/// must appear in results because spreading propagates from A (1 hop) to B.
///
/// Graph topology:
///   Source S ──CONFIRMS──► Article A ──CONFIRMS──► Article B
///
/// - S matches the query (vector/lexical) → S is in the candidate set.
/// - A is found by the graph dimension (hop 1 from S).
/// - B is at hop-2 from S → NOT found by graph with max_hops=1.
/// - Spreading from A (which is in top-5 results) boosts B.
///
/// Assertion: B appears in results when spreading_activation=true.
#[tokio::test]
#[serial]
async fn spreading_activation_surfaces_connected_nodes() {
    let mut fix = TestFixture::new().await;

    // Unique phrase that ONLY source S contains (ensures S is found by query).
    let topic_s = "zorblex quuxinator frobinate spreading activation test source alpha seven";

    // Article A and article B have deliberately unrelated content.
    let content_a = "intermediate node different content snollygoster bumfuzzle lollygag \
                     spreading test article alpha intermediate hop one";
    let content_b = "completely unrelated content widdershins absquatulate flibbertigibbet \
                     spreading test article beta target hop two";

    // Insert the three nodes.
    let source_s = fix
        .insert_source("Source S – Spreading Anchor", topic_s)
        .await;
    let article_a = fix
        .insert_article("Article A – Hop 1 Intermediate", content_a)
        .await;
    let article_b = fix
        .insert_article("Article B – Hop 2 Target", content_b)
        .await;

    // Embed all nodes so vector search can contribute scores.
    fix.insert_embedding(source_s).await;
    fix.insert_embedding(article_a).await;
    fix.insert_embedding(article_b).await;

    // Chain: S → A → B (2 CONFIRMS edges).
    insert_confirms_edge(&fix, source_s, article_a).await;
    insert_confirms_edge(&fix, article_a, article_b).await;

    let svc = SearchService::new(fix.pool.clone());

    // ── Search WITH spreading activation ─────────────────────────────────────
    let (results, _meta) = svc
        .search(make_req_spreading(topic_s, true))
        .await
        .expect("search with spreading_activation=true should succeed");

    // Source S must appear via normal search (it matches the query directly).
    assert!(
        results.iter().any(|r| r.node_id == source_s),
        "source S must appear in results (it matches the query via vector/lexical)"
    );

    // Article A must appear via the graph dimension (hop-1 from S).
    assert!(
        results.iter().any(|r| r.node_id == article_a),
        "article A must appear in results (found by graph dimension, hop-1 from S)"
    );

    // Article B must appear via spreading activation (it is A's first-degree
    // CONFIRMS neighbour, but hop-2 from S — beyond max_hops=1).
    let res_b = results.iter().find(|r| r.node_id == article_b).expect(
        "article B must appear when spreading_activation=true; \
             it is a first-degree CONFIRMS neighbour of article A, \
             which is in the top-5 results",
    );

    // Spreading score must be positive and ≤ the 0.15 cap.
    assert!(
        res_b.score > 0.0,
        "spreading-derived score for B should be positive (got {})",
        res_b.score
    );
    assert!(
        res_b.score <= 0.15,
        "spreading-derived score must be ≤ 0.15 cap (got {})",
        res_b.score
    );

    // B's graph_score field is populated with the spreading score.
    assert!(
        res_b.graph_score.is_some(),
        "spreading-derived result should have graph_score populated"
    );

    // B is at graph_hops=1 from A (the spreading parent).
    assert_eq!(
        res_b.graph_hops,
        Some(1),
        "spreading-derived result should report graph_hops=1"
    );

    fix.cleanup().await;
}

// ─── test 2 ───────────────────────────────────────────────────────────────────

/// Without `spreading_activation` (explicitly `false`), article B — at hop-2
/// from the query anchor — must NOT appear in results.
///
/// Same graph topology as test 1, but spreading is disabled.  The graph
/// dimension is pinned to max_hops=1 so it never reaches B.
#[tokio::test]
#[serial]
async fn spreading_activation_disabled_by_default() {
    let mut fix = TestFixture::new().await;

    // Use the same unique topic for source S (must not overlap with B's content).
    let topic_s = "zorblex quuxinator frobinate spreading disabled test source gamma eight";

    let content_a = "intermediate node different content snollygoster bumfuzzle lollygag \
                     spreading disabled article alpha intermediate hop one";
    let content_b = "completely unrelated content widdershins absquatulate flibbertigibbet \
                     spreading disabled article beta target hop two";

    let source_s = fix
        .insert_source("Source S – No-Spread Anchor", topic_s)
        .await;
    let article_a = fix
        .insert_article("Article A – No-Spread Intermediate", content_a)
        .await;
    let article_b = fix
        .insert_article("Article B – No-Spread Target", content_b)
        .await;

    fix.insert_embedding(source_s).await;
    fix.insert_embedding(article_a).await;
    fix.insert_embedding(article_b).await;

    // Same chain: S → A → B.
    insert_confirms_edge(&fix, source_s, article_a).await;
    insert_confirms_edge(&fix, article_a, article_b).await;

    let svc = SearchService::new(fix.pool.clone());

    // ── Search WITHOUT spreading activation ───────────────────────────────────
    let (results_no_spread, _) = svc
        .search(make_req_spreading(topic_s, false))
        .await
        .expect("search with spreading_activation=false should succeed");

    // Source S appears (direct text match).
    assert!(
        results_no_spread.iter().any(|r| r.node_id == source_s),
        "source S must appear regardless of spreading_activation"
    );

    // Article A may or may not appear (it depends on graph traversal from S).
    // Article B must NOT appear — it is at hop-2 and spreading is disabled.
    let b_present = results_no_spread.iter().any(|r| r.node_id == article_b);
    assert!(
        !b_present,
        "article B must NOT appear when spreading_activation=false; \
         it is at hop-2 from S (beyond max_hops=1) and spreading is disabled"
    );

    fix.cleanup().await;
}
