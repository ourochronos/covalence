//! Search filters and post-fusion filtering/diversification.
//!
//! Contains the [`SearchFilters`] struct for narrowing search results,
//! and post-fusion logic including entity demotion, code-chunk
//! demotion, DDSS boost, quality gating, diversification, and
//! epistemic confidence boost.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::search::dimensions::GraphView;
use crate::search::fusion::FusedResult;
use crate::search::trace::QueryTrace;

use super::super::search_helpers::ENTITY_DEMOTION_FACTOR;

/// Post-fusion filters for narrowing search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilters {
    /// Minimum epistemic confidence (projected probability).
    pub min_confidence: Option<f64>,
    /// Restrict to specific node types.
    pub node_types: Option<Vec<String>>,
    /// Restrict to specific entity classes: code, domain, actor, analysis.
    pub entity_classes: Option<Vec<String>>,
    /// Restrict to a temporal date range.
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
    /// Restrict to specific source types (e.g. "document", "code").
    /// Applies only to chunk and source results.
    pub source_types: Option<Vec<String>>,
    /// Filter by source domain. Matches against
    /// `FusedResult.source_domains`. A result passes if any of its
    /// domains overlap with the filter list.
    pub domains: Option<Vec<String>>,
    /// Orthogonal graph view restricting which edges the graph
    /// dimension traverses: "causal", "temporal", "entity",
    /// "structural", "all". Passed through to the graph dimension.
    pub graph_view: Option<GraphView>,
}

/// Apply post-fusion entity demotion (Step 8b).
///
/// Entity nodes that appear in many dimensions can accumulate high
/// fused scores. Apply a score multiplier that scales with dimension
/// evidence: nodes found by 3+ dimensions get lighter demotion,
/// while single-dimension nodes get full demotion. Entities whose
/// name appears in the query text are exempt.
pub(super) fn apply_entity_demotion(
    fused: &mut [FusedResult],
    query: &str,
    demote_entities: bool,
    trace: &mut QueryTrace,
) {
    let query_lower = query.to_lowercase();
    if !demote_entities {
        return;
    }

    let mut demoted_count = 0usize;
    for result in fused.iter_mut() {
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
            "demoted bare entity nodes in search results \
             (1-dim=0.3, 2-dim=0.5, 3+-dim=0.7)"
        );
        trace.entities_demoted = demoted_count;
        // Re-sort after demotion.
        sort_by_score(fused);
    }
}

/// Apply code-chunk demotion (Step 8c).
///
/// Chunks from `code` sources contain keywords (function names,
/// command descriptions) that match lexically but rarely answer
/// conceptual queries. Dampen their scores so they appear lower.
pub(super) fn apply_code_chunk_demotion(fused: &mut [FusedResult]) {
    let mut code_demoted = 0usize;
    for result in fused.iter_mut() {
        if result.source_type.as_deref() == Some("code")
            && result.result_type.as_deref() != Some("node")
        {
            result.fused_score *= 0.5;
            code_demoted += 1;
        }
    }
    if code_demoted > 0 {
        tracing::debug!(code_demoted, "demoted code-source chunks in search results");
        sort_by_score(fused);
    }
}

/// Apply self-referential domain boost — DDSS (Step 8d).
///
/// Detects when the query is about the system itself by comparing
/// max scores from internal domains (spec/design/code) vs external
/// (research/external). If internal content scores well relative
/// to external, boost all internal results to surface them.
pub(super) fn apply_ddss_boost(
    fused: &mut [FusedResult],
    internal_domains: &HashSet<String>,
    trace: &mut QueryTrace,
) {
    const BOOST_THRESHOLD: f64 = 0.7; // ratio above which we boost
    const BOOST_FACTOR: f64 = 1.5; // multiplier for internal results

    let max_internal = fused
        .iter()
        .filter(|r| {
            r.source_domains
                .iter()
                .any(|d| internal_domains.contains(d))
        })
        .map(|r| r.fused_score)
        .fold(0.0_f64, f64::max);

    let max_external = fused
        .iter()
        .filter(|r| {
            !r.source_domains.is_empty()
                && r.source_domains
                    .iter()
                    .all(|d| !internal_domains.contains(d))
        })
        .map(|r| r.fused_score)
        .fold(0.0_f64, f64::max);

    // Secondary signal: code-class entities in top results
    // indicate implementation-focused queries even without
    // source domain (nodes don't have a single source).
    let code_entity_signal = fused
        .iter()
        .take(10)
        .filter(|r| {
            r.entity_type.as_deref().is_some_and(|t| {
                matches!(
                    crate::models::node::derive_entity_class(t),
                    crate::models::node::EntityClass::Code
                )
            })
        })
        .count();

    if max_external > 1e-9 {
        let ratio = max_internal / max_external;
        // Boost when: internal content scores >= 70% of
        // external, OR 3+ top-10 results are code entities.
        if ratio >= BOOST_THRESHOLD || code_entity_signal >= 3 {
            let mut boosted = 0usize;
            for result in fused.iter_mut() {
                if result
                    .source_domains
                    .iter()
                    .any(|d| internal_domains.contains(d))
                {
                    result.fused_score *= BOOST_FACTOR;
                    boosted += 1;
                }
            }
            if boosted > 0 {
                tracing::info!(
                    ratio = format!("{ratio:.2}"),
                    code_entity_signal,
                    boosted,
                    "DDSS: self-referential boost applied"
                );
                sort_by_score(fused);
                trace.self_referential_boost = true;
            }
        }
    }
}

/// Remove low-quality chunks after reranking (Step 9b).
pub(super) fn remove_low_quality_chunks(fused: &mut Vec<FusedResult>, trace: &mut QueryTrace) {
    use super::super::chunk_quality::{
        is_author_block, is_bibliography_entry, is_boilerplate_heavy, is_metadata_only,
        is_reference_section, is_title_only, is_trivial_code_chunk,
    };

    let pre = fused.len();
    fused.retain(|result| {
        if result.entity_type.as_deref() != Some("chunk") {
            return true;
        }
        let content = match result.content.as_deref() {
            Some(c) => c,
            None => return true,
        };
        !(is_bibliography_entry(content)
            || is_reference_section(content)
            || is_boilerplate_heavy(content)
            || is_metadata_only(content)
            || is_title_only(content)
            || is_author_block(content)
            || is_trivial_code_chunk(content))
    });
    let removed = pre - fused.len();
    if removed > 0 {
        tracing::debug!(removed, "removed low-quality chunks after reranking");
        trace.chunks_quality_demoted = removed;
    }
}

/// Apply post-fusion filters (Step 10).
pub(super) fn apply_post_fusion_filters(fused: &mut Vec<FusedResult>, filters: &SearchFilters) {
    if let Some(min_conf) = filters.min_confidence {
        fused.retain(|r| r.confidence.is_some_and(|c| c >= min_conf));
    }
    if let Some(ref types) = filters.node_types {
        fused.retain(|r| r.entity_type.as_ref().is_some_and(|t| types.contains(t)));
    }
    if let Some(ref classes) = filters.entity_classes {
        fused.retain(|r| {
            // For node results, derive entity_class from entity_type
            if let Some(ref etype) = r.entity_type {
                let ec = crate::models::node::derive_entity_class(etype);
                classes.iter().any(|c| c == ec.as_str())
            } else {
                // Non-node results pass through (chunks, articles)
                true
            }
        });
    }
    if let Some(ref types) = filters.source_types {
        fused.retain(|r| {
            // Pass through non-source/chunk results (nodes,
            // articles) since they have no source_type.
            r.source_type.as_ref().is_none_or(|st| types.contains(st))
        });
    }
    if let Some(ref domains) = filters.domains {
        let pre = fused.len();
        fused.retain(|r| {
            // Pass through non-source/chunk results (nodes,
            // articles) since they have no source_domains.
            if r.source_domains.is_empty() {
                return true;
            }
            r.source_domains
                .iter()
                .any(|d| domains.iter().any(|fd| fd == d))
        });
        tracing::info!(
            domains = ?domains,
            before = pre,
            after = fused.len(),
            "domain filter applied"
        );
    }
}

/// Apply source diversification and content dedup (Step 10b).
pub(super) fn apply_diversification(fused: &mut Vec<FusedResult>) {
    const MAX_CHUNKS_PER_SOURCE: usize = 2;
    const CONTENT_PREFIX_LEN: usize = 100;

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

    // Pass 1b: cross-source content dedup. Same content
    // prefix from different sources (e.g., codebase
    // re-ingested multiple times) → keep first (highest
    // scored since results are sorted).
    let mut cross_prefixes: HashMap<String, usize> = HashMap::new();
    let pre_cross = fused.len();
    fused.retain(|r| {
        if r.result_type.as_deref() == Some("chunk") {
            if let Some(content) = &r.content {
                let prefix_end = content
                    .char_indices()
                    .nth(CONTENT_PREFIX_LEN)
                    .map(|(i, _)| i)
                    .unwrap_or(content.len());
                let prefix = content[..prefix_end].to_string();
                let count = cross_prefixes.entry(prefix).or_insert(0);
                *count += 1;
                *count <= 1
            } else {
                true
            }
        } else {
            true
        }
    });
    let cross_deduped = pre_cross - fused.len();

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

    if deduped + cross_deduped + capped + title_deduped > 0 {
        tracing::debug!(
            content_deduped = deduped,
            cross_source_deduped = cross_deduped,
            source_capped = capped,
            article_title_deduped = title_deduped,
            max_per_source = MAX_CHUNKS_PER_SOURCE,
            "source diversification"
        );
    }
}

/// Apply epistemic confidence boost (Step 10c).
///
/// Adjusts fused scores based on epistemic confidence so
/// high-confidence results rank above low-confidence ones.
///
/// Formula: score *= 1 + gamma * (confidence - 0.5)
pub(super) fn apply_confidence_boost(fused: &mut [FusedResult]) {
    const GAMMA: f64 = 0.3;
    let mut boosted = 0usize;
    for result in fused.iter_mut() {
        if let Some(conf) = result.confidence {
            let factor = 1.0 + GAMMA * (conf - 0.5);
            result.fused_score *= factor;
            boosted += 1;
        }
    }
    if boosted > 0 {
        sort_by_score(fused);
        tracing::debug!(boosted, gamma = GAMMA, "epistemic confidence boost applied");
    }
}

/// Sort fused results by score descending.
fn sort_by_score(fused: &mut [FusedResult]) {
    fused.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}
