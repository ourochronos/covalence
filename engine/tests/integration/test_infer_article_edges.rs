//! Integration tests for the `infer_article_edges` slow-path task (covalence#160).
//!
//! Verifies:
//! 1. Enqueue + run: after running the handler for an article that shares
//!    sources with a neighbour, a `RELATES_TO` edge is inserted.
//! 2. Idempotency: re-running the same task does not create duplicate edges.
//! 3. Tier 2 (domain_path): two articles with ≥ 2 shared tags get an edge.
//! 4. Graceful degradation: an article with no sources/embeddings/tags produces
//!    no edges and no panic.
//! 5. No self-loops: the handler never inserts an edge from an article to itself.

use serde_json::json;
use serial_test::serial;
use std::sync::Arc;
use uuid::Uuid;

use covalence_engine::worker::{infer_article_edges::handle_infer_article_edges, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Count RELATES_TO or EXTENDS edges whose source is `article_id`.
async fn count_inferred_edges(pool: &sqlx::PgPool, article_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 \
           AND edge_type IN ('RELATES_TO', 'EXTENDS', 'CONFIRMS', 'CONTRADICTS')",
    )
    .bind(article_id)
    .fetch_one(pool)
    .await
    .expect("count_inferred_edges query failed")
}

/// Insert an `article_sources` row linking an article to a source.
async fn link_article_source(pool: &sqlx::PgPool, article_id: Uuid, source_id: Uuid) {
    sqlx::query(
        "INSERT INTO covalence.article_sources \
             (article_id, source_id, relationship, causal_weight, confidence) \
         VALUES ($1, $2, 'originates', 1.0, 1.0) \
         ON CONFLICT DO NOTHING",
    )
    .bind(article_id)
    .bind(source_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("link_article_source failed: {e}"));
}

/// Insert an article with explicit `domain_path`.
async fn insert_article_with_domain(
    fix: &mut TestFixture,
    title: &str,
    content: &str,
    domain: Vec<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    let domain_arr: Vec<String> = domain.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, status, title, content, metadata, domain_path) \
         VALUES ($1, 'article', 'active', $2, $3, '{}'::jsonb, $4)",
    )
    .bind(id)
    .bind(title)
    .bind(content)
    .bind(&domain_arr)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_article_with_domain({title}) failed: {e}"));
    fix.track(id)
}

// ─── Test 1: Tier 1 — shared sources produce a RELATES_TO edge ───────────────

/// Given two articles that share ≥ 30 % of their source sets, the handler
/// must insert a `RELATES_TO` edge from the subject to the neighbour.
#[tokio::test]
#[serial]
async fn test_tier1_shared_sources_creates_edge() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // Two shared sources + one exclusive source each → Jaccard = 2/(2+1+1) = 0.5 ≥ 0.3
    let src_shared_1 = fix.insert_source("Shared Source A", "Content A").await;
    let src_shared_2 = fix.insert_source("Shared Source B", "Content B").await;
    let src_only_subject = fix.insert_source("Subject Only", "Content C").await;
    let src_only_neighbour = fix.insert_source("Neighbour Only", "Content D").await;

    let subject = fix
        .insert_article("Subject Article", "Talks about A and B and C.")
        .await;
    let neighbour = fix
        .insert_article("Neighbour Article", "Talks about A and B and D.")
        .await;

    link_article_source(&fix.pool, subject, src_shared_1).await;
    link_article_source(&fix.pool, subject, src_shared_2).await;
    link_article_source(&fix.pool, subject, src_only_subject).await;
    link_article_source(&fix.pool, neighbour, src_shared_1).await;
    link_article_source(&fix.pool, neighbour, src_shared_2).await;
    link_article_source(&fix.pool, neighbour, src_only_neighbour).await;

    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task("infer_article_edges", Some(subject), json!({}));
    let result = handle_infer_article_edges(&fix.pool, &llm, &task)
        .await
        .expect("handle_infer_article_edges should succeed");

    let edges_inserted: i64 = result["edges_inserted"].as_i64().unwrap_or(0);
    assert!(
        edges_inserted >= 1,
        "Expected ≥ 1 edge inserted, got {edges_inserted}"
    );

    let edge_count = count_inferred_edges(&fix.pool, subject).await;
    assert!(
        edge_count >= 1,
        "Expected ≥ 1 RELATES_TO/EXTENDS edge from subject, got {edge_count}"
    );

    // Confirm the edge points at the neighbour.
    let to_neighbour: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 \
           AND edge_type IN ('RELATES_TO', 'EXTENDS', 'CONFIRMS', 'CONTRADICTS')",
    )
    .bind(subject)
    .bind(neighbour)
    .fetch_one(&fix.pool)
    .await
    .expect("edge count query failed");

    assert_eq!(
        to_neighbour, 1,
        "Expected exactly one edge from subject → neighbour"
    );

    fix.cleanup().await;
}

// ─── Test 2: Idempotency ──────────────────────────────────────────────────────

/// Running the handler twice for the same article must not create duplicate
/// edges.
#[tokio::test]
#[serial]
async fn test_infer_article_edges_idempotent() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src1 = fix.insert_source("Shared X", "Content X").await;
    let src2 = fix.insert_source("Shared Y", "Content Y").await;

    let subject = fix.insert_article("Subject (idempotent)", "Content…").await;
    let neighbour = fix
        .insert_article("Neighbour (idempotent)", "Content…")
        .await;

    link_article_source(&fix.pool, subject, src1).await;
    link_article_source(&fix.pool, subject, src2).await;
    link_article_source(&fix.pool, neighbour, src1).await;
    link_article_source(&fix.pool, neighbour, src2).await;

    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task("infer_article_edges", Some(subject), json!({}));

    // First run.
    handle_infer_article_edges(&fix.pool, &llm, &task)
        .await
        .expect("first run should succeed");

    let edges_after_first = count_inferred_edges(&fix.pool, subject).await;
    assert!(edges_after_first >= 1, "first run should insert ≥ 1 edge");

    // Second run — should be a no-op.
    let result2 = handle_infer_article_edges(&fix.pool, &llm, &task)
        .await
        .expect("second run should succeed");

    let edges_after_second = count_inferred_edges(&fix.pool, subject).await;
    assert_eq!(
        edges_after_first, edges_after_second,
        "second run must not insert duplicate edges"
    );

    let skipped: i64 = result2["edges_skipped"].as_i64().unwrap_or(0);
    assert!(
        skipped >= 1,
        "second run should report ≥ 1 skipped edge, got {skipped}"
    );

    fix.cleanup().await;
}

// ─── Test 3: Tier 2 — domain_path overlap ────────────────────────────────────

/// Two articles with ≥ 2 shared `domain_path` tags must receive a semantic
/// edge even when they share no sources and have no embeddings.
#[tokio::test]
#[serial]
async fn test_tier2_domain_path_creates_edge() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let subject = insert_article_with_domain(
        &mut fix,
        "Domain Subject",
        "Talks about Rust and distributed systems.",
        vec!["rust", "distributed-systems", "databases"],
    )
    .await;

    let _neighbour = insert_article_with_domain(
        &mut fix,
        "Domain Neighbour",
        "Also about Rust and distributed systems.",
        vec!["rust", "distributed-systems", "caching"],
    )
    .await;

    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task("infer_article_edges", Some(subject), json!({}));
    let result = handle_infer_article_edges(&fix.pool, &llm, &task)
        .await
        .expect("handle_infer_article_edges should succeed");

    let edges_inserted: i64 = result["edges_inserted"].as_i64().unwrap_or(0);
    assert!(
        edges_inserted >= 1,
        "Expected ≥ 1 edge from Tier 2 domain_path overlap, got {edges_inserted}"
    );

    fix.cleanup().await;
}

// ─── Test 4: Graceful degradation — article with no signals ──────────────────

/// An article with no sources, no embeddings, and no domain_path tags must
/// complete without errors and insert zero edges.
#[tokio::test]
#[serial]
async fn test_no_signals_produces_no_edges() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let lonely = fix
        .insert_article("Lonely Article", "No sources, no embeddings.")
        .await;

    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task("infer_article_edges", Some(lonely), json!({}));
    let result = handle_infer_article_edges(&fix.pool, &llm, &task)
        .await
        .expect("handler should succeed even with no signals");

    let edges_inserted: i64 = result["edges_inserted"].as_i64().unwrap_or(-1);
    assert_eq!(
        edges_inserted, 0,
        "isolated article should produce 0 edges, got {edges_inserted}"
    );

    fix.cleanup().await;
}

// ─── Test 5: No self-loops ────────────────────────────────────────────────────

/// The handler must never create an edge from an article back to itself,
/// even when the article appears in the neighbour result set (which it should
/// not, but guard against DB anomalies).
#[tokio::test]
#[serial]
async fn test_no_self_loops() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let subject = insert_article_with_domain(
        &mut fix,
        "Self-loop subject",
        "Should never point to itself.",
        vec!["rust", "safety", "ownership"],
    )
    .await;

    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task("infer_article_edges", Some(subject), json!({}));
    handle_infer_article_edges(&fix.pool, &llm, &task)
        .await
        .expect("handler should succeed");

    let self_loop: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $1",
    )
    .bind(subject)
    .fetch_one(&fix.pool)
    .await
    .expect("self-loop count query failed");

    assert_eq!(self_loop, 0, "handler must not create self-loop edges");

    fix.cleanup().await;
}
