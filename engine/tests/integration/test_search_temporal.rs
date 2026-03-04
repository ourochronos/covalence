//! Integration tests for temporal date filters on SearchRequest (covalence#34).
//!
//! Tests cover:
//! * `search_after_excludes_old_content` — `after` filter removes older nodes.
//! * `search_before_excludes_new_content` — `before` filter removes newer nodes.
//! * `search_date_range_combined` — combined `after` + `before` keeps only the
//!   middle node of three created at different times.

use chrono::{Duration, Utc};
use serial_test::serial;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Insert a source node and immediately override its `created_at` to a fixed
/// offset from now (positive or negative seconds).
async fn insert_source_at_offset(
    fix: &mut TestFixture,
    title: &str,
    content: &str,
    offset: Duration,
) -> uuid::Uuid {
    let id = fix.insert_source(title, content).await;
    let target_ts = Utc::now() + offset;
    sqlx::query("UPDATE covalence.nodes SET created_at = $1 WHERE id = $2")
        .bind(target_ts)
        .bind(id)
        .execute(&fix.pool)
        .await
        .unwrap_or_else(|e| panic!("backdating created_at for {id} failed: {e}"));
    id
}

// ─── tests ────────────────────────────────────────────────────────────────────

/// Searching with `after` set to a timestamp between two nodes must return only
/// the node created *after* that timestamp, excluding the older one.
///
/// Setup:
///   • "old" source: created_at = now − 2 h
///   • "new" source: created_at ≈ now
///   • `after` = now − 1 h  →  only "new" should appear
#[tokio::test]
#[serial]
async fn search_after_excludes_old_content() {
    let mut fix = TestFixture::new().await;

    let content = "temporal after filter test unique phrase zorblax quuxle frobinate wibble";

    let old_id = insert_source_at_offset(
        &mut fix,
        "Old Source After Test",
        content,
        Duration::hours(-2),
    )
    .await;
    let new_id = insert_source_at_offset(
        &mut fix,
        "New Source After Test",
        content,
        Duration::seconds(-5),
    )
    .await;

    fix.insert_embedding(old_id).await;
    fix.insert_embedding(new_id).await;

    let cutoff = Utc::now() - Duration::hours(1);

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
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: Some(cutoff),
        before: None,
        min_score: None,
        spreading_activation: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results, _meta) = svc
        .search(req)
        .await
        .expect("after-filter search should succeed");

    assert!(
        results.iter().any(|r| r.node_id == new_id),
        "new source (created ~now) should appear when after={cutoff}"
    );
    assert!(
        !results.iter().any(|r| r.node_id == old_id),
        "old source (created 2 h ago) should be excluded when after={cutoff}"
    );

    fix.cleanup().await;
}

/// Searching with `before` set to a timestamp between two nodes must return only
/// the node created *before* that timestamp, excluding the newer one.
///
/// Setup:
///   • "old" source: created_at = now − 2 h
///   • "new" source: created_at ≈ now
///   • `before` = now − 1 h  →  only "old" should appear
#[tokio::test]
#[serial]
async fn search_before_excludes_new_content() {
    let mut fix = TestFixture::new().await;

    let content = "temporal before filter test unique phrase snollygoster lollygag bumfuzzle";

    let old_id = insert_source_at_offset(
        &mut fix,
        "Old Source Before Test",
        content,
        Duration::hours(-2),
    )
    .await;
    let new_id = insert_source_at_offset(
        &mut fix,
        "New Source Before Test",
        content,
        Duration::seconds(-5),
    )
    .await;

    fix.insert_embedding(old_id).await;
    fix.insert_embedding(new_id).await;

    let cutoff = Utc::now() - Duration::hours(1);

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
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: None,
        before: Some(cutoff),
        min_score: None,
        spreading_activation: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results, _meta) = svc
        .search(req)
        .await
        .expect("before-filter search should succeed");

    assert!(
        results.iter().any(|r| r.node_id == old_id),
        "old source (created 2 h ago) should appear when before={cutoff}"
    );
    assert!(
        !results.iter().any(|r| r.node_id == new_id),
        "new source (created ~now) should be excluded when before={cutoff}"
    );

    fix.cleanup().await;
}

/// Searching with both `after` and `before` set must return only the node
/// created within the specified time window.
///
/// Setup:
///   • "old"    source: created_at = now − 4 h
///   • "middle" source: created_at = now − 2 h
///   • "new"    source: created_at ≈ now
///   • `after`  = now − 3 h, `before` = now − 1 h  →  only "middle" appears
#[tokio::test]
#[serial]
async fn search_date_range_combined() {
    let mut fix = TestFixture::new().await;

    let content = "temporal date range combined test unique phrase absquatulate flibbertigibbet";

    let old_id = insert_source_at_offset(
        &mut fix,
        "Old Source Range Test",
        content,
        Duration::hours(-4),
    )
    .await;
    let middle_id = insert_source_at_offset(
        &mut fix,
        "Middle Source Range Test",
        content,
        Duration::hours(-2),
    )
    .await;
    let new_id = insert_source_at_offset(
        &mut fix,
        "New Source Range Test",
        content,
        Duration::seconds(-5),
    )
    .await;

    fix.insert_embedding(old_id).await;
    fix.insert_embedding(middle_id).await;
    fix.insert_embedding(new_id).await;

    let after_cutoff = Utc::now() - Duration::hours(3);
    let before_cutoff = Utc::now() - Duration::hours(1);

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
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: Some(after_cutoff),
        before: Some(before_cutoff),
        min_score: None,
        spreading_activation: None,
        facet_function: None,
        facet_scope: None,
        explain: None,
    };

    let (results, _meta) = svc
        .search(req)
        .await
        .expect("date-range search should succeed");

    assert!(
        results.iter().any(|r| r.node_id == middle_id),
        "middle source (created 2 h ago) should appear in [{after_cutoff}, {before_cutoff}]"
    );
    assert!(
        !results.iter().any(|r| r.node_id == old_id),
        "old source (created 4 h ago) should be excluded by after={after_cutoff}"
    );
    assert!(
        !results.iter().any(|r| r.node_id == new_id),
        "new source (created ~now) should be excluded by before={before_cutoff}"
    );

    fix.cleanup().await;
}
