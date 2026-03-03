//! Namespace isolation tests (covalence#47).
//!
//! Verifies that sources, articles, and search results are strictly scoped
//! to the namespace they were written into.  Bleeding across namespaces is a
//! correctness failure.

use serial_test::serial;
use uuid::Uuid;

use super::helpers::TestFixture;
use covalence_engine::services::{
    article_service::{ArticleService, CreateArticleRequest},
    search_service::{SearchRequest, SearchService},
    source_service::{IngestRequest, SourceService},
};

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Insert a source into a specific namespace and track the node ID.
async fn insert_ns_source(fix: &mut TestFixture, ns: &str, title: &str, content: &str) -> Uuid {
    let req = IngestRequest {
        title: Some(title.into()),
        content: content.into(),
        source_type: None,
        reliability: None,
        metadata: None,
        session_id: None,
        capture_method: None,
    };
    let svc = SourceService::new(fix.pool.clone()).with_namespace(ns.to_string());
    let resp = svc.ingest(req).await.expect("ingest should succeed");
    fix.track(resp.id);
    resp.id
}

/// Insert an article into a specific namespace and track the node ID.
async fn insert_ns_article(fix: &mut TestFixture, ns: &str, title: &str, content: &str) -> Uuid {
    let req = CreateArticleRequest {
        title: Some(title.into()),
        content: content.into(),
        source_ids: None,
        epistemic_type: None,
        domain_path: None,
        metadata: None,
    };
    let svc = ArticleService::new(fix.pool.clone()).with_namespace(ns.to_string());
    let resp = svc.create(req).await.expect("create should succeed");
    fix.track(resp.id);
    resp.id
}

// ─── tests ───────────────────────────────────────────────────────────────────

/// Ingest into namespace `"alpha"` and `"beta"`, then verify that
/// `SourceService::get` scoped to `"alpha"` returns the alpha source and
/// cannot see the beta source.
#[tokio::test]
#[serial]
async fn test_namespace_source_isolation() {
    let mut fix = TestFixture::new().await;

    let alpha_id = insert_ns_source(
        &mut fix,
        "alpha",
        "Alpha document",
        "This belongs to namespace alpha.",
    )
    .await;
    let beta_id = insert_ns_source(
        &mut fix,
        "beta",
        "Beta document",
        "This belongs to namespace beta.",
    )
    .await;

    // Alpha-scoped service can see its own source.
    let alpha_svc = SourceService::new(fix.pool.clone()).with_namespace("alpha".into());
    let found = alpha_svc.get(alpha_id).await;
    assert!(
        found.is_ok(),
        "alpha source should be visible in alpha namespace"
    );

    // Alpha-scoped service cannot see the beta source.
    let not_found = alpha_svc.get(beta_id).await;
    assert!(
        not_found.is_err(),
        "beta source should NOT be visible in alpha namespace"
    );

    // Beta-scoped service can see its own source.
    let beta_svc = SourceService::new(fix.pool.clone()).with_namespace("beta".into());
    let found_beta = beta_svc.get(beta_id).await;
    assert!(
        found_beta.is_ok(),
        "beta source should be visible in beta namespace"
    );

    fix.cleanup().await;
}

/// Ensure that `SourceService::list` only returns sources in the correct
/// namespace.
#[tokio::test]
#[serial]
async fn test_namespace_source_list_isolation() {
    let mut fix = TestFixture::new().await;

    let alpha_id = insert_ns_source(
        &mut fix,
        "alpha-list",
        "Alpha list doc",
        "Alpha namespace content for list test.",
    )
    .await;
    let _beta_id = insert_ns_source(
        &mut fix,
        "beta-list",
        "Beta list doc",
        "Beta namespace content for list test.",
    )
    .await;

    let alpha_svc = SourceService::new(fix.pool.clone()).with_namespace("alpha-list".into());
    let alpha_sources = alpha_svc
        .list(Default::default())
        .await
        .expect("list should succeed");

    let alpha_ids: Vec<Uuid> = alpha_sources.iter().map(|s| s.id).collect();
    assert!(
        alpha_ids.contains(&alpha_id),
        "alpha source should appear in alpha list"
    );

    // Verify no beta sources leaked into the alpha list.
    let beta_svc = SourceService::new(fix.pool.clone()).with_namespace("beta-list".into());
    let beta_sources = beta_svc
        .list(Default::default())
        .await
        .expect("list should succeed");
    for s in &beta_sources {
        assert!(
            !alpha_ids.contains(&s.id),
            "beta source {} should NOT appear in alpha namespace list",
            s.id
        );
    }

    fix.cleanup().await;
}

/// Verify that `ArticleService` respects namespace for create + get.
#[tokio::test]
#[serial]
async fn test_namespace_article_isolation() {
    let mut fix = TestFixture::new().await;

    let alpha_article = insert_ns_article(
        &mut fix,
        "alpha-art",
        "Alpha Article",
        "Content for the alpha article.",
    )
    .await;
    let beta_article = insert_ns_article(
        &mut fix,
        "beta-art",
        "Beta Article",
        "Content for the beta article.",
    )
    .await;

    let alpha_svc = ArticleService::new(fix.pool.clone()).with_namespace("alpha-art".into());
    assert!(
        alpha_svc.get(alpha_article).await.is_ok(),
        "alpha article should be visible in alpha namespace"
    );
    assert!(
        alpha_svc.get(beta_article).await.is_err(),
        "beta article should NOT be visible in alpha namespace"
    );

    let beta_svc = ArticleService::new(fix.pool.clone()).with_namespace("beta-art".into());
    assert!(
        beta_svc.get(beta_article).await.is_ok(),
        "beta article should be visible in beta namespace"
    );
    assert!(
        beta_svc.get(alpha_article).await.is_err(),
        "alpha article should NOT be visible in beta namespace"
    );

    fix.cleanup().await;
}

/// Verify that `SearchService` (lexical path) does not bleed across namespaces.
///
/// We insert a uniquely-phrased source into `"alpha-search"`, then run the
/// same query scoped to `"beta-search"`.  The beta search should return no
/// results for that unique phrase.
#[tokio::test]
#[serial]
async fn test_namespace_search_isolation() {
    let mut fix = TestFixture::new().await;

    // Use a very distinctive phrase that won't match anything else.
    let unique_phrase = format!("xq47namespace{}", Uuid::new_v4().simple());

    let _alpha_src = insert_ns_source(
        &mut fix,
        "alpha-search",
        "Alpha Search Source",
        &format!("Unique phrase for search isolation: {unique_phrase}"),
    )
    .await;

    // Search in alpha — should find the document (lexical).
    let alpha_svc = SearchService::new(fix.pool.clone()).with_namespace("alpha-search".into());
    let (alpha_results, _) = alpha_svc
        .search(SearchRequest {
            query: unique_phrase.clone(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 10,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        })
        .await
        .expect("alpha search should succeed");

    // Alpha search should find the document (assuming lexical is available).
    // We don't assert exact count because the vector dimension may be absent,
    // but if results exist they must all be in the alpha namespace.
    for r in &alpha_results {
        // Each result's node must belong to the alpha-search namespace.
        let ns: String = sqlx::query_scalar("SELECT namespace FROM covalence.nodes WHERE id = $1")
            .bind(r.node_id)
            .fetch_one(&fix.pool)
            .await
            .expect("node should exist");

        assert_eq!(
            ns, "alpha-search",
            "result node {} has namespace '{}', expected 'alpha-search'",
            r.node_id, ns
        );
    }

    // Search in beta — must return zero results for the unique phrase.
    let beta_svc = SearchService::new(fix.pool.clone()).with_namespace("beta-search".into());
    let (beta_results, _) = beta_svc
        .search(SearchRequest {
            query: unique_phrase.clone(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None,
            limit: 10,
            weights: None,
            mode: None,
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
        })
        .await
        .expect("beta search should succeed");

    assert_eq!(
        beta_results.len(),
        0,
        "search in 'beta-search' namespace must not find alpha documents"
    );

    fix.cleanup().await;
}

/// Verify that the default namespace (`"default"`) does not show sources
/// inserted into custom namespaces, and vice versa.
#[tokio::test]
#[serial]
async fn test_default_namespace_does_not_bleed() {
    let mut fix = TestFixture::new().await;

    // Insert into default (via the raw test helper, which doesn't set namespace).
    let default_src = fix
        .insert_source("Default NS source", "Content in the default namespace.")
        .await;

    let custom_src = insert_ns_source(
        &mut fix,
        "custom-ns",
        "Custom NS source",
        "Content in the custom namespace.",
    )
    .await;

    // Default-scoped service should see the default source, not the custom one.
    let default_svc = SourceService::new(fix.pool.clone()); // namespace = "default"
    assert!(
        default_svc.get(default_src).await.is_ok(),
        "default source should be visible in default namespace"
    );
    assert!(
        default_svc.get(custom_src).await.is_err(),
        "custom-ns source should NOT be visible in default namespace"
    );

    // Custom-scoped service should see its source, not the default one.
    let custom_svc = SourceService::new(fix.pool.clone()).with_namespace("custom-ns".into());
    assert!(
        custom_svc.get(custom_src).await.is_ok(),
        "custom-ns source should be visible in custom-ns namespace"
    );
    assert!(
        custom_svc.get(default_src).await.is_err(),
        "default source should NOT be visible in custom-ns namespace"
    );

    fix.cleanup().await;
}
