//! Confidence propagation — Phase 1 (covalence#137).
//!
//! Three-phase pipeline:
//! * **Phase 1A** — Dempster-Shafer multi-source fusion over ORIGINATES links.
//! * **Phase 1B** — DF-QuAD contradiction penalty from CONTRADICTS / CONTENDS.
//! * **Phase 1C** — SUPERSEDES decay applied last.
//!
//! Public entry-point: [`recompute::recompute_article_confidence`].

pub mod constants;
pub mod dfquad_penalty;
pub mod ds_fusion;
pub mod recompute;
pub mod supersedes_decay;

pub use recompute::recompute_article_confidence;
pub use recompute::{ConfidenceInputs, compute_confidence, fetch_article_confidence_inputs};

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Structured result returned by [`recompute_article_confidence`].
#[derive(Debug, Clone)]
pub struct ConfidenceResult {
    /// The node (article or claim) whose confidence was recomputed.
    pub node_id: Uuid,
    /// Final clamped score in `[CONF_FLOOR, 1.0]` written to the article.
    pub final_score: f64,
    /// Full JSON breakdown persisted to `covalence.nodes.confidence_breakdown`.
    pub breakdown: serde_json::Value,
    /// Diagnostic flags (e.g. `"no_sources"`, `"floor_clamped"`).
    pub flags: Vec<String>,
    /// Timestamp of this computation run.
    pub computed_at: DateTime<Utc>,
}
