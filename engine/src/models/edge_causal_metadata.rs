//! Data models for the `edge_causal_metadata` enrichment table (covalence#116).
//!
//! This table adds full Pearl-hierarchy causal semantics to any edge.  Rows
//! are optional — absence means "fall back to the raw `causal_weight` on the
//! edge itself".  When present, the three confidence fields enable a composite
//! causal score:
//!
//! ```text
//! score = causal_strength × direction_conf × (1 − hidden_conf_risk)
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

// =============================================================================
// Enum — Pearl hierarchy level
// =============================================================================

/// Pearl's three-rung causal hierarchy.
///
/// Stored as the Postgres enum `causal_level_enum`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[sqlx(type_name = "causal_level_enum", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CausalLevel {
    /// Rung 1 — purely observational correlation.
    Association,
    /// Rung 2 — effect of an action / do-calculus.
    Intervention,
    /// Rung 3 — counterfactual / imagined worlds.
    Counterfactual,
}

impl Default for CausalLevel {
    fn default() -> Self {
        Self::Association
    }
}

// =============================================================================
// Enum — Evidence type
// =============================================================================

/// How the causal claim was established.
///
/// Stored as the Postgres enum `causal_evidence_type_enum`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema,
)]
#[sqlx(type_name = "causal_evidence_type_enum", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CausalEvidenceType {
    /// Hard-coded schema prior (default for structural edges).
    StructuralPrior,
    /// Explicitly asserted by a domain expert.
    ExpertAssertion,
    /// Derived from statistical co-occurrence analysis.
    Statistical,
    /// Backed by a randomised / quasi-experimental study.
    Experimental,
    /// Detected via Granger-temporal precedence test.
    GrangerTemporal,
    /// Extracted by an LLM from unstructured text.
    LlmExtracted,
    /// Derived from a declarative domain rule.
    DomainRule,
}

impl Default for CausalEvidenceType {
    fn default() -> Self {
        Self::StructuralPrior
    }
}

// =============================================================================
// Row struct
// =============================================================================

/// A full row from `covalence.edge_causal_metadata`.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EdgeCausalMetadata {
    /// The edge this row enriches (also the primary key).
    pub edge_id: Uuid,
    /// Pearl hierarchy level (default: `association`).
    pub causal_level: CausalLevel,
    /// Estimated strength of the causal effect [0.0, 1.0].
    pub causal_strength: f64,
    /// How the causal claim was established.
    pub evidence_type: CausalEvidenceType,
    /// Confidence that the causal direction is correct [0.0, 1.0].
    pub direction_conf: f64,
    /// Estimated risk from unobserved confounders [0.0, 1.0].
    pub hidden_conf_risk: f64,
    /// Optional temporal delay between cause and effect (milliseconds).
    pub temporal_lag_ms: Option<i32>,
    /// Free-text annotation (added in migration 035, covalence#143).
    pub notes: Option<String>,
    /// Row creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Row last-update timestamp (managed by the DB trigger).
    pub updated_at: DateTime<Utc>,
}

// =============================================================================
// Patch payload (formerly EdgeCausalMetadataUpsert)
// =============================================================================

/// API / internal payload used to create or update a causal metadata row.
///
/// Renamed from `EdgeCausalMetadataUpsert` in migration 035 (covalence#143, #145).
/// All fields except `edge_id` are optional; when a field is `None` the stored
/// procedure `covalence.upsert_causal_metadata` preserves the existing database
/// value via `COALESCE`, fixing the silent-reset bug (#145).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeCausalMetadataPatch {
    /// The edge to enrich.
    pub edge_id: Uuid,
    /// Override the default `association` level.
    pub causal_level: Option<CausalLevel>,
    /// Override the default `0.5` strength.
    pub causal_strength: Option<f64>,
    /// Override the default `structural_prior` evidence type.
    pub evidence_type: Option<CausalEvidenceType>,
    /// Confidence that the causal direction is correct [0.0, 1.0].
    pub direction_conf: Option<f64>,
    /// Estimated risk from unobserved confounders [0.0, 1.0].
    pub hidden_conf_risk: Option<f64>,
    /// Optional temporal delay between cause and effect (milliseconds).
    pub temporal_lag_ms: Option<i32>,
    /// Optional free-text annotation.
    pub notes: Option<String>,
}

/// Backward-compatibility alias — new code should use [`EdgeCausalMetadataPatch`].
#[deprecated(since = "0.35.0", note = "Use EdgeCausalMetadataPatch instead")]
pub type EdgeCausalMetadataUpsert = EdgeCausalMetadataPatch;
