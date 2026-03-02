//! Integration tests for temporal and context-scoped memory recall (tracking#98).
//!
//! Tests cover:
//! * [`since`] filter — only returns memories created at or after a given timestamp.
//! * [`context_prefix`] filter — only returns memories whose `context` metadata
//!   field starts with the given prefix.

use chrono::{Duration, Utc};
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::services::memory_service::{MemoryService, RecallRequest};

use super::helpers::TestFixture;

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Insert a raw memory node directly into the database, bypassing the service
/// so we can control `created_at`.  Returns the UUID and registers it in the
/// fixture for cleanup.
async fn insert_raw_memory(
    fix: &mut TestFixture,
    content: &str,
    context: Option<&str>,
    created_at_offset_days: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    let ts = Utc::now() + Duration::days(created_at_offset_days);
    let metadata = serde_json::json!({
        "memory": true,
        "tags": [],
        "importance": 0.5,
        "context": context,
        "forgotten": false,
    });
    let content_hash = format!("hash-{id}");

    sqlx::query(
        "INSERT INTO covalence.nodes
             (id, node_type, source_type, title, content, content_hash, fingerprint,
              size_tokens, reliability, metadata, status, confidence, created_at)
         VALUES ($1, 'source', 'observation', 'memory', $2, $3, $3, 10, 0.6, $4, 'active', 0.6, $5)",
    )
    .bind(id)
    .bind(content)
    .bind(&content_hash)
    .bind(&metadata)
    .bind(ts)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("insert_raw_memory({content}) failed: {e}"));

    fix.track(id);
    id
}

// ─── since filter ─────────────────────────────────────────────────────────────

/// Memories created *before* the `since` threshold must be excluded; memories
/// created *on or after* the threshold must be included.
#[tokio::test]
#[serial]
async fn recall_since_excludes_old_memories() {
    let mut fix = TestFixture::new().await;
    let svc = MemoryService::new(fix.pool.clone());

    // Insert an "old" memory with created_at 30 days ago.
    insert_raw_memory(&mut fix, "ancient knowledge about distant stars", None, -30).await;

    // Insert a "new" memory created right now (offset = 0).
    let new_id = insert_raw_memory(&mut fix, "recent knowledge about distant stars", None, 0).await;

    // Recall with since = 7 days ago — should only match the new memory.
    let since = Utc::now() - Duration::days(7);
    let results = svc
        .recall(RecallRequest {
            query: "knowledge distant stars".to_string(),
            limit: 10,
            tags: vec![],
            min_confidence: None,
            since: Some(since),
            context_prefix: None,
        })
        .await
        .expect("recall should succeed");

    assert!(
        !results.is_empty(),
        "expected at least one result but got none"
    );
    let ids: Vec<_> = results.iter().map(|m| m.id).collect();
    assert!(
        ids.contains(&new_id),
        "new memory should be in results; got ids: {ids:?}"
    );
    for m in &results {
        assert!(
            m.created_at >= since,
            "memory {} has created_at {:?} which is before since {:?}",
            m.id,
            m.created_at,
            since
        );
    }

    fix.cleanup().await;
}

/// When `since` is `None`, both old and new memories should be returned.
#[tokio::test]
#[serial]
async fn recall_without_since_returns_all_matching() {
    let mut fix = TestFixture::new().await;
    let svc = MemoryService::new(fix.pool.clone());

    let old_id = insert_raw_memory(&mut fix, "ancient lore about cosmic nebulae", None, -60).await;

    let new_id = insert_raw_memory(&mut fix, "recent lore about cosmic nebulae", None, 0).await;

    let results = svc
        .recall(RecallRequest {
            query: "lore cosmic nebulae".to_string(),
            limit: 10,
            tags: vec![],
            min_confidence: None,
            since: None,
            context_prefix: None,
        })
        .await
        .expect("recall should succeed");

    let ids: Vec<_> = results.iter().map(|m| m.id).collect();
    assert!(
        ids.contains(&old_id),
        "old memory should appear when since=None; ids: {ids:?}"
    );
    assert!(
        ids.contains(&new_id),
        "new memory should appear when since=None; ids: {ids:?}"
    );

    fix.cleanup().await;
}

// ─── context_prefix filter ────────────────────────────────────────────────────

/// Memories whose context does NOT start with the prefix must be excluded;
/// memories that DO start with the prefix must be included.
#[tokio::test]
#[serial]
async fn recall_context_prefix_filters_correctly() {
    let mut fix = TestFixture::new().await;
    let svc = MemoryService::new(fix.pool.clone());

    // Memory with a matching context.
    let matching_id = insert_raw_memory(
        &mut fix,
        "session specific information about quantum physics",
        Some("session:main"),
        0,
    )
    .await;

    // Memory with an extended-but-still-matching context (prefix match).
    let extended_id = insert_raw_memory(
        &mut fix,
        "extended session information about quantum physics",
        Some("session:main:2026-03-02"),
        0,
    )
    .await;

    // Memory with a completely different context — must be excluded.
    insert_raw_memory(
        &mut fix,
        "other context information about quantum physics",
        Some("session:other"),
        0,
    )
    .await;

    // Memory with no context at all — must be excluded.
    insert_raw_memory(
        &mut fix,
        "no context information about quantum physics",
        None,
        0,
    )
    .await;

    let results = svc
        .recall(RecallRequest {
            query: "information quantum physics".to_string(),
            limit: 20,
            tags: vec![],
            min_confidence: None,
            since: None,
            context_prefix: Some("session:main".to_string()),
        })
        .await
        .expect("recall should succeed");

    assert!(
        !results.is_empty(),
        "expected at least one result but got none"
    );
    let ids: Vec<_> = results.iter().map(|m| m.id).collect();
    assert!(
        ids.contains(&matching_id),
        "exact-match memory should be returned; ids: {ids:?}"
    );
    assert!(
        ids.contains(&extended_id),
        "extended-prefix memory should be returned; ids: {ids:?}"
    );
    for m in &results {
        let ctx = m.context.as_deref().unwrap_or("");
        assert!(
            ctx.starts_with("session:main"),
            "memory {} has context {:?} which does not start with 'session:main'",
            m.id,
            m.context
        );
    }

    fix.cleanup().await;
}

/// When `context_prefix` is `None`, memories regardless of context should be returned.
#[tokio::test]
#[serial]
async fn recall_without_context_prefix_returns_all_matching() {
    let mut fix = TestFixture::new().await;
    let svc = MemoryService::new(fix.pool.clone());

    let id_a = insert_raw_memory(
        &mut fix,
        "gravitational waves research findings",
        Some("session:alpha"),
        0,
    )
    .await;

    let id_b = insert_raw_memory(
        &mut fix,
        "gravitational waves observatory data",
        Some("session:beta"),
        0,
    )
    .await;

    let id_none =
        insert_raw_memory(&mut fix, "gravitational waves theoretical models", None, 0).await;

    let results = svc
        .recall(RecallRequest {
            query: "gravitational waves".to_string(),
            limit: 20,
            tags: vec![],
            min_confidence: None,
            since: None,
            context_prefix: None,
        })
        .await
        .expect("recall should succeed");

    let ids: Vec<_> = results.iter().map(|m| m.id).collect();
    assert!(ids.contains(&id_a), "id_a should be returned; ids: {ids:?}");
    assert!(ids.contains(&id_b), "id_b should be returned; ids: {ids:?}");
    assert!(
        ids.contains(&id_none),
        "id_none should be returned; ids: {ids:?}"
    );

    fix.cleanup().await;
}

// ─── context populated in response ───────────────────────────────────────────

/// Verify that the `context` field is properly populated on the returned
/// `Memory` structs (Feature 3 — it was missing from the SELECT before).
#[tokio::test]
#[serial]
async fn recall_populates_context_field() {
    let mut fix = TestFixture::new().await;
    let svc = MemoryService::new(fix.pool.clone());

    let id = insert_raw_memory(
        &mut fix,
        "interstellar medium composition study",
        Some("session:science:2026"),
        0,
    )
    .await;

    let results = svc
        .recall(RecallRequest {
            query: "interstellar medium".to_string(),
            limit: 5,
            tags: vec![],
            min_confidence: None,
            since: None,
            context_prefix: None,
        })
        .await
        .expect("recall should succeed");

    let mem = results
        .iter()
        .find(|m| m.id == id)
        .expect("the inserted memory should appear in results");

    assert_eq!(
        mem.context.as_deref(),
        Some("session:science:2026"),
        "context field should be populated from the database"
    );

    fix.cleanup().await;
}

// ─── combined filters ─────────────────────────────────────────────────────────

/// Both `since` and `context_prefix` can be applied simultaneously.
#[tokio::test]
#[serial]
async fn recall_combined_since_and_context_prefix() {
    let mut fix = TestFixture::new().await;
    let svc = MemoryService::new(fix.pool.clone());

    // Old + right context → excluded (too old).
    insert_raw_memory(
        &mut fix,
        "stellar formation data from archives",
        Some("session:main"),
        -30,
    )
    .await;

    // New + wrong context → excluded (wrong prefix).
    insert_raw_memory(
        &mut fix,
        "stellar formation recent observations",
        Some("session:other"),
        0,
    )
    .await;

    // New + right context → included.
    let target_id = insert_raw_memory(
        &mut fix,
        "stellar formation current research",
        Some("session:main"),
        0,
    )
    .await;

    let since = Utc::now() - Duration::days(7);
    let results = svc
        .recall(RecallRequest {
            query: "stellar formation".to_string(),
            limit: 20,
            tags: vec![],
            min_confidence: None,
            since: Some(since),
            context_prefix: Some("session:main".to_string()),
        })
        .await
        .expect("recall should succeed");

    let ids: Vec<_> = results.iter().map(|m| m.id).collect();
    assert!(
        ids.contains(&target_id),
        "only the new+matching-context memory should be returned; ids: {ids:?}"
    );
    assert_eq!(
        ids.len(),
        1,
        "exactly one memory should match both filters; got {ids:?}"
    );

    fix.cleanup().await;
}
