//! Query trace recording for search observability.
//!
//! Records per-query execution metadata: query text, strategy used,
//! per-dimension result counts, final result count, and wall-clock
//! execution time. Traces can be stored in a dedicated PG table for
//! offline analysis or logged via `tracing`.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::search::strategy::SearchStrategy;

/// A recorded trace of a single search execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryTrace {
    /// The original query text.
    pub query_text: String,
    /// The strategy that was used (after auto-selection, if any).
    pub strategy: String,
    /// Number of results returned per dimension.
    pub dimension_counts: HashMap<String, usize>,
    /// Total number of fused results before filtering/truncation.
    pub fused_count: usize,
    /// Final number of results returned to the caller.
    pub final_count: usize,
    /// Whether the query cache was hit.
    pub cache_hit: bool,
    /// Whether abstention was triggered.
    pub abstained: bool,
    /// Wall-clock execution time in milliseconds.
    pub execution_ms: u64,
    /// Number of entity nodes demoted in content-focused strategies.
    #[serde(default)]
    pub entities_demoted: usize,
    /// Per-result-type counts in the final result set.
    #[serde(default)]
    pub result_type_counts: HashMap<String, usize>,
}

impl QueryTrace {
    /// Create a new query trace builder.
    pub fn new(query_text: &str, strategy: &SearchStrategy) -> Self {
        let strategy_str = match strategy {
            SearchStrategy::Balanced => "balanced",
            SearchStrategy::Precise => "precise",
            SearchStrategy::Exploratory => "exploratory",
            SearchStrategy::Recent => "recent",
            SearchStrategy::GraphFirst => "graph_first",
            SearchStrategy::Global => "global",
            SearchStrategy::Custom(_) => "custom",
        };
        Self {
            query_text: query_text.to_string(),
            strategy: strategy_str.to_string(),
            dimension_counts: HashMap::new(),
            fused_count: 0,
            final_count: 0,
            cache_hit: false,
            abstained: false,
            execution_ms: 0,
            entities_demoted: 0,
            result_type_counts: HashMap::new(),
        }
    }

    /// Record dimension result count.
    pub fn record_dimension(&mut self, name: &str, count: usize) {
        self.dimension_counts.insert(name.to_string(), count);
    }

    /// Set the execution duration from a `Duration`.
    pub fn set_duration(&mut self, duration: Duration) {
        self.execution_ms = duration.as_millis() as u64;
    }

    /// Record a per-result-type count.
    pub fn record_result_type(&mut self, result_type: &str) {
        *self
            .result_type_counts
            .entry(result_type.to_string())
            .or_insert(0) += 1;
    }

    /// Log this trace via the `tracing` crate at info level.
    pub fn emit(&self) {
        tracing::info!(
            query = %self.query_text,
            strategy = %self.strategy,
            fused_count = self.fused_count,
            final_count = self.final_count,
            cache_hit = self.cache_hit,
            abstained = self.abstained,
            entities_demoted = self.entities_demoted,
            execution_ms = self.execution_ms,
            "search trace"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_new_defaults() {
        let trace = QueryTrace::new("test query", &SearchStrategy::Balanced);
        assert_eq!(trace.query_text, "test query");
        assert_eq!(trace.strategy, "balanced");
        assert!(trace.dimension_counts.is_empty());
        assert_eq!(trace.fused_count, 0);
        assert_eq!(trace.final_count, 0);
        assert!(!trace.cache_hit);
        assert!(!trace.abstained);
        assert_eq!(trace.execution_ms, 0);
    }

    #[test]
    fn trace_record_dimension() {
        let mut trace = QueryTrace::new("hello", &SearchStrategy::Precise);
        trace.record_dimension("vector", 15);
        trace.record_dimension("lexical", 8);
        assert_eq!(trace.dimension_counts.get("vector"), Some(&15));
        assert_eq!(trace.dimension_counts.get("lexical"), Some(&8));
        assert_eq!(trace.strategy, "precise");
    }

    #[test]
    fn trace_set_duration() {
        let mut trace = QueryTrace::new("q", &SearchStrategy::Global);
        trace.set_duration(Duration::from_millis(42));
        assert_eq!(trace.execution_ms, 42);
    }

    #[test]
    fn trace_serde_roundtrip() {
        let mut trace = QueryTrace::new("serde test", &SearchStrategy::Exploratory);
        trace.record_dimension("graph", 10);
        trace.fused_count = 25;
        trace.final_count = 10;
        trace.cache_hit = true;

        let json = serde_json::to_string(&trace).expect("serialize");
        let back: QueryTrace = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.query_text, "serde test");
        assert_eq!(back.strategy, "exploratory");
        assert_eq!(back.dimension_counts.get("graph"), Some(&10));
        assert_eq!(back.fused_count, 25);
        assert!(back.cache_hit);
    }

    #[test]
    fn trace_custom_strategy() {
        let custom = crate::search::strategy::DimensionWeights {
            vector: 0.5,
            lexical: 0.5,
            temporal: 0.0,
            graph: 0.0,
            structural: 0.0,
            global: 0.0,
        };
        let trace = QueryTrace::new("custom", &SearchStrategy::Custom(custom));
        assert_eq!(trace.strategy, "custom");
    }
}
