//! Integration tests for Phase 1 of the session ingestion engine (covalence#40).
//!
//! Covers:
//! * `test_append_and_retrieve_messages` — POST /sessions, append 3 messages, GET messages
//! * `test_flush_creates_source`          — append, flush, verify source exists with transcript
//! * `test_flush_marks_messages_flushed`  — flush hides messages from unflushed view
//! * `test_finalize_closes_session`       — finalize sets status = "closed"

use serial_test::serial;

use covalence_engine::services::session_service::{
    AppendMessagesRequest, FinalizeRequest, MessageItem, SessionService,
};
use covalence_engine::services::source_service::SourceService;

use super::helpers::TestFixture;

// ─── helper ───────────────────────────────────────────────────────────────────

/// Create a session and return its id (tracked for cleanup via `nodes` is not
/// applicable here — sessions live in their own table; we rely on the per-test
/// TRUNCATE in `setup_pool` for cleanup).
async fn make_session(fix: &TestFixture, label: &str) -> uuid::Uuid {
    let svc = SessionService::new(fix.pool.clone());
    let session = svc
        .create(
            covalence_engine::services::session_service::CreateSessionRequest {
                label: Some(label.to_string()),
                metadata: serde_json::json!({}),
                platform: Some("test-platform".to_string()),
                channel: Some("test-channel".to_string()),
            },
        )
        .await
        .expect("session creation should succeed");
    session.id
}

fn three_messages() -> AppendMessagesRequest {
    AppendMessagesRequest {
        messages: vec![
            MessageItem {
                speaker: Some("Alice".to_string()),
                role: "user".to_string(),
                content: "Hello, how are you?".to_string(),
                chunk_index: None,
            },
            MessageItem {
                speaker: Some("Bot".to_string()),
                role: "assistant".to_string(),
                content: "I am doing well, thank you!".to_string(),
                chunk_index: None,
            },
            MessageItem {
                speaker: None,
                role: "system".to_string(),
                content: "Conversation ended.".to_string(),
                chunk_index: Some(0),
            },
        ],
    }
}

// ─── test 1: append and retrieve messages ────────────────────────────────────

/// Append 3 messages to a session and verify they can be retrieved with the
/// correct field values.
#[tokio::test]
#[serial]
async fn test_append_and_retrieve_messages() {
    let fix = TestFixture::new().await;
    let svc = SessionService::new(fix.pool.clone());

    let session_id = make_session(&fix, "ingestion-test-1").await;

    // Append 3 messages
    let appended = svc
        .append_messages(session_id, three_messages())
        .await
        .expect("append should succeed");

    assert_eq!(appended.len(), 3, "should have inserted 3 messages");

    // Retrieve (include_flushed=true so we see everything)
    let retrieved = svc
        .get_messages(session_id, true)
        .await
        .expect("get_messages should succeed");

    assert_eq!(retrieved.len(), 3, "should retrieve 3 messages");

    // Verify individual fields
    let first = &retrieved[0];
    assert_eq!(first.role, "user");
    assert_eq!(first.speaker.as_deref(), Some("Alice"));
    assert_eq!(first.content, "Hello, how are you?");
    assert!(first.flushed_at.is_none());

    let second = &retrieved[1];
    assert_eq!(second.role, "assistant");
    assert_eq!(second.speaker.as_deref(), Some("Bot"));

    let third = &retrieved[2];
    assert_eq!(third.role, "system");
    assert!(third.speaker.is_none());
    assert_eq!(third.chunk_index, Some(0));

    fix.cleanup().await;
}

// ─── test 2: flush creates a source ─────────────────────────────────────────

/// Appending 2 messages and flushing should produce a source node whose
/// content includes a recognisable transcript of the conversation.
#[tokio::test]
#[serial]
async fn test_flush_creates_source() {
    let fix = TestFixture::new().await;
    let svc = SessionService::new(fix.pool.clone());
    let source_svc = SourceService::new(fix.pool.clone());

    let session_id = make_session(&fix, "ingestion-test-2").await;

    // Append 2 messages
    svc.append_messages(
        session_id,
        AppendMessagesRequest {
            messages: vec![
                MessageItem {
                    speaker: Some("Tester".to_string()),
                    role: "user".to_string(),
                    content: "Tell me about covalence.".to_string(),
                    chunk_index: None,
                },
                MessageItem {
                    speaker: Some("Agent".to_string()),
                    role: "assistant".to_string(),
                    content: "Covalence is a knowledge substrate.".to_string(),
                    chunk_index: None,
                },
            ],
        },
    )
    .await
    .expect("append should succeed");

    // Flush
    let flush_result = svc
        .flush(session_id, &source_svc)
        .await
        .expect("flush should succeed");

    assert_eq!(
        flush_result.message_count, 2,
        "flush should report 2 messages"
    );

    // Verify the source was created and contains transcript content
    let source = source_svc
        .get(flush_result.source_id)
        .await
        .expect("get source should succeed");

    assert!(
        source.content.contains("covalence"),
        "transcript should contain message content"
    );
    assert!(
        source.content.contains("Tester"),
        "transcript should contain speaker name"
    );
    assert!(
        source.content.contains("Session:"),
        "transcript should have session header"
    );
    assert_eq!(
        source.source_type.as_deref(),
        Some("conversation"),
        "source_type should be 'conversation'"
    );

    fix.cleanup().await;
}

// ─── test 3: flush marks messages as flushed ─────────────────────────────────

/// After a flush, `get_messages(include_flushed=false)` should return 0 rows
/// and `get_messages(include_flushed=true)` should return rows with
/// `flushed_at` set.
#[tokio::test]
#[serial]
async fn test_flush_marks_messages_flushed() {
    let fix = TestFixture::new().await;
    let svc = SessionService::new(fix.pool.clone());
    let source_svc = SourceService::new(fix.pool.clone());

    let session_id = make_session(&fix, "ingestion-test-3").await;

    svc.append_messages(session_id, three_messages())
        .await
        .expect("append should succeed");

    svc.flush(session_id, &source_svc)
        .await
        .expect("flush should succeed");

    // Unflushed view: should be empty
    let unflushed = svc
        .get_messages(session_id, false)
        .await
        .expect("get_messages should succeed");
    assert_eq!(
        unflushed.len(),
        0,
        "no unflushed messages should remain after flush"
    );

    // All view: should have 3 messages, all with flushed_at set
    let all = svc
        .get_messages(session_id, true)
        .await
        .expect("get_messages should succeed");
    assert_eq!(all.len(), 3, "all 3 messages should be retrievable");
    for msg in &all {
        assert!(
            msg.flushed_at.is_some(),
            "message {} should have flushed_at set",
            msg.id
        );
    }

    fix.cleanup().await;
}

// ─── test 4: finalize closes the session ─────────────────────────────────────

/// Appending messages and then finalizing should flush the messages (creating a
/// source) and set the session status to "closed".
#[tokio::test]
#[serial]
async fn test_finalize_closes_session() {
    let fix = TestFixture::new().await;
    let svc = SessionService::new(fix.pool.clone());
    let source_svc = SourceService::new(fix.pool.clone());

    let session_id = make_session(&fix, "ingestion-test-4").await;

    svc.append_messages(session_id, three_messages())
        .await
        .expect("append should succeed");

    svc.finalize(
        session_id,
        FinalizeRequest {
            compile: Some(false),
        }
        .compile
        .unwrap_or(false),
        &source_svc,
    )
    .await
    .expect("finalize should succeed");

    // Verify session is now closed
    let session = svc
        .get(session_id)
        .await
        .expect("get should succeed")
        .expect("session should exist");

    assert_eq!(
        session.status, "closed",
        "session should be closed after finalize"
    );

    fix.cleanup().await;
}
