//! Epistemic model — confidence representation, propagation, and reasoning.
//!
//! This module implements the hybrid multi-stage epistemic pipeline:
//!
//! - **Stage 1**: Dempster-Shafer evidence fusion ([`fusion::dempster_shafer_combine`])
//! - **Stage 2**: Subjective Logic cumulative fusion ([`fusion::cumulative_fuse`])
//! - **Stage 3**: DF-QuAD contradiction handling ([`contradiction`])
//! - **Stage 4**: Temporal decay — supersedes/corrects ([`decay`])
//! - **Stage 5**: Global calibration (TrustRank — implemented in graph module)
//!
//! The [`convergence`] module ties stages 3-5 together with damped fixed-point
//! iteration to prevent epistemic oscillation.

pub mod confidence;
pub mod contradiction;
pub mod convergence;
pub mod decay;
pub mod delta;
pub mod forgetting;
pub mod fusion;
pub mod invalidation;
pub mod propagation;

pub use invalidation::{
    ConflictCheck, ConflictType, EdgeConflict, InvalidationAction, InvalidationReason,
};
pub use propagation::{
    ClaimAttack, ClaimInput, PropagationConfig, PropagationResult, propagate_confidence,
};
