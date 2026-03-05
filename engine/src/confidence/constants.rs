//! Numeric constants for confidence propagation (covalence#137).

/// Hard lower bound applied after the full propagation pipeline.
/// Ensures no article ever reaches zero confidence (epistemic humility floor).
pub const CONF_FLOOR: f64 = 0.02;

// ── Test-only fixtures ────────────────────────────────────────────────────────
// These constants are reference values for unit tests, not production defaults.
// Edge weights in production are read from `covalence.article_sources.causal_weight`.
#[cfg(test)]
pub const TEST_CONTRADICTS_DEFAULT_WEIGHT: f64 = 1.0;

#[cfg(test)]
pub const TEST_CONTENDS_DEFAULT_WEIGHT: f64 = 0.3;
