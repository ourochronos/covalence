//! Integration tests for covalence#161 — Provenance Cap & Auto-Split.
//!
//! # What is tested
//!
//! | Test                                                 | Description                                          |
//! |------------------------------------------------------|------------------------------------------------------|
//! | `auto_split_over_threshold_triggers_split`           | handle_auto_split splits article with >50 ORIGINATES |
//! | `auto_split_idempotent_on_archived_article`          | Already-archived article → early return / skipped   |
//! | `auto_split_produces_two_children_with_correct_edges`| Both children get correct ORIGINATES + CHILD_OF edges|
//! | `compile_enqueues_auto_split_when_over_threshold`    | handle_compile enqueues auto_split when count > cap  |
//! | `compile_no_auto_split_when_under_threshold`         | handle_compile does NOT enqueue for small articles   |

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use sqlx::Row;
use uuid::Uuid;

use covalence_engine::worker::{
    QueueTask, handle_compile,
    llm::LlmClient,
    provenance_cap::{DEFAULT_PROVENANCE_SPLIT_THRESHOLD, handle_auto_split},
};

use super::helpers::{MockLlmClient, TestFixture};

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Insert `count` source nodes and ORIGINATES edges to `article_id`.
async fn seed_originates_edges(fix: &mut TestFixture, article_id: Uuid, count: usize) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let src = fix
            .insert_source(
                &format!("Provenance source {i}"),
                &format!("Source content {i}: distinct text for clustering test."),
            )
            .await;
        fix.insert_originates_edge(src, article_id).await;
        ids.push(src);
    }
    ids
}

/// Insert a well-separated embedding: dimension `slot % 1536` = 1.0, rest 0.0.
/// Different slots produce orthogonal unit vectors — useful for cluster tests.
async fn insert_slotted_embedding(fix: &mut TestFixture, node_id: Uuid, slot: usize) {
    let dim = 1536usize;
    let idx = slot % dim;
    let mut vals = vec![0.0_f32; dim];
    vals[idx] = 1.0_f32;
    let literal = format!(
        "[{}]",
        vals.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    sqlx::query(&format!(
        "INSERT INTO covalence.node_embeddings (node_id, embedding, model)
         VALUES ($1, $2::halfvec({dim}), 'test-slotted')
         ON CONFLICT (node_id) DO UPDATE SET embedding = EXCLUDED.embedding"
    ))
    .bind(node_id)
    .bind(&literal)
    .execute(&fix.pool)
    .await
    .expect("insert_slotted_embedding failed");
}

/// Insert an embedding computed with the same deterministic hash as MockLlmClient.
/// Used to prime the dedup cache so a subsequent compile deduplicates to `node_id`.
async fn insert_mock_embedding_for_content(fix: &TestFixture, node_id: Uuid, content: &str) {
    let emb = MockLlmClient::deterministic_embedding(content);
    let dim = emb.len();
    let literal = format!(
        "[{}]",
        emb.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    sqlx::query(&format!(
        "INSERT INTO covalence.node_embeddings (node_id, embedding, model)
         VALUES ($1, $2::halfvec({dim}), 'test-mock')
         ON CONFLICT (node_id) DO UPDATE SET embedding = EXCLUDED.embedding"
    ))
    .bind(node_id)
    .bind(&literal)
    .execute(&fix.pool)
    .await
    .expect("insert_mock_embedding_for_content failed");
}

// ─── Test 1: over-threshold → split ──────────────────────────────────────────

/// An article with > DEFAULT_PROVENANCE_SPLIT_THRESHOLD ORIGINATES edges must
/// be split into two active children.  The parent must be archived (NOT tombstoned).
#[tokio::test]
#[serial]
async fn auto_split_over_threshold_triggers_split() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let article_id = fix
        .insert_article(
            "Over-threshold article",
            "This article has too many provenance sources and must be auto-split.",
        )
        .await;

    let n = DEFAULT_PROVENANCE_SPLIT_THRESHOLD + 1; // 51
    let source_ids = seed_originates_edges(&mut fix, article_id, n).await;

    // Give sources two distinct embedding directions so k-means gets real clusters.
    for (i, &src_id) in source_ids.iter().enumerate() {
        let slot = if i < n / 2 { i } else { 768 + (i - n / 2) };
        insert_slotted_embedding(&mut fix, src_id, slot).await;
    }

    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");
    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task(
        "auto_split",
        None,
        json!({
            "article_id": article_id.to_string(),
            "reason": "originates_overflow",
            "originates_count_at_trigger": n,
        }),
    );

    let result = handle_auto_split(&fix.pool, &llm, &task)
        .await
        .expect("handle_auto_split should succeed for over-threshold article");

    // Parent is archived.
    assert_eq!(
        fix.node_status(article_id).await,
        "archived",
        "parent article must be archived after auto_split"
    );

    // Two active children created.
    let child_a = Uuid::parse_str(result["child_a_id"].as_str().unwrap()).unwrap();
    let child_b = Uuid::parse_str(result["child_b_id"].as_str().unwrap()).unwrap();
    fix.track(child_a);
    fix.track(child_b);

    assert_eq!(
        fix.node_status(child_a).await,
        "active",
        "child A must be active"
    );
    assert_eq!(
        fix.node_status(child_b).await,
        "active",
        "child B must be active"
    );

    // All sources accounted for.
    let cluster_a = result["cluster_a_len"].as_u64().unwrap_or(0) as usize;
    let cluster_b = result["cluster_b_len"].as_u64().unwrap_or(0) as usize;
    assert_eq!(
        cluster_a + cluster_b,
        n,
        "all sources must be assigned to a cluster"
    );
    assert!(cluster_a >= 1, "cluster A non-empty");
    assert!(cluster_b >= 1, "cluster B non-empty");

    fix.cleanup().await;
}

// ─── Test 2: idempotency — archived article is skipped ───────────────────────

/// When `handle_auto_split` is invoked for an article that is no longer active
/// (e.g. already archived by a prior run), it must return early with
/// `{"skipped": true, "reason": "article_not_active"}`.
#[tokio::test]
#[serial]
async fn auto_split_idempotent_on_archived_article() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let article_id = fix
        .insert_article(
            "Already-archived article",
            "Content that was already split.",
        )
        .await;

    // Pre-archive to simulate a completed prior split.
    sqlx::query("UPDATE covalence.nodes SET status = 'archived' WHERE id = $1")
        .bind(article_id)
        .execute(&fix.pool)
        .await
        .expect("failed to archive article");

    let task = TestFixture::make_task(
        "auto_split",
        None,
        json!({
            "article_id": article_id.to_string(),
            "reason": "originates_overflow",
            "originates_count_at_trigger": 55,
        }),
    );

    let result = handle_auto_split(&fix.pool, &llm, &task)
        .await
        .expect("handle_auto_split should return Ok for archived article");

    assert_eq!(
        result["skipped"].as_bool(),
        Some(true),
        "archived article should be skipped"
    );
    assert_eq!(
        result["reason"].as_str(),
        Some("article_not_active"),
        "skip reason should be article_not_active"
    );

    fix.cleanup().await;
}

// ─── Test 3: children inherit correct edges ───────────────────────────────────

/// After a successful auto_split, each child must:
/// - be 'active'
/// - have ORIGINATES edges to exactly the sources in its cluster
/// - have a CHILD_OF edge pointing to the parent
/// - have rows in `article_sources` for its sources
/// - sources must be disjoint across children
#[tokio::test]
#[serial]
async fn auto_split_produces_two_children_with_correct_edges() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let article_id = fix
        .insert_article(
            "Clustered article",
            "An article with two well-separated source clusters.",
        )
        .await;

    // 10 + 10 = 20 sources total; cluster A at slots 0-9, cluster B at slots 768-777.
    let n_per = 10usize;
    let mut all_src_ids = Vec::new();

    for i in 0..n_per {
        let src = fix
            .insert_source(&format!("Cluster A-{i}"), &format!("Cluster A content {i}"))
            .await;
        fix.insert_originates_edge(src, article_id).await;
        insert_slotted_embedding(&mut fix, src, i).await;
        all_src_ids.push(src);
    }
    for i in 0..n_per {
        let src = fix
            .insert_source(&format!("Cluster B-{i}"), &format!("Cluster B content {i}"))
            .await;
        fix.insert_originates_edge(src, article_id).await;
        insert_slotted_embedding(&mut fix, src, 768 + i).await;
        all_src_ids.push(src);
    }

    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");
    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task(
        "auto_split",
        None,
        json!({
            "article_id": article_id.to_string(),
            "reason": "originates_overflow",
            "originates_count_at_trigger": n_per * 2,
        }),
    );

    let result = handle_auto_split(&fix.pool, &llm, &task)
        .await
        .expect("handle_auto_split should succeed");

    let child_a = Uuid::parse_str(result["child_a_id"].as_str().unwrap()).unwrap();
    let child_b = Uuid::parse_str(result["child_b_id"].as_str().unwrap()).unwrap();
    fix.track(child_a);
    fix.track(child_b);

    // ORIGINATES edges for each child.
    let child_a_srcs: Vec<Uuid> = sqlx::query(
        "SELECT source_node_id FROM covalence.edges
         WHERE  target_node_id = $1 AND edge_type = 'ORIGINATES'",
    )
    .bind(child_a)
    .fetch_all(&fix.pool)
    .await
    .expect("query child_a sources")
    .iter()
    .map(|r| r.get("source_node_id"))
    .collect();

    let child_b_srcs: Vec<Uuid> = sqlx::query(
        "SELECT source_node_id FROM covalence.edges
         WHERE  target_node_id = $1 AND edge_type = 'ORIGINATES'",
    )
    .bind(child_b)
    .fetch_all(&fix.pool)
    .await
    .expect("query child_b sources")
    .iter()
    .map(|r| r.get("source_node_id"))
    .collect();

    // Total = original source count.
    assert_eq!(
        child_a_srcs.len() + child_b_srcs.len(),
        n_per * 2,
        "total ORIGINATES across children should equal original count"
    );

    // Sources are disjoint.
    let a_set: std::collections::HashSet<Uuid> = child_a_srcs.iter().copied().collect();
    let b_set: std::collections::HashSet<Uuid> = child_b_srcs.iter().copied().collect();
    assert!(
        a_set.is_disjoint(&b_set),
        "sources must not appear in both children"
    );

    // CHILD_OF edges exist.
    for &(child_id, label) in &[(child_a, "A"), (child_b, "B")] {
        let cnt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM covalence.edges
             WHERE  source_node_id = $1 AND target_node_id = $2 AND edge_type = 'CHILD_OF'",
        )
        .bind(child_id)
        .bind(article_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert_eq!(cnt, 1, "child {label} must have exactly one CHILD_OF edge");
    }

    // article_sources bridge populated.
    for &(child_id, label) in &[(child_a, "A"), (child_b, "B")] {
        let cnt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM covalence.article_sources
             WHERE  article_id = $1 AND relationship = 'originates'",
        )
        .bind(child_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert!(cnt > 0, "article_sources must have rows for child {label}");
    }

    fix.cleanup().await;
}

// ─── Test 4: compile enqueues auto_split when over threshold ─────────────────

/// `handle_compile` must enqueue an `auto_split` task when the total count of
/// ORIGINATES edges on the compiled article exceeds PROVENANCE_SPLIT_THRESHOLD.
///
/// Strategy:
///  1. Compile with 2 sources → creates article A.
///  2. Store the embedding that MockLlmClient would produce for the compiled
///     content, so subsequent compile calls deduplicate to article A.
///  3. Seed `threshold - 1` additional ORIGINATES edges directly on A.
///  4. Compile the same 2 sources again → dedup fires for A → trigger counts
///     total edges (`threshold - 1 + 2 = threshold + 1`) → enqueues auto_split.
#[tokio::test]
#[serial]
async fn compile_enqueues_auto_split_when_over_threshold() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let src1 = fix
        .insert_source("Source alpha", "Alpha content for overflow test.")
        .await;
    let src2 = fix
        .insert_source("Source beta", "Beta content for overflow test.")
        .await;

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_task_type("split");
    fix.track_task_type("critique_article");
    fix.track_task_type("auto_split");
    fix.track_task_type("infer_article_edges");

    // ── Step 1: first compile → article A. ───────────────────────────────────
    let task1 = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src1.to_string(), src2.to_string()],
            "title_hint": "Overflow test article",
        }),
    );
    let r1 = handle_compile(&fix.pool, &llm, &task1)
        .await
        .expect("first compile should succeed");
    let article_a = Uuid::parse_str(r1["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_a);

    // ── Step 2: prime the embedding cache so compile deduplicates to article A.
    // The mock LLM always returns the same content for "synthesizer" prompts.
    let mock_content = "This article synthesizes the provided source documents into a coherent \
                        knowledge unit. It covers the key facts and relationships described \
                        across the source material.";
    insert_mock_embedding_for_content(&fix, article_a, mock_content).await;

    // ── Step 3: seed `threshold - 1` extra ORIGINATES on article A so that
    //    after the second compile adds 0 new edges (ON CONFLICT DO NOTHING),
    //    total = (threshold - 1) + 2 = threshold + 1 > threshold.
    let n_extra = DEFAULT_PROVENANCE_SPLIT_THRESHOLD - 1;
    for i in 0..n_extra {
        let extra_src = fix
            .insert_source(&format!("Extra source {i}"), &format!("Extra content {i}."))
            .await;
        sqlx::query(
            "INSERT INTO covalence.edges
                 (source_node_id, target_node_id, edge_type, weight, created_by)
             VALUES ($1, $2, 'ORIGINATES', 1.0, 'test_seed')
             ON CONFLICT DO NOTHING",
        )
        .bind(extra_src)
        .bind(article_a)
        .execute(&fix.pool)
        .await
        .expect("seed extra ORIGINATES edge");
    }

    // Sanity: no auto_split queued yet.
    let queued_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.slow_path_queue
         WHERE  task_type = 'auto_split'
           AND  payload->>'article_id' = $1",
    )
    .bind(article_a.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);
    assert_eq!(
        queued_before, 0,
        "no auto_split queued before second compile"
    );

    // ── Step 4: second compile with same sources → dedup to article A →
    //    trigger counts total ORIGINATES > threshold → enqueues auto_split.
    let task2 = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src1.to_string(), src2.to_string()],
            "title_hint": "Overflow test article",
        }),
    );
    handle_compile(&fix.pool, &llm, &task2)
        .await
        .expect("second compile should succeed");

    let queued_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.slow_path_queue
         WHERE  task_type = 'auto_split'
           AND  payload->>'article_id' = $1",
    )
    .bind(article_a.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert!(
        queued_after >= 1,
        "auto_split must be enqueued when ORIGINATES count exceeds \
         PROVENANCE_SPLIT_THRESHOLD (article_a={article_a})"
    );

    fix.cleanup().await;
}

// ─── Test 5: compile does NOT enqueue auto_split when under threshold ─────────

/// A freshly compiled article with only 2 sources must NOT trigger an
/// `auto_split` enqueue.
#[tokio::test]
#[serial]
async fn compile_no_auto_split_when_under_threshold() {
    let mut fix = TestFixture::new().await;
    let mock = Arc::new(MockLlmClient::new());
    let llm: Arc<dyn LlmClient> = mock.clone();

    let src1 = fix.insert_source("Source A", "Content for source A.").await;
    let src2 = fix.insert_source("Source B", "Content for source B.").await;

    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_task_type("split");
    fix.track_task_type("critique_article");
    fix.track_task_type("auto_split");
    fix.track_task_type("infer_article_edges");

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src1.to_string(), src2.to_string()],
            "title_hint": "Small article",
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap()).unwrap();
    fix.track(article_id);

    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM covalence.slow_path_queue
         WHERE  task_type = 'auto_split'
           AND  payload->>'article_id' = $1",
    )
    .bind(article_id.to_string())
    .fetch_one(&fix.pool)
    .await
    .unwrap_or(0);

    assert_eq!(
        queued, 0,
        "no auto_split must be enqueued for a small article (article_id={article_id})"
    );

    fix.cleanup().await;
}
