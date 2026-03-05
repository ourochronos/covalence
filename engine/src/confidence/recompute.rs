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

// ── Data struct ───────────────────────────────────────────────────────────────

/// Raw inputs for the three-phase confidence pipeline.
///
/// Fetched from `article_sources` by [`fetch_article_confidence_inputs`].
/// Passed to the pure [`compute_confidence`] function, which has no DB dependency.
pub struct ConfidenceInputs {
    /// Effective reliabilities of ORIGINATES/COMPILED_FROM sources (Phase 1A).
    /// Each value is `reliability × causal_weight × conf_link`, clamped to [0, 1].
    pub supporting: Vec<f64>,
    /// (trust_score, causal_weight) pairs for CONTRADICTS/CONTENDS sources (Phase 1B).
    pub attackers: Vec<(f64, f64)>,
    /// (trust_score, causal_weight) pairs for SUPERSEDES sources (Phase 1C).
    pub supersedes: Vec<(f64, f64)>,
}

// ── DB fetch ──────────────────────────────────────────────────────────────────

/// Fetch the raw confidence inputs for one article from `article_sources`.
///
/// Executes the three read queries (ORIGINATES, CONTRADICTS/CONTENDS, SUPERSEDES)
/// and returns a [`ConfidenceInputs`] struct ready for [`compute_confidence`].
///
/// # Errors
/// Returns any database error encountered during the queries.
pub async fn fetch_article_confidence_inputs(
    article_id: Uuid,
    conn: &mut PgConnection,
) -> anyhow::Result<ConfidenceInputs> {
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

    let mut supporting: Vec<f64> = Vec::with_capacity(orig_rows.len());

    for row in &orig_rows {
        use sqlx::Row;
        // reliability is DOUBLE PRECISION on nodes; causal_weight / conf_link are REAL on article_sources.
        // causal_weight and conf_link are NOT NULL; COALESCE in SQL is belt-and-suspenders.
        let rel: f64 = row.try_get("reliability").unwrap_or(0.5);
        let cw: f64 = row.try_get::<f32, _>("causal_weight")? as f64;
        let cl: f64 = row.try_get::<f32, _>("conf_link")? as f64;
        let eff = (rel * cw * cl).clamp(0.0, 1.0);
        supporting.push(eff);
    }

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

    let mut supersedes: Vec<(f64, f64)> = Vec::with_capacity(supersedes_rows.len());

    for row in &supersedes_rows {
        use sqlx::Row;
        let trust: f64 = row.try_get("trust_score").unwrap_or(0.5);
        // causal_weight is NOT NULL; COALESCE in SQL is belt-and-suspenders.
        // Read as f32 (REAL column) then widen to f64.
        let cw: f64 = row.try_get::<f32, _>("causal_weight")? as f64;
        supersedes.push((trust, cw));
    }

    Ok(ConfidenceInputs { supporting, attackers, supersedes })
}

// ── Pure computation ──────────────────────────────────────────────────────────

/// Run the three-phase confidence pipeline over pre-fetched inputs.
///
/// Applies Phase 1A (DS fusion), Phase 1B (DF-QuAD penalty), and Phase 1C
/// (SUPERSEDES decay) in sequence, then clamps the result to
/// `[CONF_FLOOR, 1.0]`.
///
/// Returns `(final_score, breakdown_json, flags)`.  No DB calls.  No async.
pub fn compute_confidence(inputs: &ConfidenceInputs) -> (f64, serde_json::Value, Vec<String>) {
    let mut flags: Vec<String> = Vec::new();

    if inputs.supporting.is_empty() {
        flags.push("no_sources".into());
    }

    // Phase 1A
    let conf_ds = ds_fusion(&inputs.supporting);

    // Phase 1B
    let conf_after_penalty = dfquad_penalty(conf_ds, &inputs.attackers);

    // Pre-compute total_attack for the breakdown JSON.
    let total_attack = if inputs.attackers.is_empty() {
        0.0_f64
    } else {
        let cp = inputs.attackers.iter().fold(1.0_f64, |acc, &(t, w)| {
            acc * (1.0 - (t * w).clamp(0.0, 1.0))
        });
        1.0 - cp
    };

    // Phase 1C
    let conf_after_decay = supersedes_decay(conf_after_penalty, &inputs.supersedes);

    let total_supersede = if inputs.supersedes.is_empty() {
        0.0_f64
    } else {
        let cp = inputs.supersedes.iter().fold(1.0_f64, |acc, &(t, w)| {
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
            "source_count": inputs.supporting.len(),
            "source_reliabilities": inputs.supporting,
            "raw_score": conf_ds,
        },
        "contradicts_penalty": {
            "attacker_count": inputs.attackers.len(),
            "total_attack": total_attack,
            "score_after": conf_after_penalty,
        },
        "supersedes_decay": {
            "superseder_count": inputs.supersedes.len(),
            "total_supersede": total_supersede,
            "score_after": conf_after_decay,
        },
        "final_score": final_score,
        "uncertainty_interval": [bel, pl],
        "flags": flags,
        "computed_at": computed_at.to_rfc3339(),
    });

    (final_score, breakdown, flags)
}

// ── Composed entry-point ──────────────────────────────────────────────────────

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
    // Capture source_ids for the back-fill loop before inputs is consumed.
    // We need to re-run the originates query to get source IDs — or we can
    // keep source_ids alongside supporting in fetch.  Since fetch doesn't
    // expose them, we run a lightweight re-fetch here (same as before).
    //
    // NOTE: to avoid a second round-trip we fetch inputs, then separately
    // collect source_ids from a targeted query below.  The SQL is identical
    // to what was here before the refactor; the only new thing is that the
    // three-phase math now lives in compute_confidence.
    let inputs = fetch_article_confidence_inputs(article_id, conn).await?;
    let (final_score, breakdown, flags) = compute_confidence(&inputs);
    let computed_at = Utc::now();

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
    //
    // We need (source_id, eff_reliability) pairs, which fetch_article_confidence_inputs
    // does not expose (it only keeps the numeric slice).  Re-run the narrow query
    // to get source IDs; this is the same SQL as before and is cheap (indexed).
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

    for row in &orig_rows {
        use sqlx::Row;
        let sid: Uuid = row.try_get("source_id")?;
        let rel: f64 = row.try_get("reliability").unwrap_or(0.5);
        let cw: f64 = row.try_get::<f32, _>("causal_weight")? as f64;
        let cl: f64 = row.try_get::<f32, _>("conf_link")? as f64;
        let eff = (rel * cw * cl).clamp(0.0, 1.0);

        sqlx::query(
            "UPDATE covalence.article_sources
             SET contribution_weight = $1
             WHERE article_id  = $2
               AND source_id   = $3
               AND relationship IN ('originates', 'compiled_from')",
        )
        .bind(eff as f32)
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure regression test: verifies that `compute_confidence` produces scores
    /// and flags consistent with the DS / DF-QuAD / supersedes-decay formulas.
    ///
    /// Hand-calculated expected values:
    ///
    /// Phase 1A (DS fusion):
    ///   supporting = [0.8, 0.6]
    ///   conf_ds = 1 − (1−0.8)(1−0.6) = 1 − 0.2×0.4 = 1 − 0.08 = 0.92
    ///
    /// Phase 1B (DF-QuAD):
    ///   attackers = [(0.5, 1.0)]
    ///   total_attack = 1 − (1 − 0.5×1.0) = 0.5
    ///   conf_after_penalty = 0.92 × (1 − 0.5) = 0.92 × 0.5 = 0.46
    ///
    /// Phase 1C (supersedes decay):
    ///   supersedes = [(0.4, 0.5)]
    ///   factor = 0.4×0.5 = 0.2 → total_supersede = 0.2
    ///   conf_after_decay = 0.46 × (1 − 0.2) = 0.46 × 0.8 = 0.368
    ///
    /// Final: 0.368 > CONF_FLOOR (0.02), so no floor_clamped flag.
    #[test]
    fn test_compute_confidence_matches_recompute() {
        let inputs = ConfidenceInputs {
            supporting: vec![0.8, 0.6],
            attackers: vec![(0.5, 1.0)],
            supersedes: vec![(0.4, 0.5)],
        };

        let (final_score, breakdown, flags) = compute_confidence(&inputs);

        // Final score
        let expected = 0.368_f64;
        assert!(
            (final_score - expected).abs() < 1e-9,
            "final_score={final_score}, expected≈{expected}"
        );

        // No floor clamp should have been triggered
        assert!(
            !flags.contains(&"floor_clamped".to_string()),
            "unexpected floor_clamped flag: {flags:?}"
        );
        assert!(
            !flags.contains(&"no_sources".to_string()),
            "unexpected no_sources flag: {flags:?}"
        );

        // Breakdown sanity checks
        let raw_score = breakdown["ds_fusion"]["raw_score"].as_f64().unwrap();
        assert!((raw_score - 0.92).abs() < 1e-9, "ds raw_score={raw_score}");

        let score_after_penalty = breakdown["contradicts_penalty"]["score_after"]
            .as_f64()
            .unwrap();
        assert!(
            (score_after_penalty - 0.46).abs() < 1e-9,
            "score_after_penalty={score_after_penalty}"
        );

        let score_after_decay = breakdown["supersedes_decay"]["score_after"]
            .as_f64()
            .unwrap();
        assert!(
            (score_after_decay - 0.368).abs() < 1e-9,
            "score_after_decay={score_after_decay}"
        );
    }

    /// Verify no_sources flag when supporting is empty.
    #[test]
    fn test_compute_confidence_no_sources_flag() {
        let inputs = ConfidenceInputs {
            supporting: vec![],
            attackers: vec![],
            supersedes: vec![],
        };
        let (final_score, _breakdown, flags) = compute_confidence(&inputs);
        assert!(
            flags.contains(&"no_sources".to_string()),
            "expected no_sources flag"
        );
        // DS fusion of empty → 0.0, which is < CONF_FLOOR → floor_clamped
        assert!(
            flags.contains(&"floor_clamped".to_string()),
            "expected floor_clamped flag for zero confidence"
        );
        assert!(
            (final_score - CONF_FLOOR).abs() < 1e-12,
            "expected CONF_FLOOR={CONF_FLOOR}, got {final_score}"
        );
    }

    /// Verify floor_clamped flag when the result would fall below CONF_FLOOR.
    #[test]
    fn test_compute_confidence_floor_clamp() {
        // A single very-low supporting source plus a strong attacker drives
        // conf_after_penalty very close to zero.
        let inputs = ConfidenceInputs {
            supporting: vec![0.01],
            attackers: vec![(0.99, 1.0)],
            supersedes: vec![],
        };
        let (final_score, _breakdown, flags) = compute_confidence(&inputs);
        assert!(
            flags.contains(&"floor_clamped".to_string()),
            "expected floor_clamped, got {flags:?}"
        );
        assert!(
            (final_score - CONF_FLOOR).abs() < 1e-12,
            "expected CONF_FLOOR={CONF_FLOOR}, got {final_score}"
        );
    }
}
