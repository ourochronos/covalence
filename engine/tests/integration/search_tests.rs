//! Integration tests for search ranking (trust-aware: covalence#31;
//! hierarchical mode: tracking#92 Phase B item 1).
//!
//! ## Trust-aware tests (existing)
//!
//! * A source node with high reliability should outrank an identical source
//!   node with low reliability (all other factors equal).
//! * An article whose linked sources have high average reliability should
//!   outrank an otherwise-identical article backed by low-reliability sources.
//!
//! Both tests use identical content / embeddings so the three dimensional
//! scores (vector, lexical, graph) are equal, making trust the sole
//! differentiating factor in the fusion formula.
//!
//! ## Hierarchical mode tests (tracking#92)
//!
//! * `test_hierarchical_search_returns_articles_first` — articles precede
//!   sources in hierarchical results.
//! * `test_hierarchical_search_expands_sources` — sources linked to top
//!   articles appear with `expanded_from` set to the parent article id.
//! * `test_standard_mode_unchanged` — standard mode returns mixed node types
//!   as before (no change to default behaviour).

use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::search_service::{SearchMode, SearchRequest, SearchService};

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
        mode: None,
        recency_bias: None,
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
        mode: None,
        recency_bias: None,
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
        mode: None,
        recency_bias: None,
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

// ─── Hierarchical search tests (tracking#92 Phase B, item 1) ─────────────────

/// In hierarchical mode the result list must contain all directly-matched
/// articles **before** any hierarchically-expanded source nodes.
///
/// Setup:
///   • One article (with embedding) containing the query phrase.
///   • One source (with embedding) containing the same phrase.
///   • The source is linked to the article via an ORIGINATES edge.
///
/// Because the source is linked to the article it will be expanded into the
/// result set by the hierarchical logic — but only after the article itself.
#[tokio::test]
#[serial]
async fn test_hierarchical_search_returns_articles_first() {
    let mut fix = TestFixture::new().await;

    let content =
        "hierarchical ordering test unique phrase kappa iota theta eta zeta epsilon delta";

    // Insert one article and one source with identical content/embeddings.
    let article_id = fix.insert_article("Hierarchical Article", content).await;
    let source_id = fix.insert_source("Hierarchical Source", content).await;

    fix.insert_embedding(article_id).await;
    fix.insert_embedding(source_id).await;

    // Link source → article.
    fix.insert_originates_edge(source_id, article_id).await;

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
        weights: None,
        mode: Some(SearchMode::Hierarchical),
        recency_bias: None,
    };

    let (results, _meta) = svc
        .search(req)
        .await
        .expect("hierarchical search should succeed");

    // At least the article must be returned.
    assert!(
        !results.is_empty(),
        "hierarchical search should return at least one result"
    );

    // All article-type results must appear before any source-type results.
    let mut seen_source = false;
    for r in &results {
        if r.node_type == "source" {
            seen_source = true;
        }
        if r.node_type == "article" && seen_source {
            panic!(
                "article result (id={}) appeared after a source result — \
                 hierarchical mode must return articles first",
                r.node_id
            );
        }
    }

    // The article itself must be present.
    assert!(
        results.iter().any(|r| r.node_id == article_id),
        "article should appear in hierarchical results"
    );

    fix.cleanup().await;
}

/// Hierarchical mode must expand linked source nodes and mark them with
/// `expanded_from` pointing to the parent article.
///
/// Setup:
///   • One article with an embedding.
///   • Two sources (no direct embedding needed for expansion — they are
///     looked up by provenance, not by dimensional search), each linked to
///     the article via an ORIGINATES edge.
///
/// We give the sources embeddings too so they would appear in standard search;
/// the point is that in hierarchical mode they appear **because of provenance**
/// and carry `expanded_from = article_id`.
#[tokio::test]
#[serial]
async fn test_hierarchical_search_expands_sources() {
    let mut fix = TestFixture::new().await;

    let article_content =
        "hierarchical expansion test unique phrase alpha beta gamma sigma rho pi omicron";
    let source_content_a = "backing source A for hierarchical expansion test unique phrase";
    let source_content_b = "backing source B for hierarchical expansion test unique phrase";

    let article_id = fix
        .insert_article("Hierarchical Expansion Article", article_content)
        .await;
    let source_a = fix.insert_source("Source A", source_content_a).await;
    let source_b = fix.insert_source("Source B", source_content_b).await;

    fix.insert_embedding(article_id).await;
    fix.insert_embedding(source_a).await;
    fix.insert_embedding(source_b).await;

    // Link both sources to the article.
    fix.insert_originates_edge(source_a, article_id).await;
    fix.insert_originates_edge(source_b, article_id).await;

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: article_content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
        weights: None,
        mode: Some(SearchMode::Hierarchical),
        recency_bias: None,
    };

    let (results, _meta) = svc
        .search(req)
        .await
        .expect("hierarchical search should succeed");

    // The article must appear.
    assert!(
        results.iter().any(|r| r.node_id == article_id),
        "article_id should be in hierarchical results"
    );

    // Both sources should appear as expanded results.
    let src_a_result = results.iter().find(|r| r.node_id == source_a);
    let src_b_result = results.iter().find(|r| r.node_id == source_b);

    assert!(
        src_a_result.is_some(),
        "source_a should appear in expanded hierarchical results"
    );
    assert!(
        src_b_result.is_some(),
        "source_b should appear in expanded hierarchical results"
    );

    // Both expanded sources must have expanded_from = article_id.
    let a_expanded_from = src_a_result
        .unwrap()
        .expanded_from
        .expect("source_a should have expanded_from set");
    let b_expanded_from = src_b_result
        .unwrap()
        .expanded_from
        .expect("source_b should have expanded_from set");

    assert_eq!(
        a_expanded_from, article_id,
        "source_a.expanded_from should equal article_id"
    );
    assert_eq!(
        b_expanded_from, article_id,
        "source_b.expanded_from should equal article_id"
    );

    // The article result itself must NOT have expanded_from set.
    let art_result = results
        .iter()
        .find(|r| r.node_id == article_id)
        .expect("article should be in results");
    assert!(
        art_result.expanded_from.is_none(),
        "directly-matched article should have expanded_from = None"
    );

    fix.cleanup().await;
}

/// Standard mode (the default) must continue returning mixed node types in
/// score order — the introduction of `mode` must not change existing behaviour.
///
/// We insert one article and one source with the same content/embeddings and
/// confirm both appear in results regardless of which comes first.
#[tokio::test]
#[serial]
async fn test_standard_mode_unchanged() {
    let mut fix = TestFixture::new().await;

    let content =
        "standard mode unchanged test unique phrase nu xi omicron pi rho sigma tau upsilon";

    let article_id = fix.insert_article("Standard Mode Article", content).await;
    let source_id = fix.insert_source("Standard Mode Source", content).await;

    fix.insert_embedding(article_id).await;
    fix.insert_embedding(source_id).await;

    let svc = SearchService::new(fix.pool.clone());

    // Explicit Standard mode.
    let req_explicit = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
        weights: None,
        mode: Some(SearchMode::Standard),
        recency_bias: None,
    };

    // Default mode (mode = None).
    let req_default = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
        weights: None,
        mode: None,
        recency_bias: None,
    };

    let (results_explicit, _) = svc
        .search(req_explicit)
        .await
        .expect("explicit standard search should succeed");
    let (results_default, _) = svc
        .search(req_default)
        .await
        .expect("default mode search should succeed");

    // Both modes must return results containing both node types (no type filter
    // should have been applied).
    for (label, results) in [
        ("explicit", &results_explicit),
        ("default", &results_default),
    ] {
        assert!(
            results.iter().any(|r| r.node_id == article_id),
            "{label}: article should appear in standard results"
        );
        assert!(
            results.iter().any(|r| r.node_id == source_id),
            "{label}: source should appear in standard results"
        );
    }

    // Standard mode must NOT set expanded_from on any result.
    for r in results_explicit.iter().chain(results_default.iter()) {
        assert!(
            r.expanded_from.is_none(),
            "standard mode result {} should have expanded_from = None",
            r.node_id
        );
    }

    fix.cleanup().await;
}

// ─── Recency bias tests (tracking#92 Phase B, item 2) ────────────────────────

/// `recency_bias` absent (or `None`) must produce exactly the same scores as
/// an explicit `recency_bias: Some(0.0)` request, preserving existing ranking
/// behaviour.
///
/// We insert two sources with identical content and compare their scores under
/// both request forms.  The score vectors must be element-wise equal (within
/// floating-point tolerance).
#[tokio::test]
#[serial]
async fn test_recency_bias_default_unchanged() {
    let mut fix = TestFixture::new().await;

    let content = "recency bias default unchanged test unique phrase xi chi psi omega";

    let src_a = fix.insert_source("Recency Default Source A", content).await;
    let src_b = fix.insert_source("Recency Default Source B", content).await;

    fix.insert_embedding(src_a).await;
    fix.insert_embedding(src_b).await;

    let svc = SearchService::new(fix.pool.clone());

    // Request with no recency_bias (defaults to 0.0 internally).
    let req_none = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["source".to_string()]),
        limit: 10,
        weights: None,
        mode: None,
        recency_bias: None,
    };

    // Request with explicit recency_bias = 0.0.
    let req_zero = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["source".to_string()]),
        limit: 10,
        weights: None,
        mode: None,
        recency_bias: Some(0.0),
    };

    let (results_none, _) = svc
        .search(req_none)
        .await
        .expect("search without recency_bias should succeed");
    let (results_zero, _) = svc
        .search(req_zero)
        .await
        .expect("search with recency_bias=0.0 should succeed");

    // Both result sets must contain both sources.
    assert!(
        results_none.iter().any(|r| r.node_id == src_a),
        "src_a should appear in results_none"
    );
    assert!(
        results_none.iter().any(|r| r.node_id == src_b),
        "src_b should appear in results_none"
    );

    // Scores for each node must be identical between the two requests
    // (within floating-point rounding tolerance).
    for id in [src_a, src_b] {
        let score_none = results_none
            .iter()
            .find(|r| r.node_id == id)
            .map(|r| r.score)
            .unwrap_or_else(|| panic!("node {id} missing from results_none"));
        let score_zero = results_zero
            .iter()
            .find(|r| r.node_id == id)
            .map(|r| r.score)
            .unwrap_or_else(|| panic!("node {id} missing from results_zero"));
        assert!(
            (score_none - score_zero).abs() < 1e-12,
            "node {id}: score with recency_bias=None ({score_none}) should equal \
             score with recency_bias=Some(0.0) ({score_zero})"
        );
    }

    fix.cleanup().await;
}

/// With `recency_bias: Some(1.0)` a freshly-inserted source must rank strictly
/// above an otherwise-identical source that is 365 days old, even when the
/// older source has a marginally better content-match score.
///
/// Math check:
///   freshness_weight = 0.05 + 1.0 * 0.35 = 0.40
///   dim_weight       = 1.0 - 0.40 - 0.10 = 0.50
///   freshness_new  ≈ exp(0)        = 1.0
///   freshness_old  ≈ exp(-3.65)    ≈ 0.026
///
/// Score difference from freshness alone (assuming equal dim scores):
///   Δ = 0.40 * (1.0 - 0.026) = 0.390  — overwhelms any dim-score gap.
///
/// To simulate a slightly worse content match for the new source we give it
/// a distinct title while keeping the body identical so lexical recall still
/// picks both up.  The freshness advantage easily covers the small dim delta.
#[tokio::test]
#[serial]
async fn test_recency_bias_favors_recent() {
    let mut fix = TestFixture::new().await;

    let content = "recency bias ranking test unique phrase alpha gamma epsilon zeta eta";

    // Insert two sources with identical content.
    let new_src = fix.insert_source("Recency Bias New Source", content).await;
    let old_src = fix.insert_source("Recency Bias Old Source", content).await;

    fix.insert_embedding(new_src).await;
    fix.insert_embedding(old_src).await;

    // Age the old source by backdating modified_at by 365 days.
    sqlx::query(
        "UPDATE covalence.nodes \
         SET modified_at = now() - interval '365 days' \
         WHERE id = $1",
    )
    .bind(old_src)
    .execute(&fix.pool)
    .await
    .expect("backdating old_src modified_at should succeed");

    let svc = SearchService::new(fix.pool.clone());
    let req = SearchRequest {
        query: content.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: Some(vec!["source".to_string()]),
        limit: 10,
        weights: None,
        mode: None,
        recency_bias: Some(1.0),
    };

    let (results, _meta) = svc
        .search(req)
        .await
        .expect("recency-bias search should succeed");

    // Both sources must be present.
    let new_result = results
        .iter()
        .find(|r| r.node_id == new_src)
        .expect("new source should appear in results");
    let old_result = results
        .iter()
        .find(|r| r.node_id == old_src)
        .expect("old source should appear in results");

    // The newer source must score strictly higher.
    assert!(
        new_result.score > old_result.score,
        "newer source (score={:.6}) should outrank 365-day-old source \
         (score={:.6}) when recency_bias=1.0",
        new_result.score,
        old_result.score,
    );

    fix.cleanup().await;
}
