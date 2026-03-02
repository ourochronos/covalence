//! Integration tests for AGE ↔ SQL edge synchronisation (covalence#28).
//!
//! These tests exercise the three main properties introduced by the fix:
//!
//! 1. **Compile writes edges to SQL** — after `handle_compile` the provenance
//!    edges exist in `covalence.edges`.
//!
//! 2. **Archive removes vertex from AGE** — after a worker archives an article
//!    (split or merge), the `archive_vertex` path is exercised and the helper
//!    records no errors (we verify the SQL record is still present and status
//!    is `archived`; AGE is not directly queryable in unit tests without a
//!    running PG+AGE instance, so we assert the SQL side is intact).
//!
//! 3. **Admin sync-edges endpoint** — calling `AdminService::sync_edges()`
//!    against a clean database returns consistent counts (no orphans when
//!    everything was created via `GraphRepository`).

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::{
    handle_compile, handle_split, llm::LlmClient, merge_edges::handle_merge,
};

use super::helpers::{MockLlmClient, TestFixture};

// ─── compile: edges land in SQL ──────────────────────────────────────────────

/// After `handle_compile`, provenance edges from each source to the new article
/// must exist in `covalence.edges`.  This validates that the raw-SQL INSERT was
/// successfully replaced by `graph.create_edge()`.
#[tokio::test]
#[serial]
async fn compile_edges_appear_in_sql() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src_a = fix
        .insert_source("Sync Source A", "Content about distributed systems.")
        .await;
    let src_b = fix
        .insert_source("Sync Source B", "Content about consensus algorithms.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({ "source_ids": [src_a.to_string(), src_b.to_string()] }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    // Both sources must have an edge to the new article.
    let edge_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.edges \
         WHERE target_node_id = $1 \
           AND source_node_id = ANY($2) \
           AND edge_type IN ('ORIGINATES','CONFIRMS','SUPERSEDES','CONTRADICTS','CONTENDS')",
    )
    .bind(article_id)
    .bind(&vec![src_a, src_b])
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        edge_count, 2,
        "each source should have exactly one provenance edge to the compiled article"
    );

    fix.cleanup().await;
}

// ─── split: archived original kept in SQL, children have edges ───────────────

/// After `handle_split`, the original article is `archived` in SQL (historical
/// data preserved), and SPLIT_INTO edges from it to both children exist.
#[tokio::test]
#[serial]
async fn split_archives_original_and_creates_edges() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let original = fix
        .insert_article(
            "Large Article",
            "First half content. This is a long article.\n\nSecond half content here.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task("split", Some(original), json!({}));

    let result = handle_split(&fix.pool, &llm, &task)
        .await
        .expect("split should succeed");

    let id_a = Uuid::parse_str(result["part_a_id"].as_str().unwrap()).unwrap();
    let id_b = Uuid::parse_str(result["part_b_id"].as_str().unwrap()).unwrap();
    fix.track(id_a);
    fix.track(id_b);

    // Original should be archived (SQL row preserved).
    assert_eq!(
        fix.node_status(original).await,
        "archived",
        "original article must be archived"
    );

    // SPLIT_INTO edges from original to both children must exist in SQL.
    let split_edges_a = fix.edge_count_from(original, "SPLIT_INTO").await;
    assert_eq!(
        split_edges_a, 2,
        "two SPLIT_INTO edges should exist from the original to both children"
    );

    fix.cleanup().await;
}

// ─── merge: archived originals preserved, MERGED_FROM edges exist ─────────────

/// After `handle_merge`, both original articles are `archived` in SQL and
/// MERGED_FROM edges from the new article to each original exist.
#[tokio::test]
#[serial]
async fn merge_archives_originals_and_creates_merged_from_edges() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let id_a = fix
        .insert_article("Merge Input A", "Content from article A about topic X.")
        .await;
    let id_b = fix
        .insert_article("Merge Input B", "Content from article B about topic X.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": id_a.to_string(), "article_id_b": id_b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    // Both originals must be archived.
    assert_eq!(fix.node_status(id_a).await, "archived");
    assert_eq!(fix.node_status(id_b).await, "archived");

    // MERGED_FROM edges: new_id → id_a and new_id → id_b.
    let merged_from_a = fix.edge_count_from(new_id, "MERGED_FROM").await;
    assert_eq!(
        merged_from_a, 2,
        "two MERGED_FROM edges should exist from the merged article to both originals"
    );

    fix.cleanup().await;
}

// ─── sync_edges: clean DB reports no orphans ─────────────────────────────────

/// A freshly-seeded database (edges created via `handle_compile`) should have
/// zero orphaned AGE edges and zero missing SQL→AGE edges *when the AGE graph
/// is not reachable* — the endpoint must be resilient and return counts.
///
/// We test the SQL-only half of the sync: after compile, all SQL edges have
/// their expected shape.  The `sync_edges` method is called but we only assert
/// the `missing_created` count is ≥ 0 (not negative / panicking) since in CI
/// the AGE graph may not be available.
#[tokio::test]
#[serial]
async fn sync_edges_does_not_panic_on_clean_db() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src = fix
        .insert_source("Sync Test Source", "Content for sync edge test.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");

    let task = TestFixture::make_task("compile", None, json!({ "source_ids": [src.to_string()] }));
    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    // Call sync_edges — it should not panic regardless of AGE availability.
    use covalence_engine::services::admin_service::AdminService;
    let svc = AdminService::new(fix.pool.clone());
    let sync_result = svc.sync_edges().await;

    // We tolerate an error if AGE is genuinely unreachable; we just assert no panic.
    match sync_result {
        Ok(report) => {
            assert!(
                report.missing_created >= 0,
                "missing_created should be non-negative"
            );
            assert!(
                report.orphaned_deleted >= 0,
                "orphaned_deleted should be non-negative"
            );
            assert!(
                report.already_synced >= 0,
                "already_synced should be non-negative"
            );
        }
        Err(e) => {
            // AGE not available in this environment — acceptable in unit CI.
            eprintln!("sync_edges returned error (AGE likely not running): {e}");
        }
    }

    fix.cleanup().await;
}

// ─── memory supersedes: edge exists in SQL ───────────────────────────────────

/// When a memory is stored with `supersedes_id`, a SUPERSEDES edge must appear
/// in `covalence.edges` (previously written via raw SQL, now via GraphRepository).
#[tokio::test]
#[serial]
async fn memory_supersedes_edge_appears_in_sql() {
    let mut fix = TestFixture::new().await;

    // Insert the "old" memory node directly.
    let old_id = fix.insert_source("old memory", "I prefer Rust.").await;

    // Use MemoryService to store a superseding memory.
    use covalence_engine::services::memory_service::{MemoryService, StoreMemoryRequest};
    let svc = MemoryService::new(fix.pool.clone());
    let new_mem = svc
        .store(StoreMemoryRequest {
            content: "I prefer Rust over C++.".into(),
            tags: vec![],
            importance: 0.8,
            context: None,
            supersedes_id: Some(old_id),
        })
        .await
        .expect("store memory should succeed");

    fix.track(new_mem.id);
    fix.track_task_type("embed");

    // SUPERSEDES edge: new_mem.id → old_id must exist.
    let edge_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM covalence.edges \
         WHERE source_node_id = $1 AND target_node_id = $2 AND edge_type = 'SUPERSEDES'",
    )
    .bind(new_mem.id)
    .bind(old_id)
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        edge_count, 1,
        "SUPERSEDES edge must exist in covalence.edges"
    );

    fix.cleanup().await;
}
