//! Error classification and backoff computation for the retry queue.
//!
//! Classifies job failures as transient, rate-limited, or permanent
//! and computes appropriate backoff delays for each class.

/// How a failed job should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Transient error (timeout, connection reset) — normal backoff.
    Transient,
    /// Rate limit or quota exhaustion — long backoff (wait for reset).
    RateLimit,
    /// Permanent error (not found, bad payload) — dead immediately.
    Permanent,
}

/// Classify an error to determine retry strategy.
pub fn classify_error(err: &crate::error::Error) -> FailureClass {
    use crate::error::Error;
    match err {
        // Source deleted or missing → permanent, no point retrying.
        Error::NotFound { .. } => FailureClass::Permanent,
        // Bad queue payload → permanent.
        Error::Queue(msg) if msg.contains("missing") || msg.contains("invalid") => {
            FailureClass::Permanent
        }
        // Ingestion errors need string inspection for rate limits.
        Error::Ingestion(msg) => classify_ingestion_error(msg),
        // Everything else is transient.
        _ => FailureClass::Transient,
    }
}

/// Inspect ingestion error messages for rate limit / quota patterns.
fn classify_ingestion_error(msg: &str) -> FailureClass {
    let lower = msg.to_lowercase();
    if lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("quota")
        || lower.contains("402 payment required")
        || lower.contains("too many requests")
        || lower.contains("capacity")
        || lower.contains("exhausted")
    {
        FailureClass::RateLimit
    } else if lower.contains("not found")
        || lower.contains("404")
        || lower.contains("no raw content")
        // Poison pills: deterministic failures that won't succeed on retry.
        || lower.contains("json parse")
        || lower.contains("invalid json")
        || lower.contains("expected value")
        || lower.contains("schema validation")
        || lower.contains("context window")
        || lower.contains("context length exceeded")
        || lower.contains("maximum context")
        || lower.contains("token limit")
        || lower.contains("content too long")
        || lower.contains("invalid base64")
        || lower.contains("invalid utf-8")
    {
        FailureClass::Permanent
    } else {
        FailureClass::Transient
    }
}

/// Compute backoff delay in seconds based on failure class.
///
/// - `Transient`: exponential backoff `base * 2^(attempt-1)`, capped at `max`.
/// - `RateLimit`: starts at 15 minutes, doubles up to `max` (typically 1h).
/// - `Permanent`: returns 0 (will be sent to dead-letter immediately).
pub fn compute_backoff_for_class(
    class: FailureClass,
    base_secs: u64,
    attempt: i32,
    max_secs: u64,
) -> u64 {
    match class {
        FailureClass::Permanent => 0,
        FailureClass::RateLimit => {
            // Rate limits: start at 15 min, double each retry, cap at max.
            let rate_base = 900u64; // 15 minutes
            let exp = attempt.saturating_sub(1) as u32;
            rate_base
                .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX))
                .min(max_secs)
        }
        FailureClass::Transient => compute_backoff(base_secs, attempt, max_secs),
    }
}

/// Compute exponential backoff delay in seconds.
///
/// Formula: `base * 2^(attempt - 1)`, clamped to `max`.
pub fn compute_backoff(base_secs: u64, attempt: i32, max_secs: u64) -> u64 {
    let exp = attempt.saturating_sub(1) as u32;
    base_secs
        .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX))
        .min(max_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_first_attempt() {
        // attempt=1 -> base * 2^0 = base
        assert_eq!(compute_backoff(30, 1, 3600), 30);
    }

    #[test]
    fn backoff_second_attempt() {
        // attempt=2 -> base * 2^1 = 60
        assert_eq!(compute_backoff(30, 2, 3600), 60);
    }

    #[test]
    fn backoff_third_attempt() {
        // attempt=3 -> base * 2^2 = 120
        assert_eq!(compute_backoff(30, 3, 3600), 120);
    }

    #[test]
    fn backoff_clamped_to_max() {
        // attempt=10 -> 30 * 2^9 = 15360, clamped to 3600
        assert_eq!(compute_backoff(30, 10, 3600), 3600);
    }

    #[test]
    fn backoff_zero_attempt() {
        // attempt=0 shouldn't happen in practice, but defensively
        // saturating_sub(0, 1) = -1 as i32, cast to u32 = u32::MAX,
        // which overflows the shift → clamped to max_secs.
        assert_eq!(compute_backoff(30, 0, 3600), 3600);
    }

    #[test]
    fn backoff_large_attempt_no_overflow() {
        // attempt=100 -> should saturate, not panic
        let result = compute_backoff(30, 100, 3600);
        assert_eq!(result, 3600);
    }

    #[test]
    fn backoff_zero_base() {
        assert_eq!(compute_backoff(0, 5, 3600), 0);
    }

    #[test]
    fn backoff_zero_max() {
        assert_eq!(compute_backoff(30, 1, 0), 0);
    }

    #[test]
    fn backoff_progression() {
        let base = 10u64;
        let max = 10_000u64;
        let mut prev = 0u64;
        for attempt in 1..=8 {
            let delay = compute_backoff(base, attempt, max);
            assert!(
                delay >= prev,
                "backoff should be non-decreasing: attempt={attempt}, delay={delay}, prev={prev}"
            );
            prev = delay;
        }
    }

    // --- Error classification tests ---

    #[test]
    fn classify_not_found_is_permanent() {
        let err = crate::error::Error::NotFound {
            entity_type: "source",
            id: "abc".into(),
        };
        assert_eq!(classify_error(&err), FailureClass::Permanent);
    }

    #[test]
    fn classify_bad_payload_is_permanent() {
        let err = crate::error::Error::Queue("missing source_id in payload".into());
        assert_eq!(classify_error(&err), FailureClass::Permanent);
    }

    #[test]
    fn classify_rate_limit_402() {
        let err = crate::error::Error::Ingestion(
            "chat backend API returned 402 Payment Required: credits exhausted".into(),
        );
        assert_eq!(classify_error(&err), FailureClass::RateLimit);
    }

    #[test]
    fn classify_rate_limit_429() {
        let err = crate::error::Error::Ingestion(
            "Sorry, you've hit a rate limit that restricts the number of requests".into(),
        );
        assert_eq!(classify_error(&err), FailureClass::RateLimit);
    }

    #[test]
    fn classify_quota_exhausted() {
        let err = crate::error::Error::Ingestion(
            "TerminalQuotaError: You have exhausted your capacity on this model".into(),
        );
        assert_eq!(classify_error(&err), FailureClass::RateLimit);
    }

    #[test]
    fn classify_transient_timeout() {
        let err = crate::error::Error::Ingestion("connection timeout after 30s".into());
        assert_eq!(classify_error(&err), FailureClass::Transient);
    }

    #[test]
    fn classify_database_error_is_transient() {
        // Database errors (connection pool exhaustion, etc.) are transient.
        let err = crate::error::Error::Graph("connection refused".into());
        assert_eq!(classify_error(&err), FailureClass::Transient);
    }

    // --- Backoff-by-class tests ---

    #[test]
    fn rate_limit_backoff_starts_at_15_min() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::RateLimit, 30, 1, 7200),
            900
        );
    }

    #[test]
    fn rate_limit_backoff_doubles() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::RateLimit, 30, 2, 7200),
            1800
        );
    }

    #[test]
    fn rate_limit_backoff_capped() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::RateLimit, 30, 5, 3600),
            3600
        );
    }

    #[test]
    fn permanent_backoff_is_zero() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::Permanent, 30, 1, 3600),
            0
        );
    }

    #[test]
    fn transient_uses_normal_backoff() {
        assert_eq!(
            compute_backoff_for_class(FailureClass::Transient, 30, 1, 3600),
            30
        );
        assert_eq!(
            compute_backoff_for_class(FailureClass::Transient, 30, 3, 3600),
            120
        );
    }
}
