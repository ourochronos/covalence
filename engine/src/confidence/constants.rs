//! Numeric constants for confidence propagation (covalence#137).

/// Hard lower bound applied after the full propagation pipeline.
/// Ensures no article ever reaches zero confidence (epistemic humility floor).
pub const CONF_FLOOR: f64 = 0.02;

/// Default edge weight for CONTRADICTS attackers in DF-QuAD penalty.
pub const CONTRADICTS_DEFAULT_WEIGHT: f64 = 1.0;

/// Default edge weight for CONTENDS attackers in DF-QuAD penalty.
pub const CONTENDS_DEFAULT_WEIGHT: f64 = 0.3;
