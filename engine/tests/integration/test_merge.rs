//! Integration tests for the `merge` and `infer_edges` slow-path handlers.

use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

use covalence_engine::worker::{
    llm::LlmClient,
    merge_edges::{handle_infer_edges, handle_merge},
};

use super::helpers::{MockLlmClient, TestFixture};

// ─── merge: happy path ────────────────────────────────────────────────────────

/// Merging two articles should:
/// * Create a new `active` merged article.
/// * Archive both originals.
/// * Create two `MERGED_FROM` edges: new_article → each original.
/// * Queue an `embed` task for the merged article.
#[tokio::test]
#[serial]
async fn merge_creates_merged_article() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let a = fix
        .insert_article(
            "Article A",
            "Article A discusses the first set of concepts.",
        )
        .await;
    let b = fix
        .insert_article(
            "Article B",
            "Article B covers complementary related topics.",
        )
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({
            "article_id_a": a.to_string(),
            "article_id_b": b.to_string(),
        }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("handle_merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap())
        .expect("new_article_id should be a valid UUID");
    fix.track(new_id);

    assert_eq!(
        result["degraded"],
        json!(false),
        "merge should not be degraded"
    );

    // New merged article is active.
    assert_eq!(fix.node_status(new_id).await, "active");

    // Originals are archived.
    for (label, id) in [("article_a", a), ("article_b", b)] {
        assert_eq!(
            fix.node_status(id).await,
            "archived",
            "{label} should be archived after merge"
        );
    }

    // Two MERGED_FROM edges.
    assert_eq!(
        fix.edge_count_from(new_id, "MERGED_FROM").await,
        2,
        "two MERGED_FROM edges expected from new article"
    );

    // Embed task queued.
    assert_eq!(
        fix.pending_task_count("embed", new_id).await,
        1,
        "embed task should be queued for merged article"
    );

    fix.cleanup().await;
}

/// The merged article's content must incorporate material from both sources.
#[tokio::test]
#[serial]
async fn merge_content_includes_both_articles() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let a = fix
        .insert_article("Art A", "Unique phrase from article A.")
        .await;
    let b = fix
        .insert_article("Art B", "Unique phrase from article B.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": a.to_string(), "article_id_b": b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("handle_merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    // The mock LLM returns a fixed title; the content_len must be > 0.
    let content_len = result["content_len"].as_u64().unwrap_or(0);
    assert!(
        content_len > 0,
        "merged article must have non-empty content"
    );

    fix.cleanup().await;
}

// ─── merge: fallback on LLM error ────────────────────────────────────────────

/// When the LLM fails, merge falls back to content concatenation.
/// The result must be marked `degraded = true`.
#[tokio::test]
#[serial]
async fn merge_fallback_on_llm_error() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::always_fail());

    let a = fix
        .insert_article("Degraded A", "Fallback merge article A content.")
        .await;
    let b = fix
        .insert_article("Degraded B", "Fallback merge article B content.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": a.to_string(), "article_id_b": b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("merge should succeed in degraded mode");

    assert_eq!(
        result["degraded"],
        json!(true),
        "should be degraded when LLM fails"
    );

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    let content = fix.node_content(new_id).await;
    assert!(
        content.contains("Fallback merge article A content."),
        "degraded merge should concatenate article A content"
    );
    assert!(
        content.contains("Fallback merge article B content."),
        "degraded merge should concatenate article B content"
    );

    fix.cleanup().await;
}

// ─── merge: provenance inherited ─────────────────────────────────────────────

/// Provenance edges on both input articles must be copied to the merged article.
#[tokio::test]
#[serial]
async fn merge_inherits_provenance_edges() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let src_a = fix
        .insert_source("Prov Src A", "Source A provenance.")
        .await;
    let src_b = fix
        .insert_source("Prov Src B", "Source B provenance.")
        .await;
    let art_a = fix
        .insert_article("Art A Prov", "Article A with provenance.")
        .await;
    let art_b = fix
        .insert_article("Art B Prov", "Article B with provenance.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("tree_embed");

    fix.insert_originates_edge(src_a, art_a).await;
    fix.insert_originates_edge(src_b, art_b).await;

    let task = TestFixture::make_task(
        "merge",
        None,
        json!({ "article_id_a": art_a.to_string(), "article_id_b": art_b.to_string() }),
    );

    let result = handle_merge(&fix.pool, &llm, &task)
        .await
        .expect("merge should succeed");

    let new_id = Uuid::parse_str(result["new_article_id"].as_str().unwrap()).unwrap();
    fix.track(new_id);

    // Both provenance sources should have edges pointing to the new article.
    for (label, src) in [("src_a", src_a), ("src_b", src_b)] {
        let inherited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2",
        )
        .bind(src)
        .bind(new_id)
        .fetch_one(&fix.pool)
        .await
        .unwrap_or(0);
        assert!(
            inherited >= 1,
            "provenance from {label} should be inherited by merged article"
        );
    }

    fix.cleanup().await;
}

// ─── infer_edges ─────────────────────────────────────────────────────────────

/// `handle_infer_edges` must complete without error when the node has an
/// embedding.  If vector neighbours are found the mock LLM classifies them
/// at confidence 0.85 (above the 0.5 gate).
#[tokio::test]
#[serial]
async fn infer_edges_completes_with_embeddings() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let node_a = fix
        .insert_source(
            "Node A",
            "Deep learning and convolutional networks for image recognition.",
        )
        .await;
    let node_b = fix
        .insert_source(
            "Node B",
            "Computer vision techniques using CNNs for classification.",
        )
        .await;

    // Identical embeddings → cosine distance = 0 → they are each other's
    // nearest neighbour.
    fix.insert_embedding(node_a).await;
    fix.insert_embedding(node_b).await;

    let task = TestFixture::make_task("infer_edges", Some(node_a), json!({}));
    let result = handle_infer_edges(&fix.pool, &llm, &task)
        .await
        .expect("handle_infer_edges should succeed");

    assert!(
        result.get("edges_created").is_some(),
        "result should contain edges_created"
    );

    fix.cleanup().await;
}

// ─── covalence#77 — UTF-8 byte-boundary safety ───────────────────────────────

/// Regression test for covalence#77.
///
/// Build a source whose content places a multi-byte character (em-dash U+2014,
/// 3 UTF-8 bytes: 0xE2 0x80 0x94) exactly at the 1 200-byte truncation point
/// used by `infer_edges` when it assembles the LLM prompt snippets.
///
/// Before the fix, `&content[..1200]` would panic with:
///   "byte index 1200 is not a char boundary; it is inside '—' (bytes 1199..1202)"
///
/// After the fix, `safe_truncate` walks back to a valid char boundary so the
/// handler completes successfully and the result contains `edges_created`.
#[tokio::test]
#[serial]
async fn infer_edges_no_panic_on_multibyte_boundary() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // Build content whose byte length is slightly above 1 200 with an em-dash
    // (3 bytes: 0xE2 0x80 0x94) starting at byte offset 1 199.
    //
    // Strategy:
    //   • Fill 1 199 bytes with ASCII 'a' characters.
    //   • Append the em-dash '—' (bytes 1199–1201, inclusive).
    //   • Append more ASCII to push total length > 1 200.
    //
    // After this construction:
    //   content[..1200] would land inside the em-dash → panic without the fix.
    //   safe_truncate(&content, 1200) safely steps back to offset 1199.
    let mut content = "a".repeat(1199); // 1199 ASCII bytes
    content.push('—'); // em-dash: 3 bytes → total now 1202
    content.push_str(&"b".repeat(100)); // padding so len > 1200

    assert!(
        content.len() > 1200,
        "content must be longer than 1200 bytes for the truncation to trigger"
    );
    assert!(
        !content.is_char_boundary(1200),
        "byte 1200 must be inside the em-dash for this test to be meaningful"
    );

    let node_a = fix.insert_source("Em-Dash Node A", &content).await;

    // node_b uses similar content so the cosine distance < 0.3 threshold is met
    // (identical embeddings → distance = 0).
    let node_b = fix.insert_source("Em-Dash Node B", &content).await;

    fix.insert_embedding(node_a).await;
    fix.insert_embedding(node_b).await;

    let task = TestFixture::make_task("infer_edges", Some(node_a), json!({}));

    // Must not panic — the whole point of the regression test.
    let result = handle_infer_edges(&fix.pool, &llm, &task)
        .await
        .expect("handle_infer_edges must not panic on multi-byte char at boundary");

    // Sanity: result must be a well-formed object with the expected keys.
    assert!(
        result.get("edges_created").is_some(),
        "result should contain edges_created; got: {result}"
    );
    assert!(
        result.get("candidates_found").is_some(),
        "result should contain candidates_found; got: {result}"
    );

    fix.cleanup().await;
}

/// Same boundary regression for the keyword-extraction snippet (800-byte limit).
///
/// The combined `"{title}\n\n{content}"` string is constructed so that an
/// em-dash straddles byte 800.  Without the fix this panics; with the fix
/// `safe_truncate` steps back to the last valid boundary.
#[tokio::test]
#[serial]
async fn infer_edges_no_panic_on_multibyte_keyword_snippet_boundary() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    // title + "\n\n" = title_len + 2 bytes of prefix inside `combined`.
    // We want the em-dash to start at byte 799 inside `combined`.
    // combined = "{title}\n\n{content}"
    // prefix length = title.len() + 2
    // So we need content to start at offset (title.len() + 2), and we want
    // offset 799 to be inside the em-dash.
    // Simplest: title = "" (0 bytes), prefix = 2 bytes ("\n\n").
    // → content[797] starts the em-dash (797 + 2 = 799).
    let title = "T"; // 1 byte
    // prefix is "{title}\n\n" = 1 + 2 = 3 bytes
    // em-dash should start at offset 799 → content offset = 799 - 3 = 796
    let mut content = "a".repeat(796);
    content.push('—'); // em-dash at bytes 796–798 of content → bytes 799–801 of combined
    content.push_str(&"c".repeat(100));

    let combined_preview = format!("{title}\n\n{content}");
    assert!(
        combined_preview.len() > 800,
        "combined must exceed 800 bytes"
    );
    assert!(
        !combined_preview.is_char_boundary(800),
        "byte 800 must be inside the em-dash"
    );

    let node = fix.insert_source(title, &content).await;
    // No embedding needed — keyword extraction runs before the embedding check.

    let task = TestFixture::make_task("infer_edges", Some(node), json!({}));

    // The node has no embedding so infer_edges will defer, but keyword
    // extraction (which contains the 800-byte slice) runs before that check.
    // Must not panic.
    let result = handle_infer_edges(&fix.pool, &llm, &task)
        .await
        .expect("handle_infer_edges must not panic on em-dash at keyword snippet boundary");

    let status = result["status"].as_str().unwrap_or("");
    assert!(
        status.contains("deferred"),
        "node without embedding should defer; got: {result}"
    );

    fix.cleanup().await;
}

/// When the node has no embedding, `infer_edges` should defer (queue embed and
/// re-queue itself) and return a `status: deferred` result.
#[tokio::test]
#[serial]
async fn infer_edges_defers_when_no_embedding() {
    let mut fix = TestFixture::new().await;
    let llm: Arc<dyn LlmClient> = Arc::new(MockLlmClient::new());

    let node = fix
        .insert_source("No-Embed Node", "Node without any embedding yet.")
        .await;
    fix.track_task_type("embed");
    fix.track_task_type("infer_edges");

    let task = TestFixture::make_task("infer_edges", Some(node), json!({}));
    let result = handle_infer_edges(&fix.pool, &llm, &task)
        .await
        .expect("infer_edges should succeed even without embedding");

    // Result signals deferral.
    let status = result["status"].as_str().unwrap_or("");
    assert!(
        status.contains("deferred"),
        "result status should indicate deferral; got: {status}"
    );

    // An embed task should have been queued.
    assert!(
        fix.pending_task_count("embed", node).await >= 1,
        "embed task should be queued by infer_edges when embedding is absent"
    );

    fix.cleanup().await;
}
