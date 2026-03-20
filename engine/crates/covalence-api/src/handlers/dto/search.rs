//! Search-related DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Search delivery mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Return individual ranked results (default).
    #[default]
    Results,
    /// Assemble results into a deduplicated, budget-trimmed
    /// context window suitable for LLM generation.
    Context,
}

/// Content granularity for search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchGranularity {
    /// Walk up to the parent section chunk (default). For
    /// paragraph-level chunks, the content field is replaced
    /// with the parent section's content.
    #[default]
    Section,
    /// Use the matched chunk content as-is.
    Paragraph,
    /// Use the full source `normalized_content`.
    Source,
}

/// Custom per-dimension weight overrides for the `custom` search
/// strategy.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DimensionWeightsDto {
    /// Semantic vector similarity weight.
    pub vector: f64,
    /// Full-text lexical search weight.
    pub lexical: f64,
    /// Temporal recency/range weight.
    pub temporal: f64,
    /// Graph traversal weight.
    pub graph: f64,
    /// Structural centrality weight.
    pub structural: f64,
    /// Community summary search weight.
    pub global: f64,
}

/// Request body for search.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchRequest {
    /// The search query text.
    pub query: String,
    /// Search strategy (auto, balanced, precise, exploratory, recent,
    /// graph_first, global, custom). When `custom` is used, provide
    /// the `weights` object. Default: `auto` (SkewRoute selection).
    pub strategy: Option<String>,
    /// Custom dimension weights (used when `strategy` is `custom`).
    /// All six weights are required; they need not sum to 1.0
    /// (normalization is applied internally).
    pub weights: Option<DimensionWeightsDto>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Minimum epistemic confidence threshold (0.0–1.0).
    pub min_confidence: Option<f64>,
    /// Restrict to specific node types.
    pub node_types: Option<Vec<String>>,
    /// Restrict to specific entity classes: code, domain, actor, analysis.
    pub entity_classes: Option<Vec<String>>,
    /// Restrict to specific source types (e.g. "document", "code").
    pub source_types: Option<Vec<String>>,
    /// Restrict to specific source layers derived from URI prefix.
    /// Layers: "spec", "design", "code", "research", "external".
    pub source_layers: Option<Vec<String>>,
    /// Start of date range filter (ISO 8601).
    pub date_range_start: Option<String>,
    /// End of date range filter (ISO 8601).
    pub date_range_end: Option<String>,
    /// Delivery mode: `results` (default) or `context`.
    #[serde(default)]
    pub mode: SearchMode,
    /// Content granularity: `section` (default), `paragraph`,
    /// or `source`.
    #[serde(default)]
    pub granularity: SearchGranularity,
    /// Enable hierarchical (coarse-to-fine) search. Finds relevant
    /// sources first, then retrieves chunks only from those sources.
    #[serde(default)]
    pub hierarchical: bool,
    /// Orthogonal graph view restricting which edges the graph
    /// dimension traverses during BFS. Values: "causal",
    /// "temporal", "entity", "structural", "all". Default: all
    /// edges (bibliographic and synthetic excluded).
    pub graph_view: Option<String>,
}

/// A single fused search result.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResultResponse {
    /// Entity ID.
    pub id: Uuid,
    /// Fused RRF score.
    pub fused_score: f64,
    /// Epistemic confidence (projected probability).
    pub confidence: Option<f64>,
    /// Entity type (e.g. "node").
    pub entity_type: Option<String>,
    /// Canonical name of the entity.
    pub name: Option<String>,
    /// Best available text snippet.
    pub snippet: Option<String>,
    /// Full content of the matched entity. For chunks this is
    /// the chunk text (or parent section / full source depending
    /// on `granularity`). For articles it is the body. For nodes
    /// it is the description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Source URI (for chunk results).
    pub source_uri: Option<String>,
    /// Source title (for chunk results).
    pub source_title: Option<String>,
    /// Source type (e.g. "code", "document").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    /// Source domain (code/spec/design/research/external).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_domain: Option<String>,
    /// Per-dimension scores.
    pub dimension_scores: std::collections::HashMap<String, f64>,
    /// Per-dimension ranks.
    pub dimension_ranks: std::collections::HashMap<String, usize>,
    /// Related entities from the knowledge graph (1-hop neighbors).
    /// Present only for node-type results that have graph connections.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_context: Option<Vec<RelatedEntityResponse>>,
}

impl From<covalence_core::search::fusion::FusedResult> for SearchResultResponse {
    fn from(r: covalence_core::search::fusion::FusedResult) -> Self {
        let graph_context = r.graph_context.map(|gc| {
            gc.into_iter()
                .map(|re| RelatedEntityResponse {
                    name: re.name,
                    rel_type: re.rel_type,
                    direction: re.direction,
                })
                .collect()
        });
        Self {
            id: r.id,
            fused_score: r.fused_score,
            confidence: r.confidence,
            entity_type: r.entity_type,
            name: r.name,
            snippet: r.snippet,
            content: r.content,
            source_uri: r.source_uri,
            source_title: r.source_title,
            source_type: r.source_type,
            source_domain: r.source_domain,
            dimension_scores: r.dimension_scores,
            dimension_ranks: r.dimension_ranks,
            graph_context,
        }
    }
}

/// A related entity from the knowledge graph.
#[derive(Debug, Serialize, ToSchema)]
pub struct RelatedEntityResponse {
    /// Name of the related entity.
    pub name: String,
    /// Relationship type (e.g. "causes", "related_to").
    pub rel_type: String,
    /// Direction: "outgoing" or "incoming".
    pub direction: String,
}

/// A single item in an assembled context window.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContextItemResponse {
    /// 1-indexed reference number for citation.
    pub ref_number: usize,
    /// The content text.
    pub content: String,
    /// Source title for attribution.
    pub source_title: Option<String>,
    /// Source identifier for provenance.
    pub source_id: Option<String>,
    /// Relevance score.
    pub score: f64,
    /// Token count of this item.
    pub token_count: usize,
}

/// Response for context assembly mode.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContextResponse {
    /// Ordered context items with reference numbers.
    pub items: Vec<ContextItemResponse>,
    /// Total token count of assembled context.
    pub total_tokens: usize,
    /// Number of items dropped due to budget.
    pub items_dropped: usize,
    /// Number of duplicates removed.
    pub duplicates_removed: usize,
}

/// Unified search response supporting both result and context
/// delivery modes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub enum SearchApiResponse {
    /// Standard ranked results.
    Results(Vec<SearchResultResponse>),
    /// Assembled context window.
    Context(ContextResponse),
}

/// Request body for search feedback.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchFeedbackRequest {
    /// The query text that was searched.
    pub query: String,
    /// The result entity ID being rated.
    pub result_id: Uuid,
    /// Relevance rating (0.0 to 1.0).
    pub relevance: f64,
    /// Optional free-text comment.
    pub comment: Option<String>,
}

/// Response for a search trace entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchTraceResponse {
    /// Trace ID.
    pub id: Uuid,
    /// The query text.
    pub query_text: String,
    /// The search strategy used.
    pub strategy: String,
    /// Per-dimension result counts.
    pub dimension_counts: serde_json::Value,
    /// Total results returned.
    pub result_count: i32,
    /// Execution time in milliseconds.
    pub execution_ms: i32,
    /// When the trace was recorded.
    pub created_at: String,
}

/// Response for a trace replay.
#[derive(Debug, Serialize, ToSchema)]
pub struct TraceReplayResponse {
    /// The trace that was replayed.
    pub trace_id: Uuid,
    /// New search results from replay.
    pub results: Vec<SearchResultResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_mode_defaults_to_results() {
        let mode = SearchMode::default();
        assert!(matches!(mode, SearchMode::Results));
    }

    #[test]
    fn search_granularity_defaults_to_section() {
        let gran = SearchGranularity::default();
        assert!(matches!(gran, SearchGranularity::Section));
    }

    #[test]
    fn search_mode_serde_roundtrip() {
        let json = serde_json::to_string(&SearchMode::Context).unwrap();
        assert_eq!(json, "\"context\"");
        let mode: SearchMode = serde_json::from_str(&json).unwrap();
        assert!(matches!(mode, SearchMode::Context));
    }

    #[test]
    fn search_granularity_serde_roundtrip() {
        let json = serde_json::to_string(&SearchGranularity::Source).unwrap();
        assert_eq!(json, "\"source\"");
        let gran: SearchGranularity = serde_json::from_str(&json).unwrap();
        assert!(matches!(gran, SearchGranularity::Source));
    }

    #[test]
    fn search_request_defaults_mode_and_granularity() {
        // When mode and granularity are omitted from JSON, they
        // should default to Results and Section respectively.
        let json = serde_json::json!({
            "query": "test query"
        });
        let req: SearchRequest = serde_json::from_value(json).unwrap();
        assert!(matches!(req.mode, SearchMode::Results));
        assert!(matches!(req.granularity, SearchGranularity::Section));
    }

    #[test]
    fn search_request_with_mode_and_granularity() {
        let json = serde_json::json!({
            "query": "test query",
            "mode": "context",
            "granularity": "paragraph"
        });
        let req: SearchRequest = serde_json::from_value(json).unwrap();
        assert!(matches!(req.mode, SearchMode::Context));
        assert!(matches!(req.granularity, SearchGranularity::Paragraph));
    }

    #[test]
    fn search_result_response_omits_null_content() {
        let resp = SearchResultResponse {
            id: uuid::Uuid::new_v4(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: None,
            snippet: None,
            content: None,
            source_uri: None,
            source_title: None,
            source_type: None,
            source_domain: None,
            dimension_scores: std::collections::HashMap::new(),
            dimension_ranks: std::collections::HashMap::new(),
            graph_context: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        // content should be omitted when None due to
        // skip_serializing_if.
        assert!(json.get("content").is_none());
        // graph_context should also be omitted when None.
        assert!(json.get("graph_context").is_none());
    }

    #[test]
    fn search_result_response_includes_content_when_present() {
        let resp = SearchResultResponse {
            id: uuid::Uuid::new_v4(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: None,
            snippet: None,
            content: Some("full content here".to_string()),
            source_uri: None,
            source_title: None,
            source_type: None,
            source_domain: None,
            dimension_scores: std::collections::HashMap::new(),
            dimension_ranks: std::collections::HashMap::new(),
            graph_context: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["content"], "full content here");
    }

    #[test]
    fn search_result_response_includes_graph_context() {
        let resp = SearchResultResponse {
            id: uuid::Uuid::new_v4(),
            fused_score: 0.5,
            confidence: None,
            entity_type: Some("concept".to_string()),
            name: Some("GraphRAG".to_string()),
            snippet: None,
            content: None,
            source_uri: None,
            source_title: None,
            source_type: None,
            source_domain: None,
            dimension_scores: std::collections::HashMap::new(),
            dimension_ranks: std::collections::HashMap::new(),
            graph_context: Some(vec![
                RelatedEntityResponse {
                    name: "Knowledge Graph".to_string(),
                    rel_type: "related_to".to_string(),
                    direction: "outgoing".to_string(),
                },
                RelatedEntityResponse {
                    name: "RAG".to_string(),
                    rel_type: "extends".to_string(),
                    direction: "outgoing".to_string(),
                },
            ]),
        };
        let json = serde_json::to_value(&resp).unwrap();
        let gc = json.get("graph_context").unwrap().as_array().unwrap();
        assert_eq!(gc.len(), 2);
        assert_eq!(gc[0]["name"], "Knowledge Graph");
        assert_eq!(gc[0]["rel_type"], "related_to");
        assert_eq!(gc[0]["direction"], "outgoing");
    }
}
