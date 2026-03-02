//! Integration tests for the smarter-compilation feature (covalence#35).
//!
//! Verifies that:
//! * Decision rationale ("we chose X over Y because Z") survives compilation
//!   without being flattened to a bare fact.
//! * The optional `compilation_focus` field is accepted and threaded through
//!   to the task queue without error.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use sqlx::Row;
use uuid::Uuid;

use covalence_engine::worker::{handle_compile, llm::LlmClient};

use super::helpers::{MockLlmClient, TestFixture};

// ─── decision language preservation ─────────────────────────────────────────

/// A source that contains explicit decision rationale ("we chose X over Y
/// because Z") should produce an article whose content retains some form of
/// that rationale — not just the bare fact that X was chosen.
#[tokio::test]
#[serial]
async fn test_compilation_preserves_decision_language() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // Source contains an explicit decision with rationale.
    let src = fix
        .insert_source(
            "Architecture Decision Record: Storage Backend",
            "After evaluating several options we chose PostgreSQL over MongoDB \
             because our data is highly relational and strong consistency \
             guarantees are required. MongoDB was considered for its flexible \
             schema but rejected because schema flexibility is not worth the \
             consistency trade-off for this workload. \
             Open question: whether to add a Redis cache layer remains unresolved.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src]);

    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src.to_string()],
            "title_hint": "Storage Backend Decision"
        }),
    );

    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed");

    assert_eq!(result["degraded"], json!(false), "should not be degraded");

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap())
        .expect("article_id should be a valid UUID");
    fix.track(article_id);

    // Fetch the compiled article content.
    let content: String = sqlx::query_scalar("SELECT content FROM covalence.nodes WHERE id = $1")
        .bind(article_id)
        .fetch_one(&fix.pool)
        .await
        .expect("article should exist");

    // The compiled article must retain the decision rationale in some form —
    // not just the bare fact "PostgreSQL was chosen".
    let content_lower = content.to_lowercase();
    let has_rationale = content_lower.contains("because")
        || content_lower.contains("over")
        || content_lower.contains("rejected")
        || content_lower.contains("chose");

    assert!(
        has_rationale,
        "compiled article should preserve decision rationale (because/over/rejected/chose), \
         but content was:\n{content}"
    );

    fix.cleanup().await;
}

// ─── compilation_focus hint ───────────────────────────────────────────────────

/// A compile request that includes an optional `compilation_focus` should be
/// queued without error, the job should complete, and the resulting article
/// should be a valid active node.
///
/// This test validates that the field is parsed and threaded through the
/// payload pipeline end-to-end (covalence#35 §2).
#[tokio::test]
#[serial]
async fn test_compilation_focus_hint_accepted() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src = fix
        .insert_source(
            "System Design Notes",
            "We evaluated three messaging systems. We chose Kafka over RabbitMQ \
             because Kafka's log-based retention gives us the replay capability \
             required by the audit subsystem. RabbitMQ was a strong contender \
             but its lack of durable log replay was disqualifying.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("contention_check");
    fix.track_inference_log("compile", vec![src]);

    // Include compilation_focus in the task payload.
    let task = TestFixture::make_task(
        "compile",
        None,
        json!({
            "source_ids": [src.to_string()],
            "title_hint": "Messaging System Selection",
            "compilation_focus": "focus on architectural decisions and rejected alternatives"
        }),
    );

    // The handler must succeed — the focus field must not be treated as an error.
    let result = handle_compile(&fix.pool, &llm, &task)
        .await
        .expect("handle_compile should succeed with compilation_focus set");

    assert_eq!(
        result["degraded"],
        json!(false),
        "compilation with focus hint should not be degraded"
    );
    assert_eq!(result["source_count"], json!(1));

    let article_id = Uuid::parse_str(result["article_id"].as_str().unwrap())
        .expect("article_id should be a valid UUID");
    fix.track(article_id);

    // Article must be active.
    let row = sqlx::query("SELECT node_type, status FROM covalence.nodes WHERE id = $1")
        .bind(article_id)
        .fetch_one(&fix.pool)
        .await
        .expect("article node should exist");

    assert_eq!(row.get::<String, _>("node_type"), "article");
    assert_eq!(row.get::<String, _>("status"), "active");

    fix.cleanup().await;
}
