//! Core search pipeline — the `search_inner` method and its
//! orchestration logic.
//!
//! Handles the full pipeline: embedding, cache lookup, strategy
//! selection, dimension execution, fusion, abstention, expansion,
//! enrichment, reranking, filtering, and trace recording.

use std::collections::HashMap;
use std::time::Instant;

use uuid::Uuid;

use crate::error::Result;
use crate::search::abstention::check_abstention;
use crate::search::dimensions::{SearchDimension, SearchQuery};
use crate::search::expansion::spreading_activation;
use crate::search::fusion::{self, FusedResult};
use crate::search::skewroute::{detect_intent, select_strategy};
use crate::search::strategy::SearchStrategy;
use crate::search::trace::QueryTrace;

use crate::metrics;

use super::super::search_helpers::{strategy_name, truncate_with_ellipsis};
use super::SearchService;
use super::enrichment;
use super::filters;

impl SearchService {
    /// Core search pipeline shared by `search()` and
    /// `search_hierarchical()`.
    pub(super) async fn search_inner(
        &self,
        query: &str,
        strategy: SearchStrategy,
        limit: usize,
        filters: Option<filters::SearchFilters>,
        hierarchical: bool,
    ) -> Result<Vec<FusedResult>> {
        let start = Instant::now();
        metrics::record_search_query(strategy_name(&strategy));
        let time_range = filters.as_ref().and_then(|f| f.date_range);

        // When post-fusion filters are present (especially
        // source_layers), over-fetch so the filter has enough
        // candidates to meet the requested limit.
        let internal_limit = if filters.is_some() {
            (limit * 5).max(50)
        } else {
            limit
        };

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
        // Skip cache when post-fusion filters are present — the cache
        // keys on (embedding, strategy) only, so cached results would
        // bypass min_confidence, node_types, source_types,
        // source_layers, and date_range filters.
        //
        // Also skip cache for Auto strategy — SkewRoute adaptively
        // selects a strategy per query, so caching under "auto" would
        // conflate results from different resolved strategies. The
        // store step keys on the *resolved* strategy, so explicit
        // strategy queries can still hit cache entries populated by
        // Auto queries that resolved to the same strategy.
        let has_post_filters = filters.is_some();
        let is_auto = strategy == SearchStrategy::Auto;
        if !has_post_filters && !is_auto {
            if let (Some(cache), Some(emb)) = (&self.cache, &query_embedding) {
                let strategy_str = strategy_name(&strategy);
                match cache.lookup(emb, strategy_str).await {
                    Ok(Some(mut cached_results)) => {
                        metrics::record_cache_hit();
                        tracing::debug!("cache hit for query");
                        // The cache stores the full pre-truncation
                        // result set. Apply the caller's limit here
                        // so that limit=15 after a limit=5 cache
                        // write still returns all available results.
                        cached_results.truncate(limit);
                        let mut trace = QueryTrace::new(query, &strategy);
                        trace.cache_hit = true;
                        trace.final_count = cached_results.len();
                        trace.set_duration(start.elapsed());
                        trace.emit();
                        return Ok(cached_results);
                    }
                    Ok(None) => {
                        metrics::record_cache_miss();
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "cache lookup failed, proceeding \
                             without cache"
                        );
                    }
                }
            }
        } // end skip-cache-with-filters guard

        // --- Step 3: Adaptive strategy selection ---
        let effective_strategy = self
            .resolve_strategy(&strategy, query, &query_embedding, time_range)
            .await;

        // --- Step 4: Run all 6 dimensions concurrently ---
        let graph_view = filters.as_ref().and_then(|f| f.graph_view);
        let search_query = SearchQuery {
            text: query.to_string(),
            strategy: effective_strategy.clone(),
            limit: internal_limit,
            time_range,
            embedding: query_embedding.clone(),
            hierarchical,
            graph_view,
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

        let (mut ranked_lists, mut weights, mut snippets, result_types) =
            Self::collect_dimension_results(dimensions, demote_entities, &mut trace);

        // --- Step 5b–5c: Quality gating + weight redistribution ---
        Self::apply_quality_gating_and_redistribution(&mut ranked_lists, &mut weights);

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
        Self::apply_query_expansion(&mut fused, &self.graph).await;

        // --- Step 8: Enrichment ---
        enrichment::enrich_results(&mut fused, &self.repo, query, &mut snippets, &result_types)
            .await;

        // --- Step 8a: Graph context enrichment ---
        enrichment::enrich_graph_context(&mut fused, &self.graph_engine).await;

        // --- Step 8b: Post-fusion entity demotion ---
        filters::apply_entity_demotion(&mut fused, query, demote_entities, &mut trace);

        // --- Step 8c: Code-chunk demotion ---
        filters::apply_code_chunk_demotion(&mut fused);

        // --- Step 8d: Self-referential domain boost (DDSS) ---
        filters::apply_ddss_boost(&mut fused, &self.internal_domains, &mut trace);

        // --- Step 9: Reranking ---
        self.apply_reranking(&mut fused, query).await;

        // --- Step 9b: Low-quality chunk removal ---
        filters::remove_low_quality_chunks(&mut fused, &mut trace);

        // --- Step 10: Post-fusion filters ---
        if let Some(ref f) = filters {
            filters::apply_post_fusion_filters(&mut fused, f);
        }

        // --- Step 10b: Source diversification + content dedup ---
        filters::apply_diversification(&mut fused);

        // --- Step 10c: Epistemic confidence boost ---
        filters::apply_confidence_boost(&mut fused);

        // --- Step 11: Cache population ---
        if let (Some(cache), Some(emb)) = (&self.cache, &query_embedding) {
            let strategy_str = strategy_name(&effective_strategy);
            if let Err(e) = cache.store(query, emb, strategy_str, &fused).await {
                tracing::warn!(
                    error = %e,
                    "failed to store search results in cache"
                );
            }
        }

        fused.truncate(limit);

        // --- Step 12: Trace ---
        for result in &fused {
            let rtype = result.result_type.as_deref().unwrap_or("unknown");
            trace.record_result_type(rtype);
        }
        trace.final_count = fused.len();
        trace.set_duration(start.elapsed());
        trace.emit();

        metrics::record_search_latency(&trace.strategy, start.elapsed().as_secs_f64());

        // Persist trace to DB for offline analysis.
        let db_trace = crate::models::trace::SearchTrace::new(
            trace.query_text.clone(),
            trace.strategy.clone(),
            serde_json::to_value(&trace.dimension_counts).unwrap_or_default(),
            trace.final_count as i32,
            trace.execution_ms as i32,
        );
        if let Err(e) =
            crate::storage::traits::SearchTraceRepo::create(&*self.repo, &db_trace).await
        {
            tracing::warn!(error = %e, "failed to persist search trace");
        }

        Ok(fused)
    }

    /// Resolve the effective search strategy (Step 3).
    async fn resolve_strategy(
        &self,
        strategy: &SearchStrategy,
        query: &str,
        query_embedding: &Option<Vec<f64>>,
        time_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
    ) -> SearchStrategy {
        if *strategy != SearchStrategy::Auto {
            return strategy.clone();
        }

        // First, check for keyword-based intent signals.
        if let Some(intent_strategy) = detect_intent(query) {
            tracing::debug!(?intent_strategy, "intent detection selected strategy");
            return intent_strategy;
        }

        if let Some(emb) = query_embedding {
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
                    // Guard: Global strategy relies on community
                    // summaries (articles). If none exist, fall
                    // back to Exploratory.
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
    }

    /// Collect results from all 6 dimensions (Step 5).
    ///
    /// Returns (ranked_lists, weights, snippets, result_types).
    #[allow(clippy::type_complexity)]
    fn collect_dimension_results(
        dimensions: [(
            &str,
            std::result::Result<Vec<fusion::SearchResult>, crate::error::Error>,
            f64,
        ); 6],
        demote_entities: bool,
        trace: &mut QueryTrace,
    ) -> (
        Vec<Vec<fusion::SearchResult>>,
        Vec<f64>,
        HashMap<Uuid, String>,
        HashMap<Uuid, String>,
    ) {
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
                    // get worse RRF ranks.
                    if demote_entities && !results.is_empty() {
                        let (mut content, entities): (Vec<_>, Vec<_>) = results
                            .drain(..)
                            .partition(|r| r.result_type.as_deref() != Some("node"));
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

        (ranked_lists, weights, snippets, result_types)
    }

    /// Quality gating, zero-weight clearing, and weight
    /// redistribution (Steps 5b, 5b2, 5c).
    ///
    /// Dampens non-discriminating dimensions, clears results from
    /// zero-weight dimensions, and redistributes lost weight
    /// proportionally to active dimensions.
    fn apply_quality_gating_and_redistribution(
        ranked_lists: &mut [Vec<fusion::SearchResult>],
        weights: &mut [f64],
    ) {
        // --- Step 5b: Per-dimension quality gating ---
        let mut dampened_weight = 0.0_f64;
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
            if spread < 0.05 {
                let dampening = spread / 0.05;
                let original = *weight;
                *weight *= dampening;
                dampened_weight += original - *weight;
                tracing::debug!(
                    spread,
                    dampening,
                    original_weight = original,
                    new_weight = *weight,
                    "quality gate dampened non-discriminating dimension"
                );
            }
        }

        // --- Step 5b2: Clear results from zero-weight dims ---
        for (list, weight) in ranked_lists.iter_mut().zip(weights.iter()) {
            if *weight < 1e-12 && !list.is_empty() {
                tracing::debug!(
                    cleared = list.len(),
                    "cleared results from zero-weight dimension"
                );
                list.clear();
            }
        }

        // --- Step 5c: Redistribute weight ---
        let empty_weight: f64 = ranked_lists
            .iter()
            .zip(weights.iter())
            .filter(|(list, w)| list.is_empty() && **w >= 1e-12)
            .map(|(_, &w)| w)
            .sum();
        let total_redistribute = empty_weight + dampened_weight;
        if total_redistribute > 0.0 {
            let active_weight: f64 = weights
                .iter()
                .zip(ranked_lists.iter())
                .filter(|(_, list)| !list.is_empty())
                .map(|(&w, _)| w)
                .sum();
            if active_weight > 0.0 {
                for (w, list) in weights.iter_mut().zip(ranked_lists.iter()) {
                    if !list.is_empty() {
                        *w += total_redistribute * (*w / active_weight);
                    }
                }
                tracing::debug!(
                    redistributed = total_redistribute,
                    from_empty = empty_weight,
                    from_dampened = dampened_weight,
                    active_dimensions = ranked_lists.iter().filter(|l| !l.is_empty()).count(),
                    "redistributed weight from empty/dampened dimensions"
                );
            }
        }
    }

    /// Apply query expansion via spreading activation (Step 7).
    async fn apply_query_expansion(
        fused: &mut Vec<FusedResult>,
        graph: &crate::graph::SharedGraph,
    ) {
        if fused.is_empty() {
            return;
        }
        let top_k = 5.min(fused.len());
        let seed_ids: Vec<Uuid> = fused[..top_k].iter().map(|r| r.id).collect();
        let spread = spreading_activation(&seed_ids, graph, None).await;
        if !spread.expanded_ids.is_empty() {
            tracing::debug!(
                expanded = spread.expanded_ids.len(),
                seeds = spread.seeds_used,
                "spreading activation found neighbors"
            );
            // Merge expanded IDs as low-score entries if they
            // aren't already in fused results.
            let existing: std::collections::HashSet<Uuid> = fused.iter().map(|r| r.id).collect();
            for eid in &spread.expanded_ids {
                if !existing.contains(eid) {
                    let w = spread.weights.get(eid).copied().unwrap_or(0.0);
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
                        source_type: None,
                        source_domain: None,
                        source_domains: Vec::new(),
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

    /// Apply reranking to fused results (Step 9).
    async fn apply_reranking(&self, fused: &mut [FusedResult], query: &str) {
        let documents: Vec<String> = fused
            .iter()
            .map(|r| {
                r.content
                    .as_ref()
                    .map(|c| truncate_with_ellipsis(c, 1000))
                    .or_else(|| r.name.clone())
                    .or_else(|| r.snippet.clone())
                    .unwrap_or_default()
            })
            .collect();

        if !documents.is_empty() && !self.reranker.is_noop() {
            match self.reranker.rerank(query, &documents).await {
                Ok(reranked) => {
                    let max_rerank = reranked
                        .iter()
                        .map(|r| r.relevance_score)
                        .fold(0.0f64, f64::max);
                    if max_rerank > 0.0 {
                        for rr in &reranked {
                            if rr.index < fused.len() {
                                let norm_score = rr.relevance_score / max_rerank;
                                // Blend: 60% fusion + 40% reranker
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
    }
}
