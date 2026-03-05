//! `recompute_article_confidence` — the three-phase pipeline (covalence#137).
//!
//! Runs Phase 1A (DS fusion), 1B (DF-QuAD penalty), and 1C (SUPERSEDES decay)
//! in sequence, writes the result to `covalence.nodes.confidence` and
//! `covalence.nodes.confidence_breakdown`, and back-fills
//! `covalence.article_sources.contribution_weight` for ORIGINATES rows.

use chrono::Utc;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    ConfidenceResult, constants::CONF_FLOOR, dfquad_penalty::dfquad_penalty, ds_fusion::ds_fusion,
    supersedes_decay::supersedes_decay,
};

/// Recompute the confidence score for a single article.
///
/// Reads provenance links from `covalence.article_sources` and applies:
/// * **Phase 1A** — Dempster-Shafer fusion over ORIGINATES sources.
/// * **Phase 1B** — DF-QuAD penalty from CONTRADICTS / CONTENDS sources.
/// * **Phase 1C** — SUPERSEDES decay from sources that supersede this article.
///
/// The final score is clamped to `[CONF_FLOOR, 1.0]` and written to
/// `covalence.nodes.confidence`.  A structured [`ConfidenceResult`] is
/// returned so callers can log or forward the breakdown.
///
/// # Errors
/// Returns any database error encountered during the read or write queries.
pub async fn recompute_article_confidence(
    article_id: Uuid,
    conn: &mut PgConnection,
) -> anyhow::Result<ConfidenceResult> {
    // ── Phase 1A: Dempster-Shafer multi-source fusion ─────────────────────────
    //
    // effective_reliability(S) = sources.reliability × article_sources.causal_weight
    //                            × article_sources.confidence
    let orig_rows = sqlx::query(
        "SELECT
             s.source_id,
             COALESCE(n.reliability, 0.5)  AS reliability,
             COALESCE(s.causal_weight, 1.0) AS causal_weight,
             COALESCE(s.confidence, 1.0)    AS conf_link
         FROM covalence.article_sources s
         JOIN covalence.nodes n ON n.id = s.source_id
         WHERE s.article_id  = $1
           AND s.relationship IN ('originates', 'compiled_from')
           AND s.superseded_at IS NULL",
    )
    .bind(article_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut source_ids: Vec<Uuid> = Vec::with_capacity(orig_rows.len());
    let mut source_eff_reliabilities: Vec<f64> = Vec::with_capacity(orig_rows.len());
    let mut flags: Vec<String> = Vec::new();

    for row in &orig_rows {
        use sqlx::Row;
        let sid: Uuid = row.try_get("source_id")?;
        // reliability is DOUBLE PRECISION on nodes; causal_weight / conf_link are REAL on article_sources.
        // causal_weight and conf_link are NOT NULL; COALESCE in SQL is belt-and-suspenders.
        let rel: f64 = row.try_get("reliability").unwrap_or(0.5);
        let cw: f64 = row.try_get::<f32, _>("causal_weight")? as f64;
        let cl: f64 = row.try_get::<f32, _>("conf_link")? as f64;
        let eff = (rel * cw * cl).clamp(0.0, 1.0);
        source_ids.push(sid);
        source_eff_reliabilities.push(eff);
    }

    if source_ids.is_empty() {
        flags.push("no_sources".into());
    }

    let conf_ds = ds_fusion(&source_eff_reliabilities);

    // ── Phase 1B: DF-QuAD contradiction penalty ───────────────────────────────
    //
    // attack_contribution(B→A) = trust_score(B) × w(B→A)
    // Use causal_weight from article_sources as edge weight when non-null.
    let attack_rows = sqlx::query(
        "SELECT
             COALESCE(n.confidence, 0.5) AS trust_score,
             s.causal_weight,
             s.relationship
         FROM covalence.article_sources s
         JOIN covalence.nodes n ON n.id = s.source_id
         WHERE s.article_id  = $1
           AND s.relationship IN ('contradicts', 'contends')
           AND s.superseded_at IS NULL",
    )
    .bind(article_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut attackers: Vec<(f64, f64)> = Vec::with_capacity(attack_rows.len());

    for row in &attack_rows {
        use sqlx::Row;
        let trust: f64 = row.try_get("trust_score").unwrap_or(0.5);
        // causal_weight is NOT NULL; read directly as f32 and widen to f64.
        let w: f64 = row.try_get::<f32, _>("causal_weight")? as f64;
        attackers.push((trust, w));
    }

    let conf_after_penalty = dfquad_penalty(conf_ds, &attackers);

    // Pre-compute total_attack for the breakdown JSON.
    let total_attack = if attackers.is_empty() {
        0.0_f64
    } else {
        let cp = attackers.iter().fold(1.0_f64, |acc, &(t, w)| {
            acc * (1.0 - (t * w).clamp(0.0, 1.0))
        });
        1.0 - cp
    };

    // ── Phase 1C: SUPERSEDES decay ────────────────────────────────────────────
    //
    // supersede_factor(B→A) = trust_score(B) × causal_weight (default=1.0)
    let supersedes_rows = sqlx::query(
        "SELECT
             COALESCE(n.confidence, 0.5)   AS trust_score,
             COALESCE(s.causal_weight, 1.0) AS causal_weight
         FROM covalence.article_sources s
         JOIN covalence.nodes n ON n.id = s.source_id
         WHERE s.article_id  = $1
           AND s.relationship = 'supersedes'
           AND s.superseded_at IS NULL",
    )
    .bind(article_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut supersedes_pairs: Vec<(f64, f64)> = Vec::with_capacity(supersedes_rows.len());

    for row in &supersedes_rows {
        use sqlx::Row;
        let trust: f64 = row.try_get("trust_score").unwrap_or(0.5);
        // causal_weight is NOT NULL; COALESCE in SQL is belt-and-suspenders.
        // Read as f32 (REAL column) then widen to f64.
        let cw: f64 = row.try_get::<f32, _>("causal_weight")? as f64;
        supersedes_pairs.push((trust, cw));
    }

    let conf_after_decay = supersedes_decay(conf_after_penalty, &supersedes_pairs);

    let total_supersede = if supersedes_pairs.is_empty() {
        0.0_f64
    } else {
        let cp = supersedes_pairs.iter().fold(1.0_f64, |acc, &(t, w)| {
            acc * (1.0 - (t * w).clamp(0.0, 1.0))
        });
        1.0 - cp
    };

    // ── Final: clamp to [CONF_FLOOR, 1.0] ────────────────────────────────────
    let was_clamped = conf_after_decay < CONF_FLOOR;
    let final_score = conf_after_decay.clamp(CONF_FLOOR, 1.0);

    if was_clamped {
        flags.push("floor_clamped".into());
    }

    let computed_at = Utc::now();

    // Uncertainty interval from DS theory: [Bel, Pl].
    // Bel = conf_ds (committed belief mass).
    // Pl  = Bel + (1 − Bel)  (belief + uncommitted mass — epistemic upper bound).
    let bel = conf_ds;
    // TODO: make configurable — in a richer DS model, Pl could be < 1.0 when
    // uncertainty mass is distributed across multiple hypotheses.
    let pl = 1.0_f64; // always 1.0 in classic two-valued DS

    // ── Build confidence_breakdown JSON ──────────────────────────────────────
    let breakdown = serde_json::json!({
        "ds_fusion": {
            "source_count": source_eff_reliabilities.len(),
            "source_reliabilities": source_eff_reliabilities,
            "raw_score": conf_ds,
        },
        "contradicts_penalty": {
            "attacker_count": attackers.len(),
            "total_attack": total_attack,
            "score_after": conf_after_penalty,
        },
        "supersedes_decay": {
            "superseder_count": supersedes_pairs.len(),
            "total_supersede": total_supersede,
            "score_after": conf_after_decay,
        },
        "final_score": final_score,
        "uncertainty_interval": [bel, pl],
        "flags": flags,
        "computed_at": computed_at.to_rfc3339(),
    });

    // ── Persist: write trust_score + breakdown to the article node ─────────
    sqlx::query(
        "UPDATE covalence.nodes
         SET confidence           = $1,
             confidence_breakdown = $2,
             modified_at          = now()
         WHERE id = $3",
    )
    .bind(final_score)
    .bind(&breakdown)
    .bind(article_id)
    .execute(&mut *conn)
    .await?;

    // ── Persist: back-fill contribution_weight on ORIGINATES rows ─────────
    for (sid, &eff_rel) in source_ids.iter().zip(source_eff_reliabilities.iter()) {
        sqlx::query(
            "UPDATE covalence.article_sources
             SET contribution_weight = $1
             WHERE article_id  = $2
               AND source_id   = $3
               AND relationship IN ('originates', 'compiled_from')",
        )
        .bind(eff_rel as f32)
        .bind(article_id)
        .bind(sid)
        .execute(&mut *conn)
        .await?;
    }

    Ok(ConfidenceResult {
        node_id: article_id,
        final_score,
        breakdown,
        flags,
        computed_at,
    })
}
