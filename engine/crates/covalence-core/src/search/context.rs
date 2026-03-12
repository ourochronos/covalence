//! Context assembly pipeline.
//!
//! After retrieval and reranking, the raw result set is assembled
//! into a coherent context window for generation. Steps: deduplicate,
//! diversify, expand, order, budget, annotate.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ingestion::landscape::cosine_similarity;

/// Configuration for context assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum total tokens in assembled context.
    pub max_tokens: usize,
    /// Maximum results from a single source.
    pub max_per_source: usize,
    /// Cosine similarity threshold for deduplication.
    pub dedup_threshold: f64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 8000,
            max_per_source: 3,
            dedup_threshold: 0.95,
        }
    }
}

/// A context item ready for LLM consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// Reference number for citation (1-indexed).
    pub ref_number: usize,
    /// The content text.
    pub content: String,
    /// Source title for attribution.
    pub source_title: Option<String>,
    /// Source ID for provenance.
    pub source_id: Option<String>,
    /// Relevance score from search.
    pub score: f64,
    /// Token count of this item.
    pub token_count: usize,
    /// Optional parent context (for chunks with low parent
    /// alignment).
    pub parent_context: Option<String>,
}

/// Result of context assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembledContext {
    /// Ordered context items with reference numbers.
    pub items: Vec<ContextItem>,
    /// Total token count.
    pub total_tokens: usize,
    /// Number of items dropped due to budget.
    pub items_dropped: usize,
    /// Number of duplicates removed.
    pub duplicates_removed: usize,
}

/// Input item for the assembly pipeline.
#[derive(Debug, Clone)]
pub struct RawContextItem {
    /// Content text.
    pub content: String,
    /// Source identifier.
    pub source_id: Option<String>,
    /// Source title.
    pub source_title: Option<String>,
    /// Relevance score.
    pub score: f64,
    /// Approximate token count.
    pub token_count: usize,
    /// Optional embedding for dedup comparison.
    pub embedding: Option<Vec<f64>>,
    /// Optional parent context.
    pub parent_context: Option<String>,
}

/// Deduplicate items by cosine similarity of embeddings.
///
/// If two items have cosine similarity above the threshold, the
/// lower-scoring one is removed. Items without embeddings are
/// never considered duplicates.
fn deduplicate(items: &mut Vec<RawContextItem>, threshold: f64) -> usize {
    // Check if any items have embeddings at all.
    let has_embeddings = items.iter().any(|i| i.embedding.is_some());
    if !has_embeddings {
        return 0;
    }

    let mut removed = 0usize;
    let mut keep = vec![true; items.len()];

    for i in 0..items.len() {
        if !keep[i] {
            continue;
        }
        let Some(ref emb_a) = items[i].embedding else {
            continue;
        };
        for j in (i + 1)..items.len() {
            if !keep[j] {
                continue;
            }
            let Some(ref emb_b) = items[j].embedding else {
                continue;
            };
            // Skip comparison if embedding dimensions differ
            // (e.g. chunk=1024 vs node=256).
            if emb_a.len() != emb_b.len() {
                continue;
            }
            if cosine_similarity(emb_a, emb_b) > threshold {
                // Drop the lower-scoring item.
                if items[i].score >= items[j].score {
                    keep[j] = false;
                } else {
                    keep[i] = false;
                    break; // i is removed, stop comparing
                }
                removed += 1;
            }
        }
    }

    let mut idx = 0;
    items.retain(|_| {
        let k = keep[idx];
        idx += 1;
        k
    });

    removed
}

/// Cap items per source to enforce diversity.
fn diversify(items: &mut Vec<RawContextItem>, max_per_source: usize) {
    // Items should already be sorted by score descending before
    // this step, but we sort here to be safe.
    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut counts: HashMap<String, usize> = HashMap::new();
    items.retain(|item| {
        let key = item
            .source_id
            .as_deref()
            .unwrap_or("__no_source__")
            .to_string();
        let count = counts.entry(key).or_insert(0);
        if *count >= max_per_source {
            false
        } else {
            *count += 1;
            true
        }
    });
}

/// Assemble raw search results into an optimized context window.
///
/// Pipeline steps:
/// 1. **Deduplicate** — Merge near-duplicate results (cosine >
///    threshold)
/// 2. **Diversify** — Cap results per source (max_per_source)
/// 3. **Order** — Sort by relevance (highest first)
/// 4. **Budget** — Drop lowest-scoring items to fit within
///    max_tokens
/// 5. **Annotate** — Assign reference numbers \[1\], \[2\], etc.
pub fn assemble_context(items: Vec<RawContextItem>, config: &ContextConfig) -> AssembledContext {
    if items.is_empty() {
        return AssembledContext {
            items: Vec::new(),
            total_tokens: 0,
            items_dropped: 0,
            duplicates_removed: 0,
        };
    }

    let mut items = items;

    // 1. Deduplicate
    let duplicates_removed = deduplicate(&mut items, config.dedup_threshold);

    // 2. Order by score descending (before diversify needs it)
    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 3. Diversify
    diversify(&mut items, config.max_per_source);

    // 4. Budget — accumulate tokens, drop remainder
    let mut total_tokens = 0usize;
    let mut budget_cutoff = items.len();
    for (i, item) in items.iter().enumerate() {
        if total_tokens + item.token_count > config.max_tokens {
            budget_cutoff = i;
            break;
        }
        total_tokens += item.token_count;
    }
    let items_dropped = items.len() - budget_cutoff;
    items.truncate(budget_cutoff);

    // 5. Annotate — assign 1-indexed reference numbers
    let context_items: Vec<ContextItem> = items
        .into_iter()
        .enumerate()
        .map(|(i, raw)| ContextItem {
            ref_number: i + 1,
            content: raw.content,
            source_title: raw.source_title,
            source_id: raw.source_id,
            score: raw.score,
            token_count: raw.token_count,
            parent_context: raw.parent_context,
        })
        .collect();

    AssembledContext {
        items: context_items,
        total_tokens,
        items_dropped,
        duplicates_removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(
        content: &str,
        score: f64,
        tokens: usize,
        source_id: Option<&str>,
    ) -> RawContextItem {
        RawContextItem {
            content: content.to_string(),
            source_id: source_id.map(|s| s.to_string()),
            source_title: source_id.map(|s| s.to_string()),
            score,
            token_count: tokens,
            embedding: None,
            parent_context: None,
        }
    }

    #[test]
    fn assemble_empty() {
        let result = assemble_context(Vec::new(), &ContextConfig::default());
        assert!(result.items.is_empty());
        assert_eq!(result.total_tokens, 0);
        assert_eq!(result.items_dropped, 0);
        assert_eq!(result.duplicates_removed, 0);
    }

    #[test]
    fn assemble_budget_drops_lowest() {
        let config = ContextConfig {
            max_tokens: 500,
            max_per_source: 10,
            dedup_threshold: 0.95,
        };
        let items = vec![
            make_item("high", 0.9, 200, Some("a")),
            make_item("mid", 0.7, 200, Some("b")),
            make_item("low", 0.3, 200, Some("c")),
        ];
        let result = assemble_context(items, &config);
        // 200 + 200 = 400 fits, adding 200 more = 600 > 500
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total_tokens, 400);
        assert_eq!(result.items_dropped, 1);
        // The dropped item should be the lowest-scoring one.
        assert_eq!(result.items[0].content, "high");
        assert_eq!(result.items[1].content, "mid");
    }

    #[test]
    fn assemble_diversify_caps_per_source() {
        let config = ContextConfig {
            max_tokens: 100_000,
            max_per_source: 3,
            dedup_threshold: 0.95,
        };
        let items: Vec<RawContextItem> = (0..5)
            .map(|i| {
                make_item(
                    &format!("item-{i}"),
                    1.0 - (i as f64 * 0.1),
                    10,
                    Some("same_source"),
                )
            })
            .collect();
        let result = assemble_context(items, &config);
        assert_eq!(result.items.len(), 3);
        // Top 3 by score should be kept.
        assert_eq!(result.items[0].content, "item-0");
        assert_eq!(result.items[1].content, "item-1");
        assert_eq!(result.items[2].content, "item-2");
    }

    #[test]
    fn dedup_skips_mismatched_dimensions() {
        // Items with different embedding dimensions should not be
        // compared for deduplication.
        let mut items = vec![
            RawContextItem {
                content: "chunk content".to_string(),
                source_id: Some("a".to_string()),
                source_title: Some("Source A".to_string()),
                score: 0.9,
                token_count: 10,
                embedding: Some(vec![1.0; 1024]),
                parent_context: None,
            },
            RawContextItem {
                content: "node content".to_string(),
                source_id: Some("b".to_string()),
                source_title: Some("Source B".to_string()),
                score: 0.8,
                token_count: 10,
                // Different dimension — should not be compared.
                embedding: Some(vec![1.0; 256]),
                parent_context: None,
            },
        ];
        let removed = deduplicate(&mut items, 0.95);
        // Despite both being all-1.0 vectors (which would have
        // cosine similarity 1.0 if same dimension), no dedup
        // should occur because dimensions differ.
        assert_eq!(removed, 0);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn assemble_annotates_with_ref_numbers() {
        let config = ContextConfig::default();
        let items = vec![
            make_item("first", 0.9, 10, Some("a")),
            make_item("second", 0.8, 10, Some("b")),
            make_item("third", 0.7, 10, Some("c")),
        ];
        let result = assemble_context(items, &config);
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.items[0].ref_number, 1);
        assert_eq!(result.items[1].ref_number, 2);
        assert_eq!(result.items[2].ref_number, 3);
    }
}
