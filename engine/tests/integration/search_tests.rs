//! Integration tests for trust-aware search ranking (covalence#31).
//!
//! These tests verify that source reliability is correctly propagated into
//! the final search score:
//!
//! * A source node with high reliability should outrank an identical source
//!   node with low reliability (all other factors equal).
//! * An article whose linked sources have high average reliability should
//!   outrank an otherwise-identical article backed by low-reliability sources.
//!
//! Both tests use identical content / embeddings so the three dimensional
//! scores (vector, lexical, graph) are equal, making trust the sole
//! differentiating factor in the fusion formula.

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Insert a source node with an explicit reliability value.
async fn insert_source_with_reliability(
    fix: &mut TestFixture,
    title: &str,
    content: &str,
    reliability: f64,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, reliability, metadata) \
         VALUES ($1, 'source', 'active', $2, $3, $4, '{}'::jsonb)",
    )
    .bind(id)
    .bind(title)
    .bind(content)
    .bind(reliability)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_source_with_reliability({title}) failed: {e}"));
    fix.track(id)
}

/// Insert an article node with an explicit reliability value.
///
/// The article's own reliability is not used in trust computation (that comes
/// from linked sources), but we accept it here for symmetry and future use.
async fn insert_article_with_reliability(
    fix: &mut TestFixture,
    title: &str,
    content: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, metadata) \
         VALUES ($1, 'article', 'active', $2, $3, '{}'::jsonb)",
    )
    .bind(id)
    .bind(title)
    .bind(content)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_article({title}) failed: {e}"));
    fix.track(id)
}

/// Create a directed provenance edge from `source_id` to `article_id`.
async fn link_source_to_article(fix: &TestFixture, source_id: Uuid, article_id: Uuid) {
    sqlx::query(
        "INSERT INTO covalence.edges \
             (source_node_id, target_node_id, edge_type) \
         VALUES ($1, $2, 'ORIGINATES')",
    )
    .bind(source_id)
    .bind(article_id)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("link_source_to_article failed: {e}"));
}

// ─── tests ────────────────────────────────────────────────────────────────────

/// A source with reliability=0.9 must score strictly higher than an otherwise
/// identical source with reliability=0.2 when both have the same dimensional
/// scores (same content + same embedding).
///
/// The fusion formula is:
///   final = dim*0.85 + trust*0.05 + confidence*0.05 + freshness*0.05
///
/// With equal dim, confidence, and freshness:
///   Δscore = (0.9 - 0.2) * 0.05 = 0.035 — clearly distinguishable.
#[tokio::test]
#[serial]
async fn source_reliability_affects_ranking() {
    let mut fix = TestFixture::new().await;

    // Both sources have identical content so they receive equal lexical /
    // vector scores from every search dimension.
    let shared_content = "trust ranking integration test unique phrase zeta omega phi delta";
    let shared_title = "Trust Ranking Test Source";

    let high_src =
        insert_source_with_reliability(&mut fix, shared_title, shared_content, 0.9).await;

    let low_src = insert_source_with_reliability(&mut fix, shared_title, shared_content, 0.2).await;

    // Identical unit-normalised embeddings so vector scores are equal.
    fix.insert_embedding(high_src).await;
    fix.insert_embedding(low_src).await;

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: shared_content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["source".to_string()]),
        limit: 10,
        weights: None,
    };

    let (results, _meta) = svc.search(req).await.expect("search should succeed");

    // Both nodes must appear in results.
    let high_result = results
        .iter()
        .find(|r| r.node_id == high_src)
        .expect("high-reliability source should appear in results");
    let low_result = results
        .iter()
        .find(|r| r.node_id == low_src)
        .expect("low-reliability source should appear in results");

    // Trust scores should be populated and match the inserted reliability.
    let high_trust = high_result
        .trust_score
        .expect("trust_score should be Some for source nodes");
    let low_trust = low_result
        .trust_score
        .expect("trust_score should be Some for source nodes");

    assert!(
        (high_trust - 0.9).abs() < 1e-9,
        "high-reliability source trust_score should be ~0.9, got {high_trust}"
    );
    assert!(
        (low_trust - 0.2).abs() < 1e-9,
        "low-reliability source trust_score should be ~0.2, got {low_trust}"
    );

    // High-reliability source must rank above the low-reliability one.
    assert!(
        high_result.score > low_result.score,
        "high-reliability source (score={:.6}) should outrank \
         low-reliability source (score={:.6})",
        high_result.score,
        low_result.score,
    );

    fix.cleanup().await;
}

/// An article compiled from high-reliability sources must rank above an
/// otherwise-identical article compiled from low-reliability sources.
///
/// Trust for articles = AVG(reliability of linked sources).
/// With equal dimensional scores the fusion delta is:
///   Δscore = (avg_high - avg_low) * 0.05 ≥ (0.9 - 0.2) * 0.05 = 0.035
#[tokio::test]
#[serial]
async fn article_trust_derived_from_source_reliability() {
    let mut fix = TestFixture::new().await;

    let shared_content = "article trust ranking integration test unique phrase sigma tau upsilon";
    let article_title = "Trust Ranking Test Article";

    // ── Create two articles with identical content ────────────────────────────
    let high_article =
        insert_article_with_reliability(&mut fix, article_title, shared_content).await;

    let low_article =
        insert_article_with_reliability(&mut fix, article_title, shared_content).await;

    // ── Create sources with contrasting reliability ───────────────────────────
    let high_source = insert_source_with_reliability(
        &mut fix,
        "High Reliability Source",
        "backing content for high article",
        0.9,
    )
    .await;

    let low_source = insert_source_with_reliability(
        &mut fix,
        "Low Reliability Source",
        "backing content for low article",
        0.2,
    )
    .await;

    // ── Wire provenance: source → article ────────────────────────────────────
    link_source_to_article(&fix, high_source, high_article).await;
    link_source_to_article(&fix, low_source, low_article).await;

    // ── Embeddings: unit vectors (equal for both articles) ───────────────────
    fix.insert_embedding(high_article).await;
    fix.insert_embedding(low_article).await;

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: shared_content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["article".to_string()]),
        limit: 10,
        weights: None,
    };

    let (results, _meta) = svc.search(req).await.expect("search should succeed");

    // Both articles must be present in the result set.
    let high_result = results
        .iter()
        .find(|r| r.node_id == high_article)
        .expect("high-trust article should appear in results");
    let low_result = results
        .iter()
        .find(|r| r.node_id == low_article)
        .expect("low-trust article should appear in results");

    // Trust scores should reflect the average reliability of linked sources.
    let high_trust = high_result
        .trust_score
        .expect("trust_score should be Some for article nodes");
    let low_trust = low_result
        .trust_score
        .expect("trust_score should be Some for article nodes");

    assert!(
        (high_trust - 0.9).abs() < 1e-9,
        "article backed by reliability-0.9 source should have trust ~0.9, got {high_trust}"
    );
    assert!(
        (low_trust - 0.2).abs() < 1e-9,
        "article backed by reliability-0.2 source should have trust ~0.2, got {low_trust}"
    );

    // The high-trust article must outscore the low-trust one.
    assert!(
        high_result.score > low_result.score,
        "article with high-trust sources (score={:.6}) should outrank \
         article with low-trust sources (score={:.6})",
        high_result.score,
        low_result.score,
    );

    fix.cleanup().await;
}

/// An article with no linked sources should receive the default trust of 0.5.
#[tokio::test]
#[serial]
async fn article_without_sources_gets_default_trust() {
    let mut fix = TestFixture::new().await;

    let content = "orphan article trust default test unique phrase lambda mu nu";
    let art = insert_article_with_reliability(&mut fix, "Orphan Article", content).await;
    fix.insert_embedding(art).await;

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["article".to_string()]),
        limit: 5,
        weights: None,
    };

    let (results, _meta) = svc.search(req).await.expect("search should succeed");

    let result = results
        .iter()
        .find(|r| r.node_id == art)
        .expect("orphan article should appear in results");

    let trust = result
        .trust_score
        .expect("trust_score should be Some even for orphan articles");

    assert!(
        (trust - 0.5).abs() < 1e-9,
        "orphan article (no linked sources) should have default trust 0.5, got {trust}"
    );

    fix.cleanup().await;
}
