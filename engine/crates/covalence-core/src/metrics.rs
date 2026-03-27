//! Prometheus metric name constants and recording helpers.
//!
//! Thin wrappers around the `metrics` facade crate. Metric
//! registration and recorder installation happen in the API binary
//! crate; this module simply defines names and convenience functions.

// ── Search ──────────────────────────────────────────────────────

/// Counter: total search queries processed.
pub const SEARCH_QUERIES: &str = "covalence_search_queries_total";

/// Histogram: search latency in seconds.
pub const SEARCH_LATENCY: &str = "covalence_search_latency_seconds";

/// Counter: search cache hits.
pub const SEARCH_CACHE_HITS: &str = "covalence_search_cache_hits_total";

/// Counter: search cache misses.
pub const SEARCH_CACHE_MISSES: &str = "covalence_search_cache_misses_total";

// ── Queue ───────────────────────────────────────────────────────

/// Counter: total queue jobs completed (success or failure).
pub const QUEUE_JOBS: &str = "covalence_queue_jobs_total";

/// Histogram: queue job duration in seconds.
pub const QUEUE_JOB_DURATION: &str = "covalence_queue_job_duration_seconds";

// ── LLM ─────────────────────────────────────────────────────────

/// Counter: total LLM calls (across all providers).
pub const LLM_CALLS: &str = "covalence_llm_calls_total";

/// Histogram: LLM call latency in seconds.
pub const LLM_LATENCY: &str = "covalence_llm_latency_seconds";

/// Counter: total LLM input (prompt) tokens consumed.
pub const LLM_INPUT_TOKENS: &str = "covalence_llm_input_tokens_total";

/// Counter: total LLM output (completion) tokens generated.
pub const LLM_OUTPUT_TOKENS: &str = "covalence_llm_output_tokens_total";

// ── Helpers ─────────────────────────────────────────────────────

/// Increment the search query counter, labelled by strategy.
pub fn record_search_query(strategy: &str) {
    metrics::counter!(SEARCH_QUERIES, "strategy" => strategy.to_string()).increment(1);
}

/// Record search latency in seconds, labelled by strategy.
pub fn record_search_latency(strategy: &str, seconds: f64) {
    metrics::histogram!(SEARCH_LATENCY, "strategy" => strategy.to_string()).record(seconds);
}

/// Increment the search cache hit counter.
pub fn record_cache_hit() {
    metrics::counter!(SEARCH_CACHE_HITS).increment(1);
}

/// Increment the search cache miss counter.
pub fn record_cache_miss() {
    metrics::counter!(SEARCH_CACHE_MISSES).increment(1);
}

/// Increment the queue job counter, labelled by kind and status.
pub fn record_queue_job(kind: &str, status: &str) {
    metrics::counter!(
        QUEUE_JOBS,
        "kind" => kind.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

/// Record queue job duration in seconds, labelled by kind.
pub fn record_queue_job_duration(kind: &str, seconds: f64) {
    metrics::histogram!(
        QUEUE_JOB_DURATION,
        "kind" => kind.to_string()
    )
    .record(seconds);
}

/// Increment the LLM call counter, labelled by provider.
pub fn record_llm_call(provider: &str) {
    metrics::counter!(LLM_CALLS, "provider" => provider.to_string()).increment(1);
}

/// Record LLM call latency in seconds, labelled by provider.
pub fn record_llm_latency(provider: &str, seconds: f64) {
    metrics::histogram!(LLM_LATENCY, "provider" => provider.to_string()).record(seconds);
}

/// Record LLM token usage, labelled by provider.
pub fn record_llm_tokens(provider: &str, input: u64, output: u64) {
    metrics::counter!(LLM_INPUT_TOKENS, "provider" => provider.to_string()).increment(input);
    metrics::counter!(LLM_OUTPUT_TOKENS, "provider" => provider.to_string()).increment(output);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_names_use_covalence_prefix() {
        assert!(SEARCH_QUERIES.starts_with("covalence_"));
        assert!(SEARCH_LATENCY.starts_with("covalence_"));
        assert!(SEARCH_CACHE_HITS.starts_with("covalence_"));
        assert!(SEARCH_CACHE_MISSES.starts_with("covalence_"));
        assert!(QUEUE_JOBS.starts_with("covalence_"));
        assert!(QUEUE_JOB_DURATION.starts_with("covalence_"));
        assert!(LLM_CALLS.starts_with("covalence_"));
        assert!(LLM_LATENCY.starts_with("covalence_"));
        assert!(LLM_INPUT_TOKENS.starts_with("covalence_"));
        assert!(LLM_OUTPUT_TOKENS.starts_with("covalence_"));
    }

    #[test]
    fn record_helpers_do_not_panic() {
        // Without a recorder installed, metrics calls are no-ops.
        // This test verifies the helpers don't panic in that case.
        record_search_query("balanced");
        record_search_latency("precise", 0.42);
        record_cache_hit();
        record_cache_miss();
        record_queue_job("process_source", "success");
        record_queue_job_duration("extract_chunk", 1.5);
        record_llm_call("claude(haiku)");
        record_llm_latency("gemini(2.5-flash)", 2.1);
        record_llm_tokens("http(gpt-4)", 150, 42);
    }
}
