//! Numeric constants for confidence propagation (covalence#137).

/// Hard lower bound applied after the full propagation pipeline.
/// Ensures no article ever reaches zero confidence (epistemic humility floor).
pub const CONF_FLOOR: f64 = 0.02;

// ── Test-only fixtures ────────────────────────────────────────────────────────
// These constants are reference values for integration tests, not production defaults.
// Edge weights in production are read from `covalence.article_sources.causal_weight`.
// Note: not gated by #[cfg(test)] because integration tests compile against the
// library in non-test mode and cannot access #[cfg(test)]-gated items.
pub const TEST_CONTRADICTS_DEFAULT_WEIGHT: f64 = 1.0;

pub const TEST_CONTENDS_DEFAULT_WEIGHT: f64 = 0.3;
