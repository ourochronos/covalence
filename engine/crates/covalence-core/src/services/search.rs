//! Search service — multi-dimensional fused search orchestration.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use petgraph::visit::EdgeRef;

use crate::config::TableDimensions;
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::ingestion::embedder::Embedder;
use crate::search::abstention::{AbstentionConfig, check_abstention};
use crate::search::cache::{CacheConfig, QueryCache};
use crate::search::context::{AssembledContext, ContextConfig, RawContextItem, assemble_context};
use crate::search::dimensions::{
    GlobalDimension, GraphDimension, LexicalDimension, SearchDimension, SearchQuery,
    StructuralDimension, TemporalDimension, VectorDimension,
};
use crate::search::expansion::spreading_activation;
use crate::search::fusion::{self, FusedResult, RelatedEntity};
use crate::search::rerank::{NoopReranker, Reranker};
use crate::search::skewroute::{detect_intent, select_strategy};
use crate::search::strategy::SearchStrategy;
use crate::search::trace::QueryTrace;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{ArticleRepo, ChunkRepo, NodeRepo, SourceRepo};
use crate::types::ids::{ArticleId, ChunkId, NodeId};

/// Demotion factor applied to bare entity nodes in content-focused
/// search strategies. Entity nodes (e.g., "GraphRAG" with no content)
/// rank high on lexical + graph + structural dimensions but provide
/// no useful content to the user. This factor pushes them below
/// chunks and articles without removing them entirely.
const ENTITY_DEMOTION_FACTOR: f64 = 0.3;

/// Maximum length for derived chunk names.
const MAX_CHUNK_NAME_LEN: usize = 80;

/// Generic section headings that benefit from source-title
/// qualification (e.g., "Overview" → "Paper Title: Overview").
const GENERIC_HEADINGS: &[&str] = &[
    "overview",
    "introduction",
    "abstract",
    "summary",
    "background",
    "conclusion",
    "conclusions",
    "discussion",
    "methods",
    "methodology",
    "results",
    "implementation",
    "architecture",
    "design",
    "analysis",
    "evaluation",
    "related work",
    "future work",
    "appendix",
    "references",
    "acknowledgments",
    "prerequisites",
    "setup",
    "configuration",
    "usage",
    "examples",
    "getting started",
    "installation",
    "motivation",
];

/// Truncate a string to at most `max_bytes` bytes, snapping backward
/// to a valid UTF-8 character boundary. Appends `"..."` if truncated.
///
/// This prevents panics from slicing multi-byte characters (emoji,
/// CJK, accented characters) at arbitrary byte positions.
fn truncate_with_ellipsis(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Extract a keyword-in-context (KWIC) snippet from content.
///
/// Finds the best window of text surrounding query terms. Falls back
/// to `truncate_with_ellipsis` if no query terms appear in the content.
fn kwic_snippet(content: &str, query: &str, window_bytes: usize) -> String {
    let content_lower = content.to_lowercase();
    let terms: Vec<&str> = query
        .split_whitespace()
        .filter(|t| t.len() >= 3)
        .collect();

    if terms.is_empty() {
        return truncate_with_ellipsis(content, window_bytes);
    }

    // Find the position of the best term match (longest term first
    // for more specific hits).
    let mut best_pos = None;
    let mut best_len = 0;
    for term in &terms {
        let term_lower = term.to_lowercase();
        if let Some(pos) = content_lower.find(&term_lower) {
            if term_lower.len() > best_len {
                best_pos = Some(pos);
                best_len = term_lower.len();
            }
        }
    }

    let Some(match_pos) = best_pos else {
        return truncate_with_ellipsis(content, window_bytes);
    };

    // Center the window around the match.
    let half = window_bytes / 2;
    let mut start = match_pos.saturating_sub(half);
    // Walk to a char boundary.
    while start > 0 && !content.is_char_boundary(start) {
        start -= 1;
    }
    // Snap forward to a word boundary (avoid starting mid-word).
    if start > 0 {
        if let Some(space) = content[start..].find(|c: char| c.is_whitespace()) {
            let candidate = start + space + 1;
            // Only snap forward if we don't skip too far (< 30 chars).
            if candidate < content.len() && candidate - start < 30 {
                start = candidate;
            }
        }
    }

    let mut end = (start + window_bytes).min(content.len());
    while end < content.len() && !content.is_char_boundary(end) {
        end += 1;
    }
    // Snap backward to a word boundary (avoid ending mid-word).
    if end < content.len() {
        if let Some(space) = content[..end].rfind(|c: char| c.is_whitespace()) {
            // Only snap back if we don't lose too much (< 30 chars).
            if end - space < 30 {
                end = space;
            }
        }
    }

    let slice = &content[start..end];
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < content.len() { "..." } else { "" };
    format!("{prefix}{slice}{suffix}")
}

/// Derive a readable name from chunk content, optionally qualified
/// by a source title when the heading is generic.
///
/// Strategy:
/// 1. Skip leading lines that are metadata (bold labels, links, refs).
/// 2. If a Markdown heading (`# ...`) is found, use it (qualify with
///    source title if the heading is generic like "Overview").
/// 3. Otherwise, take the first meaningful sentence, truncated to
///    [`MAX_CHUNK_NAME_LEN`].
#[cfg(test)]
fn derive_chunk_name(content: &str) -> String {
    derive_chunk_name_qualified(content, None)
}

/// Derive a chunk name, qualifying generic headings with the
/// source title when available.
fn derive_chunk_name_qualified(content: &str, source_title: Option<&str>) -> String {
    let trimmed = content.trim();

    // Try to find the first meaningful line: skip bold labels
    // (`**Key:** ...`), link-only lines (`[ref](url)`), and very
    // short lines (< 10 chars) that are usually metadata.
    let meaningful = trimmed.lines().find(|line| {
        let l = line.trim();
        if l.is_empty() {
            return false;
        }
        // Skip bold-label lines: **Label:** value
        if l.starts_with("**") && l.contains(":**") {
            return false;
        }
        // Skip bare markdown links: [text](url)
        if l.starts_with('[') && l.contains("](") && l.len() < 100 {
            return false;
        }
        // Skip arxiv-style references: [2506.12345]
        if l.starts_with('[')
            && l.len() < 20
            && l.chars().skip(1).take(4).all(|c| c.is_ascii_digit())
        {
            return false;
        }
        // Skip bare numbered list items: "3.", "4.", "1)"
        if l.len() < 5
            && l.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ')')
        {
            return false;
        }
        true
    });

    // If no meaningful line was found (all lines were metadata),
    // use the first line as a last resort, stripping bold markers.
    let meaningful = match meaningful {
        Some(line) => line.trim(),
        None => {
            let first = trimmed.lines().next().unwrap_or(trimmed).trim();
            // Strip bold labels for display: "**Authors:** X" → "Authors: X"
            let cleaned = strip_inline_markdown(first);
            let cleaned = cleaned.trim();
            if cleaned.is_empty() {
                return String::new();
            }
            // Truncate and return directly — skip heading logic.
            return truncate_with_ellipsis(cleaned, MAX_CHUNK_NAME_LEN);
        }
    };

    if meaningful.is_empty() {
        return String::new();
    }

    // Check for Markdown heading.
    if let Some(heading) = meaningful.strip_prefix('#') {
        let heading = heading.trim_start_matches('#').trim();
        let heading = if let Some(end) = heading.find('\n') {
            heading[..end].trim()
        } else {
            heading
        };
        return qualify_heading(heading, source_title);
    }

    // Strip inline markdown formatting: **bold**, *italic*, `code`.
    let clean = strip_inline_markdown(meaningful);
    let clean = clean.trim();

    // First sentence: up to the first sentence-ending punctuation or
    // newline, whichever comes first.
    let sentence_end = clean
        .find(['.', '!', '?', '\n'])
        .map(|i| i + 1) // include the punctuation
        .unwrap_or(clean.len());

    if sentence_end <= MAX_CHUNK_NAME_LEN {
        let name = clean[..sentence_end].trim();
        if name.len() < clean.len() && !name.ends_with('.') {
            format!("{name}...")
        } else {
            name.to_string()
        }
    } else {
        truncate_with_ellipsis(clean, MAX_CHUNK_NAME_LEN)
    }
}

/// Qualify a heading with the source title when the heading is
/// generic (e.g., "Overview" → "Paper Title: Overview").
fn qualify_heading(heading: &str, source_title: Option<&str>) -> String {
    // Strip leading section numbers like "2 ", "1.2 ", "3.1.4. "
    // so "2 Background" matches the generic heading "background".
    let bare = strip_section_number(heading);
    let is_generic = GENERIC_HEADINGS
        .iter()
        .any(|g| bare.eq_ignore_ascii_case(g));

    if is_generic {
        if let Some(title) = source_title {
            let title = title.trim();
            if !title.is_empty() {
                // Truncate source title to leave room for heading.
                let max_title = MAX_CHUNK_NAME_LEN
                    .saturating_sub(heading.len())
                    .saturating_sub(2); // ": "
                let t = truncate_with_ellipsis(title, max_title);
                return format!("{}: {}", t, heading);
            }
        }
    }

    truncate_with_ellipsis(heading, MAX_CHUNK_NAME_LEN)
}

/// Strip a leading section number prefix from a heading.
///
/// Handles patterns like "2 Background", "1.2 Methods",
/// "3.1.4. Results", and "A.1 Appendix". Returns the heading
/// text after the number prefix, or the original string if no
/// prefix is found.
fn strip_section_number(heading: &str) -> &str {
    let s = heading.trim();
    // Find where the numeric prefix ends. Allow digits, dots,
    // and a single trailing dot (e.g., "2.", "3.1.4.").
    let prefix_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(0);
    if prefix_end == 0 {
        return s;
    }
    // The character after the prefix must be whitespace.
    let rest = &s[prefix_end..];
    let trimmed = rest.trim_start();
    if trimmed.is_empty() || rest.len() == trimmed.len() {
        // No space after prefix — not a section number.
        return s;
    }
    trimmed
}

/// Strip basic inline markdown: `**bold**`, `*italic*`, `` `code` ``.
fn strip_inline_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            // Skip **
            i += 2;
        } else if chars[i] == '*' || chars[i] == '`' {
            // Skip single * or `
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Post-fusion filters for narrowing search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilters {
    /// Minimum epistemic confidence (projected probability).
    pub min_confidence: Option<f64>,
    /// Restrict to specific node types.
    pub node_types: Option<Vec<String>>,
    /// Restrict to a temporal date range.
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
}

/// Service for orchestrating multi-dimensional search and RRF fusion.
///
/// Integrates the full search pipeline: cache lookup, adaptive
/// strategy selection, dimension search, RRF fusion, abstention
/// detection, query expansion, reranking, and trace recording.
pub struct SearchService {
    repo: Arc<PgRepo>,
    embedder: Option<Arc<dyn Embedder>>,
    graph: SharedGraph,
    vector: VectorDimension,
    lexical: LexicalDimension,
    temporal: TemporalDimension,
    graph_dim: GraphDimension,
    structural: StructuralDimension,
    global: GlobalDimension,
    reranker: Arc<dyn Reranker>,
    cache: Option<QueryCache>,
    abstention_config: AbstentionConfig,
    /// Use Convex Combination fusion instead of RRF.
    /// CC preserves score magnitude; RRF uses only rank.
    use_cc_fusion: bool,
}

impl SearchService {
    /// Create a new search service with default table dimensions.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self::with_embedder(repo, graph, None)
    }

    /// Create a new search service with an optional embedder for
    /// vector search.
    pub fn with_embedder(
        repo: Arc<PgRepo>,
        graph: SharedGraph,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        Self::with_config(repo, graph, embedder, TableDimensions::default())
    }

    /// Create a new search service with per-table embedding
    /// dimensions.
    pub fn with_config(
        repo: Arc<PgRepo>,
        graph: SharedGraph,
        embedder: Option<Arc<dyn Embedder>>,
        table_dims: TableDimensions,
    ) -> Self {
        let pool = repo.pool().clone();
        let graph_clone = Arc::clone(&graph);
        Self {
            repo,
            embedder,
            vector: VectorDimension::new(pool.clone(), table_dims.clone()),
            lexical: LexicalDimension::new(pool.clone()),
            temporal: TemporalDimension::new(pool.clone()),
            graph_dim: GraphDimension::new(Arc::clone(&graph)),
            structural: StructuralDimension::new(Arc::clone(&graph)),
            global: GlobalDimension::new(pool.clone(), table_dims),
            graph: graph_clone,
            reranker: Arc::new(NoopReranker),
            cache: None,
            abstention_config: AbstentionConfig::default(),
            use_cc_fusion: true,
        }
    }

    /// Use Convex Combination fusion instead of RRF.
    ///
    /// CC preserves score magnitude information that RRF discards
    /// by normalizing scores within each dimension to \[0, 1\] and
    /// computing a weighted sum. Bruch et al. (2210.11934) showed
    /// CC consistently outperforms RRF in hybrid retrieval.
    pub fn with_cc_fusion(mut self, enabled: bool) -> Self {
        self.use_cc_fusion = enabled;
        self
    }

    /// Enable the semantic query cache with the given config.
    ///
    /// When enabled, semantically similar queries within the TTL
    /// window will return cached results instead of re-executing
    /// the full search pipeline.
    pub fn with_cache(mut self, config: CacheConfig) -> Self {
        let pool = self.repo.pool().clone();
        self.cache = Some(QueryCache::new(pool, config));
        self
    }

    /// Set a custom reranker implementation.
    ///
    /// By default, `NoopReranker` is used which preserves the
    /// original RRF ordering. Provide a cross-encoder reranker
    /// (e.g. Voyage rerank-2.5) for improved result relevance.
    pub fn with_reranker(mut self, reranker: Arc<dyn Reranker>) -> Self {
        self.reranker = reranker;
        self
    }

    /// Set a custom abstention config.
    pub fn with_abstention_config(mut self, config: AbstentionConfig) -> Self {
        self.abstention_config = config;
        self
    }

    /// Execute a fused search across all dimensions.
    ///
    /// Pipeline:
    /// 1. Check semantic query cache (if enabled)
    /// 2. Embed the query for vector search
    /// 3. Auto-select strategy if not specified (SkewRoute)
    /// 4. Run all 6 dimensions concurrently
    /// 5. RRF fusion
    /// 6. Abstention detection
    /// 7. Query expansion (spreading activation from top-K)
    /// 8. Enrichment (node/article/chunk metadata)
    /// 9. Reranking
    /// 10. Post-fusion filtering and truncation
    /// 11. Cache population + trace recording
    pub async fn search(
        &self,
        query: &str,
        strategy: SearchStrategy,
        limit: usize,
        filters: Option<SearchFilters>,
    ) -> Result<Vec<FusedResult>> {
        let start = Instant::now();
        let time_range = filters.as_ref().and_then(|f| f.date_range);

        // --- Step 1: Embed the query ---
        let query_embedding = if let Some(ref embedder) = self.embedder {
            match embedder.embed(&[query.to_string()]).await {
                Ok(mut vecs) if !vecs.is_empty() => Some(vecs.remove(0)),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to embed query, \
                         vector search disabled"
                    );
                    None
                }
            }
        } else {
            None
        };

        tracing::debug!(
            has_embedding = query_embedding.is_some(),
            embedding_dim = query_embedding.as_ref().map(|e| e.len()),
            "query embedding status"
        );

        // --- Step 2: Cache lookup ---
        if let (Some(cache), Some(emb)) = (&self.cache, &query_embedding) {
            let strategy_str = strategy_name(&strategy);
            match cache.lookup(emb, strategy_str).await {
                Ok(Some(cached_results)) => {
                    tracing::debug!("cache hit for query");
                    let mut trace = QueryTrace::new(query, &strategy);
                    trace.cache_hit = true;
                    trace.final_count = cached_results.len();
                    trace.set_duration(start.elapsed());
                    trace.emit();
                    return Ok(cached_results);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "cache lookup failed, proceeding \
                         without cache"
                    );
                }
            }
        }

        // --- Step 3: Adaptive strategy selection ---
        let effective_strategy = if strategy == SearchStrategy::Auto {
            // First, check for keyword-based intent signals.
            // These are strong indicators that override
            // score-based selection (e.g., "latest" → Recent).
            if let Some(intent_strategy) = detect_intent(query) {
                tracing::debug!(?intent_strategy, "intent detection selected strategy");
                intent_strategy
            } else if let Some(ref emb) = query_embedding {
                // Fall back to SkewRoute score-based selection.
                let probe_query = SearchQuery {
                    text: query.to_string(),
                    strategy: SearchStrategy::Balanced,
                    limit: 20,
                    time_range,
                    embedding: Some(emb.clone()),
                    ..SearchQuery::default()
                };
                match self.vector.search(&probe_query).await {
                    Ok(results) => {
                        let scores: Vec<f64> = results.iter().map(|r| r.score).collect();
                        let mut selected = select_strategy(&scores);
                        // Guard: Global strategy relies on
                        // community summaries (articles). If
                        // none exist, fall back to Exploratory
                        // which is the right choice for broad
                        // queries (graph-heavy traversal).
                        if selected == SearchStrategy::Global {
                            let has_articles = self
                                .global
                                .search(&probe_query)
                                .await
                                .is_ok_and(|r| !r.is_empty());
                            if !has_articles {
                                selected = SearchStrategy::Exploratory;
                                tracing::debug!(
                                    "skewroute: Global → \
                                     Exploratory (no articles)"
                                );
                            }
                        }
                        if selected != SearchStrategy::Balanced {
                            tracing::debug!(
                                ?selected,
                                "skewroute auto-selected \
                                     strategy"
                            );
                        }
                        selected
                    }
                    Err(_) => strategy.clone(),
                }
            } else {
                strategy.clone()
            }
        } else {
            strategy.clone()
        };

        // --- Step 4: Run all 6 dimensions concurrently ---
        let search_query = SearchQuery {
            text: query.to_string(),
            strategy: effective_strategy.clone(),
            limit,
            time_range,
            embedding: query_embedding.clone(),
            ..SearchQuery::default()
        };

        let (vec_r, lex_r, tmp_r, grp_r, str_r, glb_r) = tokio::join!(
            self.vector.search(&search_query),
            self.lexical.search(&search_query),
            self.temporal.search(&search_query),
            self.graph_dim.search(&search_query),
            self.structural.search(&search_query),
            self.global.search(&search_query),
        );

        // --- Step 5: Collect and fuse ---
        let mut trace = QueryTrace::new(query, &effective_strategy);

        let dimensions: [(&str, std::result::Result<Vec<fusion::SearchResult>, _>, f64); 6] = {
            let w = effective_strategy.weights();
            [
                ("vector", vec_r, w.vector),
                ("lexical", lex_r, w.lexical),
                ("temporal", tmp_r, w.temporal),
                ("graph", grp_r, w.graph),
                ("structural", str_r, w.structural),
                ("global", glb_r, w.global),
            ]
        };

        // Entity demotion: for content-focused strategies, push bare
        // entity nodes to the end of each dimension's ranked list
        // BEFORE fusion. This prevents nodes from crowding out
        // chunks in high-weight dimensions like vector search.
        let demote_entities = !matches!(
            effective_strategy,
            SearchStrategy::Exploratory | SearchStrategy::GraphFirst | SearchStrategy::Global
        );

        let mut ranked_lists = Vec::new();
        let mut weights = Vec::new();
        let mut snippets: HashMap<Uuid, String> = HashMap::new();
        let mut result_types: HashMap<Uuid, String> = HashMap::new();

        for (name, result, weight) in dimensions {
            match result {
                Ok(mut results) => {
                    tracing::debug!(
                        dimension = name,
                        count = results.len(),
                        weight,
                        "search dimension returned results"
                    );
                    trace.record_dimension(name, results.len());
                    for r in &results {
                        if let Some(s) = &r.snippet {
                            snippets.entry(r.id).or_insert_with(|| s.clone());
                        }
                        if let Some(rt) = &r.result_type {
                            result_types.entry(r.id).or_insert_with(|| rt.clone());
                        }
                    }

                    // Per-dimension entity demotion: move bare entity
                    // nodes to the back of the ranked list so they
                    // get worse RRF ranks. This is more principled
                    // than post-fusion score multiplication because
                    // it affects the rank input to RRF directly.
                    if demote_entities && !results.is_empty() {
                        let (mut content, entities): (Vec<_>, Vec<_>) = results
                            .drain(..)
                            .partition(|r| r.result_type.as_deref() != Some("node"));
                        // Re-rank: content items keep their positions,
                        // entity nodes follow at the end.
                        content.extend(entities);
                        for (i, r) in content.iter_mut().enumerate() {
                            r.rank = i + 1;
                        }
                        results = content;
                    }

                    ranked_lists.push(results);
                    weights.push(weight);
                }
                Err(e) => {
                    tracing::warn!(
                        dimension = name,
                        error = %e,
                        "search dimension failed, skipping"
                    );
                    trace.record_dimension(name, 0);
                }
            }
        }

        // --- Step 5b: Per-dimension quality gating ---
        // A non-discriminating dimension (where all results have
        // nearly identical scores) adds noise to fusion without
        // providing ranking signal. Reduce its weight proportionally
        // to its discrimination power (score spread).
        // Based on: "Balancing the Blend" (2508.01405) — a weak
        // retrieval path substantially degrades fused accuracy.
        for (list, weight) in ranked_lists.iter().zip(weights.iter_mut()) {
            if list.len() < 2 {
                continue;
            }
            let max_score = list
                .iter()
                .map(|r| r.score)
                .fold(f64::NEG_INFINITY, f64::max);
            let min_score = list.iter().map(|r| r.score).fold(f64::INFINITY, f64::min);
            let spread = if max_score > 0.0 {
                (max_score - min_score) / max_score
            } else {
                0.0
            };
            // If spread < 0.05 (less than 5% relative difference
            // between best and worst), the dimension isn't
            // discriminating. Scale its weight by the spread ratio.
            if spread < 0.05 {
                let dampening = spread / 0.05;
                tracing::debug!(
                    spread,
                    dampening,
                    original_weight = *weight,
                    "quality gate dampened non-discriminating dimension"
                );
                *weight *= dampening;
            }
        }

        // --- Step 5c: Redistribute weight from empty dimensions ---
        // If a dimension returned 0 results (e.g., global with no
        // community summaries), its weight is wasted in RRF.
        // Redistribute it proportionally to non-empty dimensions so
        // strategy-selected weights remain meaningful.
        let empty_weight: f64 = ranked_lists
            .iter()
            .zip(weights.iter())
            .filter(|(list, _)| list.is_empty())
            .map(|(_, &w)| w)
            .sum();
        if empty_weight > 0.0 {
            let active_weight: f64 = weights
                .iter()
                .zip(ranked_lists.iter())
                .filter(|(_, list)| !list.is_empty())
                .map(|(&w, _)| w)
                .sum();
            if active_weight > 0.0 {
                for (w, list) in weights.iter_mut().zip(ranked_lists.iter()) {
                    if !list.is_empty() {
                        *w += empty_weight * (*w / active_weight);
                    }
                }
                tracing::debug!(
                    redistributed = empty_weight,
                    active_dimensions = ranked_lists.iter().filter(|l| !l.is_empty()).count(),
                    "redistributed weight from empty dimensions"
                );
            }
        }

        // Log effective weights and top-1 item per dimension for
        // diagnosing score anomalies.
        let dim_names = [
            "vector",
            "lexical",
            "temporal",
            "graph",
            "structural",
            "global",
        ];
        let weight_summary: String = dim_names
            .iter()
            .zip(weights.iter())
            .filter(|(_, w)| **w > 1e-6)
            .map(|(name, w)| {
                let abbr = &name[..name.len().min(3)];
                format!("{abbr}={w:.3}")
            })
            .collect::<Vec<_>>()
            .join(" ");
        tracing::debug!(weights = %weight_summary, "effective fusion weights");

        let mut fused = if self.use_cc_fusion {
            fusion::cc_fuse(&ranked_lists, &weights)
        } else {
            fusion::rrf_fuse(&ranked_lists, &weights, fusion::DEFAULT_K)
        };
        trace.fused_count = fused.len();

        // --- Step 6: Abstention detection ---
        let scores: Vec<f64> = fused.iter().map(|r| r.fused_score).collect();
        let abstention_check = check_abstention(&scores, &self.abstention_config);
        if abstention_check.should_abstain {
            trace.abstained = true;
            tracing::info!(
                reason = ?abstention_check.reason,
                "search abstention triggered"
            );
        }

        // --- Step 7: Query expansion (spreading activation) ---
        if !fused.is_empty() {
            let top_k = 5.min(fused.len());
            let seed_ids: Vec<Uuid> = fused[..top_k].iter().map(|r| r.id).collect();
            let spread = spreading_activation(&seed_ids, &self.graph, None).await;
            if !spread.expanded_ids.is_empty() {
                tracing::debug!(
                    expanded = spread.expanded_ids.len(),
                    seeds = spread.seeds_used,
                    "spreading activation found neighbors"
                );
                // Merge expanded IDs as low-score entries if
                // they aren't already in fused results.
                let existing: std::collections::HashSet<Uuid> =
                    fused.iter().map(|r| r.id).collect();
                for eid in &spread.expanded_ids {
                    if !existing.contains(eid) {
                        let w = spread.weights.get(eid).copied().unwrap_or(0.0);
                        // Use a small fraction of the lowest
                        // fused score scaled by activation
                        // weight so expanded items rank below
                        // direct hits.
                        let base = fused.last().map(|r| r.fused_score * 0.5).unwrap_or(0.01);
                        fused.push(FusedResult {
                            id: *eid,
                            fused_score: base * w.min(1.0),
                            confidence: None,
                            entity_type: None,
                            name: None,
                            snippet: None,
                            content: None,
                            source_uri: None,
                            source_title: None,
                            result_type: None,
                            created_at: None,
                            dimension_scores: HashMap::new(),
                            dimension_ranks: HashMap::new(),
                            graph_context: None,
                        });
                    }
                }
                // Re-sort after expansion merge.
                fused.sort_by(|a, b| {
                    b.fused_score
                        .partial_cmp(&a.fused_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        // --- Step 8: Enrichment ---
        for result in &mut fused {
            result.snippet = snippets.remove(&result.id);
            let rtype = result_types.get(&result.id).map(|s| s.as_str());
            result.result_type = rtype.map(|s| s.to_string());

            match rtype {
                Some("node") => {
                    if let Ok(Some(node)) =
                        NodeRepo::get(&*self.repo, NodeId::from_uuid(result.id)).await
                    {
                        result.name = Some(node.canonical_name);
                        result.entity_type = Some(node.node_type);
                        result.content = node.description.clone();
                        result.confidence =
                            node.confidence_breakdown.map(|o| o.projected_probability());
                    }
                }
                Some("article") => {
                    if let Ok(Some(article)) =
                        ArticleRepo::get(&*self.repo, ArticleId::from_uuid(result.id)).await
                    {
                        result.name = Some(article.title);
                        result.entity_type = Some("article".to_string());
                        result.content = Some(article.body.clone());
                        result.confidence = article
                            .confidence_breakdown
                            .map(|o| o.projected_probability());
                        if result.snippet.is_none() {
                            result.snippet = Some(kwic_snippet(&article.body, query, 300));
                        }
                    }
                }
                Some("source") => {
                    if let Ok(Some(source)) = SourceRepo::get(
                        &*self.repo,
                        crate::types::ids::SourceId::from_uuid(result.id),
                    )
                    .await
                    {
                        result.name = source.title.clone();
                        result.entity_type = Some("source".to_string());
                        result.source_uri = source.uri;
                        result.source_title = source.title;
                        result.created_at = Some(source.ingested_at.to_rfc3339());
                        // Use truncated raw content for snippet.
                        if let Some(ref raw) = source.raw_content {
                            result.content = Some(truncate_with_ellipsis(raw, 500));
                        }
                    }
                }
                _ => {
                    // Chunk or unknown — try chunk lookup for
                    // source_uri and content, then node lookup
                    // for compat.
                    if let Ok(Some(chunk)) =
                        ChunkRepo::get(&*self.repo, ChunkId::from_uuid(result.id)).await
                    {
                        result.content = Some(chunk.content.clone());
                        result.entity_type = Some("chunk".to_string());
                        // Look up source first so we can qualify
                        // generic chunk headings with the source title.
                        let src_title = if let Ok(Some(source)) =
                            SourceRepo::get(&*self.repo, chunk.source_id).await
                        {
                            result.source_uri = source.uri;
                            result.source_title = source.title.clone();
                            result.created_at = Some(source.ingested_at.to_rfc3339());
                            source.title
                        } else {
                            None
                        };
                        result.name = Some(derive_chunk_name_qualified(
                            &chunk.content,
                            src_title.as_deref(),
                        ));
                        // Content-based snippet fallback: if no lexical
                        // snippet exists, extract a keyword-in-context
                        // window around query terms. Falls back to the
                        // first 200 chars if no terms match.
                        if result.snippet.is_none() {
                            result.snippet = Some(kwic_snippet(&chunk.content, query, 200));
                        }
                        // Parent-context injection: for paragraph-level
                        // chunks, prepend truncated parent content to
                        // the snippet (avoids a second chunk fetch).
                        if chunk.level == "paragraph" {
                            if let Some(parent_id) = chunk.parent_chunk_id {
                                if let Ok(Some(parent)) =
                                    ChunkRepo::get(&*self.repo, parent_id).await
                                {
                                    let parent_ctx = truncate_with_ellipsis(&parent.content, 200);
                                    let prefix = format!("[{}: {}]", parent.level, parent_ctx);
                                    result.snippet = Some(match result.snippet.take() {
                                        Some(s) => format!("{} {}", prefix, s),
                                        None => prefix,
                                    });
                                }
                            }
                        }
                    }
                    // If still no entity_type, try source lookup
                    // (vector dimension produces source results
                    // but result_type may not propagate through
                    // fusion).
                    if result.entity_type.is_none() {
                        if let Ok(Some(source)) = SourceRepo::get(
                            &*self.repo,
                            crate::types::ids::SourceId::from_uuid(result.id),
                        )
                        .await
                        {
                            result.name = source.title.clone();
                            result.entity_type = Some("source".to_string());
                            result.source_uri = source.uri;
                            result.source_title = source.title;
                            result.created_at = Some(source.ingested_at.to_rfc3339());
                            if let Some(ref raw) = source.raw_content {
                                result.content = Some(truncate_with_ellipsis(raw, 500));
                            }
                        }
                    }
                    if let Ok(Some(node)) =
                        NodeRepo::get(&*self.repo, NodeId::from_uuid(result.id)).await
                    {
                        result.name = Some(node.canonical_name);
                        result.entity_type = Some(node.node_type);
                        // Only set content from node if not
                        // already set from chunk.
                        if result.content.is_none() {
                            result.content = node.description.clone();
                        }
                        result.confidence =
                            node.confidence_breakdown.map(|o| o.projected_probability());
                    }
                }
            }
        }

        // --- Step 8a: Graph context enrichment ---
        // For node-type results, attach 1-hop graph neighbors to
        // provide relationship context. Uses the in-memory sidecar
        // (no DB queries) so this is fast.
        {
            const MAX_NEIGHBORS: usize = 10;
            let graph = self.graph.read().await;
            for result in &mut fused {
                if result.entity_type.as_deref().is_none_or(|t| {
                    t == "chunk" || t == "article" || t == "source"
                }) {
                    continue;
                }
                let Some(idx) = graph.node_index(result.id) else {
                    continue;
                };
                let mut related = Vec::new();
                // Outgoing edges
                for edge in graph.graph().edges(idx) {
                    let target = &graph.graph()[edge.target()];
                    let meta = edge.weight();
                    if meta.is_synthetic {
                        continue;
                    }
                    related.push(RelatedEntity {
                        name: target.canonical_name.clone(),
                        rel_type: meta.rel_type.clone(),
                        direction: "outgoing".to_string(),
                    });
                    if related.len() >= MAX_NEIGHBORS {
                        break;
                    }
                }
                // Incoming edges (if room)
                if related.len() < MAX_NEIGHBORS {
                    for edge in graph
                        .graph()
                        .edges_directed(idx, petgraph::Direction::Incoming)
                    {
                        let source = &graph.graph()[edge.source()];
                        let meta = edge.weight();
                        if meta.is_synthetic {
                            continue;
                        }
                        related.push(RelatedEntity {
                            name: source.canonical_name.clone(),
                            rel_type: meta.rel_type.clone(),
                            direction: "incoming".to_string(),
                        });
                        if related.len() >= MAX_NEIGHBORS {
                            break;
                        }
                    }
                }
                if !related.is_empty() {
                    result.graph_context = Some(related);
                }
            }
        }

        // --- Step 8b: Post-fusion entity demotion ---
        // Secondary demotion pass: entity nodes that appear in many
        // dimensions can accumulate high fused scores. Apply a score
        // multiplier that scales with dimension evidence: nodes found
        // by 3+ dimensions get lighter demotion (they're clearly
        // relevant), while single-dimension nodes get full demotion.
        //
        // Exception: entities whose name appears in the query text
        // are exempt from demotion — the user is likely asking about
        // that entity specifically.
        let query_lower = query.to_lowercase();
        if demote_entities {
            let mut demoted_count = 0usize;
            for result in &mut fused {
                let is_bare_entity = result.result_type.as_deref() == Some("node")
                    && result
                        .entity_type
                        .as_deref()
                        .is_none_or(|t| t != "community_summary" && t != "article");
                if is_bare_entity {
                    // Skip demotion if the entity name appears in
                    // the query (case-insensitive).
                    let name_in_query = result.name.as_ref().is_some_and(|name| {
                        let name_lower = name.to_lowercase();
                        name_lower.len() >= 3 && query_lower.contains(&name_lower)
                    });
                    if name_in_query {
                        continue;
                    }

                    let num_dims = result.dimension_scores.len();
                    // Scale demotion: 1 dim → 0.3, 2 → 0.5, 3+ → 0.7
                    let factor = match num_dims {
                        0 | 1 => ENTITY_DEMOTION_FACTOR,
                        2 => 0.5,
                        _ => 0.7,
                    };
                    result.fused_score *= factor;
                    demoted_count += 1;
                }
            }
            if demoted_count > 0 {
                tracing::debug!(
                    demoted_count,
                    factor = ENTITY_DEMOTION_FACTOR,
                    "demoted bare entity nodes in search results"
                );
                trace.entities_demoted = demoted_count;
                // Re-sort after demotion.
                fused.sort_by(|a, b| {
                    b.fused_score
                        .partial_cmp(&a.fused_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        // --- Step 9: Reranking ---
        // Build documents for the reranker. Prefer snippet, then
        // name, then truncated content. Vector-only chunk results
        // have no snippet or name — without content fallback they
        // would be reranked against empty strings and always lose.
        let documents: Vec<String> = fused
            .iter()
            .map(|r| {
                r.snippet
                    .clone()
                    .or_else(|| r.name.clone())
                    .or_else(|| r.content.as_ref().map(|c| truncate_with_ellipsis(c, 500)))
                    .unwrap_or_default()
            })
            .collect();

        if !documents.is_empty() && !self.reranker.is_noop() {
            match self.reranker.rerank(query, &documents).await {
                Ok(reranked) => {
                    // Blend reranker scores with fusion scores rather
                    // than letting the reranker completely override
                    // multi-dimensional evidence. The reranker provides
                    // text-level relevance; fusion provides multi-signal
                    // evidence.
                    let max_rerank = reranked
                        .iter()
                        .map(|r| r.relevance_score)
                        .fold(0.0f64, f64::max);
                    if max_rerank > 0.0 {
                        for rr in &reranked {
                            if rr.index < fused.len() {
                                let norm_score = rr.relevance_score / max_rerank;
                                // Blend: 60% fusion score + 40% reranker
                                fused[rr.index].fused_score =
                                    fused[rr.index].fused_score * 0.6 + norm_score * 0.4;
                            }
                        }
                        fused.sort_by(|a, b| {
                            b.fused_score
                                .partial_cmp(&a.fused_score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "reranking failed, \
                         keeping fusion order"
                    );
                }
            }
        }

        // --- Step 10: Post-fusion filters ---
        if let Some(ref f) = filters {
            if let Some(min_conf) = f.min_confidence {
                fused.retain(|r| r.confidence.is_some_and(|c| c >= min_conf));
            }
            if let Some(ref types) = f.node_types {
                fused.retain(|r| r.entity_type.as_ref().is_some_and(|t| types.contains(t)));
            }
        }

        // --- Step 10b: Source diversification + content dedup ---
        // Hierarchical chunkers produce chunks at multiple levels
        // (source, section, paragraph) with overlapping content.
        // Without dedup, the same text appears multiple times.
        //
        // Two-pass approach:
        // 1. Content dedup: within the same source, if two results
        //    share the first 100 chars of content, keep the higher-
        //    scored one (catches hierarchical overlap).
        // 2. Source cap: max 2 chunks per source URI.
        const MAX_CHUNKS_PER_SOURCE: usize = 2;
        const CONTENT_PREFIX_LEN: usize = 100;
        {
            // Pass 1: content prefix dedup within same source.
            let mut seen_prefixes: HashMap<(String, String), usize> = HashMap::new();
            let pre_dedup = fused.len();
            fused.retain(|r| {
                if let (Some(uri), Some(content)) = (&r.source_uri, &r.content) {
                    let prefix_end = content
                        .char_indices()
                        .nth(CONTENT_PREFIX_LEN)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());
                    let prefix = content[..prefix_end].to_string();
                    let key = (uri.clone(), prefix);
                    let count = seen_prefixes.entry(key).or_insert(0);
                    *count += 1;
                    *count <= 1 // Keep only first occurrence
                } else {
                    true
                }
            });
            let deduped = pre_dedup - fused.len();

            // Pass 2: per-source count cap.
            let mut source_counts: HashMap<String, usize> = HashMap::new();
            let pre_diverse = fused.len();
            fused.retain(|r| {
                if let Some(ref uri) = r.source_uri {
                    let count = source_counts.entry(uri.clone()).or_insert(0);
                    *count += 1;
                    *count <= MAX_CHUNKS_PER_SOURCE
                } else {
                    true
                }
            });
            let capped = pre_diverse - fused.len();

            // Pass 3: article title dedup — different communities
            // can produce articles with the same title. Keep the
            // highest-scored instance.
            let mut seen_titles: HashMap<String, usize> = HashMap::new();
            let pre_title = fused.len();
            fused.retain(|r| {
                if r.entity_type.as_deref() == Some("article") {
                    if let Some(name) = &r.name {
                        let count = seen_titles.entry(name.clone()).or_insert(0);
                        *count += 1;
                        *count <= 1
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
            let title_deduped = pre_title - fused.len();

            if deduped + capped + title_deduped > 0 {
                tracing::debug!(
                    content_deduped = deduped,
                    source_capped = capped,
                    article_title_deduped = title_deduped,
                    max_per_source = MAX_CHUNKS_PER_SOURCE,
                    "source diversification"
                );
            }
        }

        fused.truncate(limit);

        // --- Step 11: Cache population + trace ---
        for result in &fused {
            let rtype = result.result_type.as_deref().unwrap_or("unknown");
            trace.record_result_type(rtype);
        }
        trace.final_count = fused.len();
        trace.set_duration(start.elapsed());
        trace.emit();

        if let (Some(cache), Some(emb)) = (&self.cache, &query_embedding) {
            let strategy_str = strategy_name(&strategy);
            if let Err(e) = cache.store(query, emb, strategy_str, &fused).await {
                tracing::warn!(
                    error = %e,
                    "failed to store search results in cache"
                );
            }
        }

        Ok(fused)
    }

    /// Execute a search and return the full response including
    /// Assemble fused results into a context string suitable
    /// for LLM generation.
    ///
    /// Applies deduplication (cosine > 0.95), source diversity
    /// (max 3 per source), and token budget (8K default).
    /// Returns an `AssembledContext` with numbered references.
    pub async fn assemble_context(
        &self,
        results: &[FusedResult],
        config: Option<ContextConfig>,
    ) -> AssembledContext {
        let config = config.unwrap_or_default();

        let raw_items: Vec<RawContextItem> = results
            .iter()
            .map(|r| {
                let content = r
                    .content
                    .clone()
                    .or_else(|| r.snippet.clone())
                    .or_else(|| r.name.clone())
                    .unwrap_or_default();
                // Rough token estimate: ~4 chars per token.
                let token_count = content.len().div_ceil(4);
                RawContextItem {
                    content,
                    source_id: r.source_uri.clone().or_else(|| r.entity_type.clone()),
                    source_title: r.source_title.clone().or_else(|| r.name.clone()),
                    score: r.fused_score,
                    token_count,
                    embedding: None,
                    parent_context: None,
                }
            })
            .collect();

        assemble_context(raw_items, &config)
    }

    /// Format an assembled context into a single string with
    /// numbered references suitable for LLM prompts.
    pub fn format_context(context: &AssembledContext) -> String {
        let mut out = String::new();
        for item in &context.items {
            out.push_str(&format!("[{}] {}\n\n", item.ref_number, item.content));
        }
        out
    }
}

/// Map a strategy to its string name for cache keying and
/// trace recording.
fn strategy_name(strategy: &SearchStrategy) -> &'static str {
    match strategy {
        SearchStrategy::Auto => "auto",
        SearchStrategy::Balanced => "balanced",
        SearchStrategy::Precise => "precise",
        SearchStrategy::Exploratory => "exploratory",
        SearchStrategy::Recent => "recent",
        SearchStrategy::GraphFirst => "graph_first",
        SearchStrategy::Global => "global",
        SearchStrategy::Custom(_) => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate_with_ellipsis tests ---

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii_adds_ellipsis() {
        let result = truncate_with_ellipsis("hello world", 8);
        assert_eq!(result, "hello...");
        assert!(result.len() <= 8);
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate_with_ellipsis("", 10), "");
    }

    #[test]
    fn truncate_emoji_does_not_panic() {
        // '🔥' is 4 bytes. Cutting at byte 5 would land inside the
        // second emoji — must snap back to the boundary.
        let input = "🔥🔥🔥"; // 12 bytes
        let result = truncate_with_ellipsis(input, 8);
        // Can fit one emoji (4 bytes) + "..." (3 bytes) = 7 bytes
        assert_eq!(result, "🔥...");
        assert!(result.len() <= 8);
    }

    #[test]
    fn truncate_cjk_does_not_panic() {
        // CJK characters are 3 bytes each.
        let input = "漢字漢字漢字"; // 18 bytes
        let result = truncate_with_ellipsis(input, 10);
        // max_bytes=10, subtract 3 for "..." = 7, snap back from 7
        // to char boundary at 6 (2 CJK chars), result = "漢字..."
        assert_eq!(result, "漢字...");
        assert!(result.len() <= 10);
    }

    #[test]
    fn truncate_accented_chars_does_not_panic() {
        // 'é' as composed form is 2 bytes in UTF-8.
        let input = "résumé here";
        let result = truncate_with_ellipsis(input, 8);
        // Must not panic and must be valid UTF-8.
        assert!(result.len() <= 8);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_max_bytes_less_than_three() {
        // Edge case: max_bytes < 3 means no room for ellipsis text.
        let result = truncate_with_ellipsis("hello", 2);
        // saturating_sub(3) = 0, so result is "..."
        assert_eq!(result, "...");
    }

    #[test]
    fn derive_chunk_name_heading() {
        let content = "# Introduction\nThis is the body.";
        assert_eq!(derive_chunk_name(content), "Introduction");
    }

    #[test]
    fn derive_chunk_name_multi_hash_heading() {
        let content = "### Configuration Options\nSome config details.";
        assert_eq!(derive_chunk_name(content), "Configuration Options");
    }

    #[test]
    fn derive_chunk_name_first_sentence() {
        let content = "The ingestion pipeline processes documents. It has 9 stages.";
        assert_eq!(
            derive_chunk_name(content),
            "The ingestion pipeline processes documents."
        );
    }

    #[test]
    fn derive_chunk_name_no_period() {
        let content = "Some short text without punctuation";
        assert_eq!(
            derive_chunk_name(content),
            "Some short text without punctuation"
        );
    }

    #[test]
    fn derive_chunk_name_long_truncates() {
        let long = "a".repeat(200);
        let name = derive_chunk_name(&long);
        assert!(name.len() <= MAX_CHUNK_NAME_LEN);
        assert!(name.ends_with("..."));
    }

    #[test]
    fn derive_chunk_name_empty() {
        assert_eq!(derive_chunk_name(""), "");
    }

    #[test]
    fn derive_chunk_name_newline_before_period() {
        // Per-line processing: "First line" is the whole first line.
        let content = "First line\nSecond line.";
        assert_eq!(derive_chunk_name(content), "First line");
    }

    #[test]
    fn derive_chunk_name_skips_bold_label() {
        let content = "**Status:** Draft\nThe real description starts here.";
        assert_eq!(
            derive_chunk_name(content),
            "The real description starts here."
        );
    }

    #[test]
    fn derive_chunk_name_skips_arxiv_ref() {
        let content = "[2506.12345]\nAbstract of the paper.";
        assert_eq!(derive_chunk_name(content), "Abstract of the paper.");
    }

    #[test]
    fn derive_chunk_name_strips_bold() {
        let content = "The **Subjective Logic** framework defines opinions.";
        assert_eq!(
            derive_chunk_name(content),
            "The Subjective Logic framework defines opinions."
        );
    }

    #[test]
    fn derive_chunk_name_strips_inline_code() {
        let content = "Use the `derive_chunk_name` function.";
        assert_eq!(
            derive_chunk_name(content),
            "Use the derive_chunk_name function."
        );
    }

    #[test]
    fn derive_chunk_name_skips_link_line() {
        let content = "[Paper](https://arxiv.org)\nKnowledge graphs are important.";
        assert_eq!(
            derive_chunk_name(content),
            "Knowledge graphs are important."
        );
    }

    #[test]
    fn derive_chunk_name_all_metadata_lines() {
        // When all lines are metadata, fall back to the first line
        // with bold markers stripped.
        let content = "**Authors:** (2506.00049)\n**arxiv:** 2506.00049";
        assert_eq!(derive_chunk_name(content), "Authors: (2506.00049)");
    }

    #[test]
    fn derive_chunk_name_skips_bare_numbered_item() {
        let content = "3.\nThe actual content starts here.";
        assert_eq!(
            derive_chunk_name(content),
            "The actual content starts here."
        );
    }

    #[test]
    fn strip_inline_markdown_bold() {
        assert_eq!(strip_inline_markdown("**bold** text"), "bold text");
    }

    #[test]
    fn strip_inline_markdown_mixed() {
        assert_eq!(
            strip_inline_markdown("**bold** and *italic* and `code`"),
            "bold and italic and code"
        );
    }

    // --- qualify_heading tests ---

    #[test]
    fn qualify_generic_heading_with_source() {
        assert_eq!(
            qualify_heading("Overview", Some("Epistemic Model Spec")),
            "Epistemic Model Spec: Overview"
        );
    }

    #[test]
    fn qualify_generic_heading_case_insensitive() {
        assert_eq!(
            qualify_heading("INTRODUCTION", Some("Paper Title")),
            "Paper Title: INTRODUCTION"
        );
    }

    #[test]
    fn qualify_non_generic_heading_unchanged() {
        assert_eq!(
            qualify_heading("Reciprocal Rank Fusion", Some("Paper Title")),
            "Reciprocal Rank Fusion"
        );
    }

    #[test]
    fn qualify_generic_heading_no_source() {
        assert_eq!(qualify_heading("Overview", None), "Overview");
    }

    #[test]
    fn qualify_generic_heading_empty_source() {
        assert_eq!(qualify_heading("Overview", Some("")), "Overview");
    }

    // --- strip_section_number tests ---

    #[test]
    fn strip_simple_section_number() {
        assert_eq!(strip_section_number("2 Background"), "Background");
    }

    #[test]
    fn strip_dotted_section_number() {
        assert_eq!(strip_section_number("1.2 Methods"), "Methods");
    }

    #[test]
    fn strip_deep_section_number() {
        assert_eq!(strip_section_number("3.1.4. Results"), "Results");
    }

    #[test]
    fn strip_no_section_number() {
        assert_eq!(strip_section_number("Background"), "Background");
    }

    #[test]
    fn strip_number_no_space() {
        // "2Background" is not a section number.
        assert_eq!(strip_section_number("2Background"), "2Background");
    }

    #[test]
    fn strip_just_number() {
        // Bare "2" has no text after it.
        assert_eq!(strip_section_number("2"), "2");
    }

    // --- numbered heading qualification ---

    #[test]
    fn qualify_numbered_generic_heading() {
        assert_eq!(
            qualify_heading("2 Background", Some("My Paper")),
            "My Paper: 2 Background"
        );
    }

    #[test]
    fn qualify_dotted_numbered_heading() {
        assert_eq!(
            qualify_heading("1.2 Introduction", Some("Survey")),
            "Survey: 1.2 Introduction"
        );
    }

    #[test]
    fn numbered_non_generic_not_qualified() {
        assert_eq!(
            qualify_heading("3 Reciprocal Rank Fusion", Some("Paper")),
            "3 Reciprocal Rank Fusion"
        );
    }

    #[test]
    fn qualified_name_via_derive() {
        let content = "## Overview\nThe system is designed for...";
        assert_eq!(
            derive_chunk_name_qualified(content, Some("Search Engine")),
            "Search Engine: Overview"
        );
    }

    #[test]
    fn qualified_name_specific_heading_not_qualified() {
        let content = "## Reciprocal Rank Fusion\nRRF merges ranked lists.";
        assert_eq!(
            derive_chunk_name_qualified(content, Some("Search Engine")),
            "Reciprocal Rank Fusion"
        );
    }

    #[test]
    fn qualified_name_no_source_falls_back() {
        let content = "## Overview\nThe system is designed for...";
        assert_eq!(derive_chunk_name_qualified(content, None), "Overview");
    }

    // --- KWIC snippet tests ---

    #[test]
    fn kwic_finds_query_term() {
        let content = "The preamble discusses many topics. \
            GraphRAG is a powerful paradigm for knowledge retrieval. \
            It combines graph structure with RAG approaches.";
        let snippet = kwic_snippet(content, "GraphRAG knowledge", 80);
        assert!(snippet.contains("GraphRAG"));
    }

    #[test]
    fn kwic_falls_back_to_truncate() {
        let content = "Some content that has no matching terms at all.";
        let snippet = kwic_snippet(content, "xyzzy foobar", 20);
        // Should fall back to first 20 bytes.
        assert!(snippet.len() <= 23); // 20 + "..."
    }

    #[test]
    fn kwic_short_terms_ignored() {
        // Terms < 3 chars should be skipped.
        let content = "A is B or C and D.";
        let snippet = kwic_snippet(content, "A B C", 10);
        // All terms are < 3 chars, falls back to truncate.
        assert_eq!(snippet, kwic_snippet(content, "", 10));
    }

    #[test]
    fn kwic_centers_window() {
        let content = "aaa bbb ccc ddd eee fff ggg hhh iii jjj kkk lll mmm";
        let snippet = kwic_snippet(content, "ggg", 20);
        assert!(snippet.contains("ggg"));
        // Should have ... prefix since ggg is in the middle.
        assert!(snippet.starts_with("..."));
    }

    #[test]
    fn kwic_empty_content() {
        assert_eq!(kwic_snippet("", "test", 100), "");
    }

    #[test]
    fn kwic_snaps_to_word_boundaries() {
        let content = "The knowledge graph enables powerful entity resolution \
            and relationship extraction from unstructured sources.";
        let snippet = kwic_snippet(content, "entity resolution", 60);
        assert!(snippet.contains("entity resolution"));
        // Should not start or end mid-word.
        if snippet.starts_with("...") {
            let body = &snippet[3..];
            // First char of body should be non-whitespace (start of word).
            assert!(
                body.starts_with(|c: char| !c.is_whitespace()),
                "snippet should start at word boundary: {snippet}"
            );
        }
    }
}
