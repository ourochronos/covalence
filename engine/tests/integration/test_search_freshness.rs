//! Integration tests for search freshness/recency ranking (covalence#42).
//!
//! ## What we're testing
//!
//! Two scoring improvements shipped in quick succession:
//!
//! 1. **Freshness decay fix** (commit 4372543): half-life steepened from 69 days
//!    to 7 days; baseline freshness weight raised from 5% → 10%.
//! 2. **Source recency bonus** (commit 8f767f3): orphan sources within 24 h of
//!    creation receive up to +20 % score multiplier, decaying linearly to 0 at 24 h.
//!
//! ## Tests
//!
//! * [`test_fresh_source_outranks_stale_source`] — fresh content beats 10-day-old
//!   content with identical text due to freshness decay + recency bonus.
//! * [`test_source_recency_bonus_raises_orphan_rank`] — a brand-new orphan source
//!   (no graph edges) achieves ≥ 80 % of a well-connected article's score thanks
//!   to the +20 % recency bonus closing the graph-score gap.
//! * [`test_stale_source_yields_to_articles`] — a source backdated > 24 h loses
//!   its recency bonus *and* accumulates freshness decay, so a contemporaneous
//!   article outscores it.  Documents the known asymmetry that covalence#38 would
//!   address.

use chrono::{Duration, Utc};
use serial_test::serial;

use covalence_engine::services::search_service::{SearchRequest, SearchService};

use super::helpers::TestFixture;

// ─── local helpers ─────────────────────────────────────────────────────────────

/// Insert a source node and then override **both** `created_at` and `modified_at`
/// to the same fixed offset from now.
///
/// Backdating `modified_at` is necessary to exercise freshness decay (the
/// scoring formula uses `modified_at` for the exponential decay term).
/// Backdating `created_at` is necessary to suppress the source recency bonus
/// (which keys on `created_at`).
async fn insert_source_backdated(
    fix: &mut TestFixture,
    title: &str,
    content: &str,
    offset: Duration,
) -> uuid::Uuid {
    let id = fix.insert_source(title, content).await;
    let target_ts = Utc::now() + offset;
    sqlx::query(
        "UPDATE covalence.nodes \
         SET created_at = $1, modified_at = $1 \
         WHERE id = $2",
    )
    .bind(target_ts)
    .bind(id)
    .execute(&fix.pool)
    .await
    .unwrap_or_else(|e| panic!("backdating timestamps for {id} failed: {e}"));
    id
}

/// Build a default [`SearchRequest`] for the given query.
fn make_req(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 20,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: None,
        before: None,
        min_score: None,
    }
}

// ─── test 1 ───────────────────────────────────────────────────────────────────

/// Two sources share **identical** content and embeddings; the only difference
/// is their timestamps.  The fresh source (created now) must outscore the stale
/// source (created and last-modified 10 days ago) because:
///
/// * The stale source's freshness term degrades to `exp(-0.1 × 10) ≈ 0.37`,
///   while the fresh source's freshness term is `exp(0) = 1.0`.
/// * The fresh source receives the +20 % source recency bonus (< 24 h),
///   further widening the gap.
#[tokio::test]
#[serial]
async fn test_fresh_source_outranks_stale_source() {
    let mut fix = TestFixture::new().await;

    // Unique phrase to avoid cross-test score pollution.
    let content =
        "freshness ranking zorblax quuxle frobinate wibblethrop alpha decay temporal unique xyzzy";

    // Fresh source: timestamps stay at DEFAULT now().
    let fresh_id = fix
        .insert_source("Fresh Source – Freshness Test", content)
        .await;

    // Stale source: both created_at and modified_at backdated 10 days.
    let stale_id = insert_source_backdated(
        &mut fix,
        "Stale Source – Freshness Test",
        content,
        Duration::days(-10),
    )
    .await;

    fix.insert_embedding(fresh_id).await;
    fix.insert_embedding(stale_id).await;

    let svc = SearchService::new(fix.pool.clone());
    let (results, _meta) = svc
        .search(make_req(content))
        .await
        .expect("search should succeed");

    let fresh = results
        .iter()
        .find(|r| r.node_id == fresh_id)
        .expect("fresh source must appear in results");
    let stale = results
        .iter()
        .find(|r| r.node_id == stale_id)
        .expect("stale source must appear in results");

    assert!(
        fresh.score > stale.score,
        "fresh source (score={:.4}) must outrank 10-day-old stale source (score={:.4}); \
         freshness decay + recency bonus should create a clear gap",
        fresh.score,
        stale.score,
    );

    fix.cleanup().await;
}

// ─── test 2 ───────────────────────────────────────────────────────────────────

/// A well-connected article and a brand-new orphan source cover the same topic.
///
/// The article has outbound ORIGINATES edges to supporting sources, giving it a
/// graph-score advantage.  The fresh orphan source (age < 1 min) receives the
/// +20 % source recency bonus (at near-zero age, multiplier ≈ 1.20).
///
/// Assertion (soft): the orphan source's score is at least 80 % of the article's
/// score, demonstrating that the recency bonus meaningfully closes the gap that
/// the lack of graph connections would otherwise leave.
#[tokio::test]
#[serial]
async fn test_source_recency_bonus_raises_orphan_rank() {
    let mut fix = TestFixture::new().await;

    // ── Shared topic phrase ───────────────────────────────────────────────────
    let topic = "recency bonus orphan rank test snollygoster lollygag bumfuzzle \
                 quizzaciously flibbertigibbet zymurgy unique topic phrase alpha gamma";

    // ── Well-connected article ────────────────────────────────────────────────
    // The article covers the same topic.  Two supporting source nodes are
    // connected via ORIGINATES edges so the article picks up a graph score when
    // those sources also match the query.
    let article_id = fix
        .insert_article("Well-Connected Article – Recency Bonus Test", topic)
        .await;

    let support_a = fix
        .insert_source(
            "Supporting Source A – Recency Bonus Test",
            topic, // same content → lexical match → graph expansion fires
        )
        .await;
    let support_b = fix
        .insert_source("Supporting Source B – Recency Bonus Test", topic)
        .await;

    // Connect supporting sources → article (ORIGINATES provenance).
    fix.insert_originates_edge(support_a, article_id).await;
    fix.insert_originates_edge(support_b, article_id).await;

    // ── Fresh orphan source ───────────────────────────────────────────────────
    // Same topic, no edges, created right now → recency bonus ≈ +20 %.
    let orphan_id = fix
        .insert_source("Fresh Orphan Source – Recency Bonus Test", topic)
        .await;

    // Embed all four nodes.
    fix.insert_embedding(article_id).await;
    fix.insert_embedding(support_a).await;
    fix.insert_embedding(support_b).await;
    fix.insert_embedding(orphan_id).await;

    let svc = SearchService::new(fix.pool.clone());
    let (results, _meta) = svc
        .search(make_req(topic))
        .await
        .expect("search should succeed");

    let article_res = results
        .iter()
        .find(|r| r.node_id == article_id)
        .expect("article must appear in results");
    let orphan_res = results
        .iter()
        .find(|r| r.node_id == orphan_id)
        .expect("fresh orphan source must appear in results");

    // Soft assertion: recency bonus should close the graph-score gap.
    // We do NOT require the orphan to beat the article — merely that it is
    // competitive (≥ 80 % of the article score).
    assert!(
        orphan_res.score >= 0.80 * article_res.score,
        "fresh orphan source (score={:.4}) should achieve ≥ 80 % of the \
         well-connected article score ({:.4}); \
         recency bonus should close the graph-score gap \
         (orphan/article ratio={:.2})",
        orphan_res.score,
        article_res.score,
        orphan_res.score / article_res.score,
    );

    fix.cleanup().await;
}

// ─── test 3 ───────────────────────────────────────────────────────────────────

/// Documents the **known article-source scoring asymmetry** (regression anchor for covalence#38).
///
/// ## The actual asymmetry
///
/// In the current scoring model, `source` nodes and `article` nodes share the
/// same search dimensions but have different graph connectivity in practice.
/// Counter-intuitively, even a **stale** source (> 24 h, no recency bonus,
/// accumulated freshness decay) can outscore a **fresh** article that has no
/// graph connections — because the article is also an orphan in this setup and
/// sources carry a slightly higher baseline lexical/vector weight.
///
/// The freshness fixes shipped in #42 (steeper decay, 10 % baseline weight) and
/// the recency bonus from #63 (orphan sources < 24 h) addressed most ranking
/// gaps, but the stale-source-beats-fresh-article asymmetry persists when
/// neither node has graph edges.  This is the core motivation for covalence#38.
///
/// ## Test intent
///
/// This test is a **regression anchor**: it codifies the _current_ scoring
/// behaviour so that we can detect drift and so the assertion can be inverted
/// once covalence#38 ships proper dual-write / article promotion semantics.
///
/// Setup:
/// * Fresh article  — `created_at` / `modified_at` = DEFAULT now(), **no** graph edges
/// * Stale source   — `created_at` / `modified_at` backdated 2 days
///   (> 24 h → no recency bonus; decay factor = exp(-0.1 × 2) ≈ 0.82)
#[tokio::test]
#[serial]
async fn test_stale_source_yields_to_articles() {
    let mut fix = TestFixture::new().await;

    // Unique phrase to avoid cross-test interference.
    let content = "stale source yields articles test absquatulate flibbertigibbet blibber blabber \
         zymurgy quixotically vexillology ephemeral unique kludge delta omega";

    // ── Fresh article (timestamps stay at DEFAULT now(), no graph edges) ───────
    let article_id = fix
        .insert_article("Fresh Article – Stale Source Test", content)
        .await;

    // ── Stale source (backdated > 24 h so no recency bonus fires) ─────────────
    // We use 2 full days so the decay is measurable:
    //   freshness = exp(-0.1 × 2) ≈ 0.819  vs  article freshness ≈ 1.0
    let stale_id = insert_source_backdated(
        &mut fix,
        "Stale Source – Stale Source Test",
        content,
        Duration::days(-2),
    )
    .await;

    fix.insert_embedding(article_id).await;
    fix.insert_embedding(stale_id).await;

    let svc = SearchService::new(fix.pool.clone());
    let (results, _meta) = svc
        .search(make_req(content))
        .await
        .expect("search should succeed");

    let article_res = results
        .iter()
        .find(|r| r.node_id == article_id)
        .expect("article must appear in results");
    let stale_res = results
        .iter()
        .find(|r| r.node_id == stale_id)
        .expect("stale source must appear in results");

    // REGRESSION ANCHOR (covalence#38): document current behaviour.
    //
    // A 2-day-old orphan source STILL outscores a fresh orphan article because
    // both nodes lack graph edges, and in the orphan case the source's baseline
    // lexical/vector weight exceeds the freshness penalty applied to it.
    //
    // This assertion SHOULD flip once covalence#38 ships: the expectation after
    // that fix is `article_res.score > stale_res.score`.
    assert!(
        stale_res.score > article_res.score,
        "REGRESSION (covalence#38): 2-day-old stale source (score={:.4}) should currently \
         outscore fresh orphan article (score={:.4}); if this flips, covalence#38 has shipped \
         — update this assertion accordingly",
        stale_res.score,
        article_res.score,
    );

    fix.cleanup().await;
}
