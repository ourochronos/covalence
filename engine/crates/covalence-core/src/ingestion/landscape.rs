//! Stage 6: Embedding landscape analysis.
//!
//! Analyzes parent-child alignment, adjacent similarity, and sibling
//! outlier scores to determine the extraction method for each chunk.
//! Embedding landscape topology decides which chunks warrant expensive
//! LLM extraction — not a blanket policy.

use serde::{Deserialize, Serialize};

/// Calibration statistics for a specific embedding model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCalibration {
    /// Model name (e.g., "voyage-context-3").
    pub model_name: String,
    /// 25th percentile of parent-child alignment scores.
    pub parent_child_p25: f64,
    /// 50th percentile (median) of parent-child alignment scores.
    pub parent_child_p50: f64,
    /// 75th percentile of parent-child alignment scores.
    pub parent_child_p75: f64,
    /// Mean adjacent chunk similarity.
    pub adjacent_mean: f64,
    /// Standard deviation of adjacent chunk similarity.
    pub adjacent_stddev: f64,
    /// Number of samples used for calibration.
    pub sample_size: usize,
}

/// Landscape metrics computed for a single chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandscapeMetrics {
    /// Cosine similarity between this chunk and its adjacent sibling.
    pub adjacent_similarity: Option<f64>,
    /// How much this chunk deviates from its siblings' mean alignment.
    pub sibling_outlier_score: Option<f64>,
    /// Whether this chunk introduces graph-novel entities.
    pub graph_novelty: Option<f64>,
    /// Detection flags from the analysis.
    pub flags: Vec<String>,
    /// Valley prominence (depth relative to neighbors).
    pub valley_prominence: Option<f64>,
}

/// Result of landscape analysis for a chunk.
#[derive(Debug, Clone)]
pub struct ChunkLandscapeResult {
    /// Chunk ID (index in the batch, not database ID).
    pub chunk_index: usize,
    /// Parent-child alignment score (cosine similarity).
    pub parent_alignment: Option<f64>,
    /// Determined extraction method.
    pub extraction_method: ExtractionMethod,
    /// Detailed metrics.
    pub metrics: LandscapeMetrics,
}

/// Extraction method determined by landscape analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    /// Skip extraction — embedding linkage is sufficient.
    EmbeddingLinkage,
    /// Quick delta check against existing graph.
    DeltaCheck,
    /// Full entity/relationship extraction.
    FullExtraction,
    /// Full extraction with second-pass review (gleaning).
    FullExtractionWithReview,
}

impl ExtractionMethod {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmbeddingLinkage => "embedding_linkage",
            Self::DeltaCheck => "delta_check",
            Self::FullExtraction => "full_extraction",
            Self::FullExtractionWithReview => "full_extraction_with_review",
        }
    }
}

/// Compute cosine similarity between two embedding vectors.
///
/// Returns 0.0 for mismatched lengths, empty vectors, or zero-norm
/// vectors.
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Analyze the embedding landscape for a batch of chunks.
///
/// Takes chunk embeddings organized by parent, plus optional parent
/// embeddings, and determines the extraction method for each chunk.
///
/// # Arguments
/// * `chunk_embeddings` - Ordered list of chunk embeddings
/// * `parent_embeddings` - Parent embedding per chunk (`None` for
///   document-level)
/// * `calibration` - Model-specific calibration statistics (`None` =
///   use defaults)
///
/// # Returns
/// A [`ChunkLandscapeResult`] for each input chunk.
pub fn analyze_landscape(
    chunk_embeddings: &[Vec<f64>],
    parent_embeddings: &[Option<&Vec<f64>>],
    calibration: Option<&ModelCalibration>,
) -> Vec<ChunkLandscapeResult> {
    let default_cal = ModelCalibration {
        model_name: "default".to_string(),
        parent_child_p25: 0.65,
        parent_child_p50: 0.75,
        parent_child_p75: 0.85,
        adjacent_mean: 0.70,
        adjacent_stddev: 0.12,
        sample_size: 0,
    };
    let cal = calibration.unwrap_or(&default_cal);

    let mut results = Vec::with_capacity(chunk_embeddings.len());

    // Single-chunk sources: the one chunk IS the entire document,
    // so parent-child alignment is meaningless (≈1.0). Always
    // extract to avoid silently skipping small documents.
    if chunk_embeddings.len() == 1 {
        tracing::debug!("single-chunk source — bypassing landscape, forcing full extraction");
        let parent_alignment =
            parent_embeddings[0].map(|pe| cosine_similarity(&chunk_embeddings[0], pe));
        return vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment,
            extraction_method: ExtractionMethod::FullExtraction,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec!["single_chunk_bypass".to_string()],
                valley_prominence: None,
            },
        }];
    }

    for i in 0..chunk_embeddings.len() {
        // 1. Parent-child alignment
        let parent_alignment =
            parent_embeddings[i].map(|pe| cosine_similarity(&chunk_embeddings[i], pe));

        // 2. Adjacent similarity
        let adjacent_similarity = if i + 1 < chunk_embeddings.len() {
            Some(cosine_similarity(
                &chunk_embeddings[i],
                &chunk_embeddings[i + 1],
            ))
        } else {
            None
        };

        // 3. Sibling outlier score (deviation from median)
        let sibling_outlier_score = parent_alignment.map(|pa| (pa - cal.parent_child_p50).abs());

        // 4. Valley detection
        let valley_prominence = if i > 0 && i + 1 < chunk_embeddings.len() {
            let prev_sim = cosine_similarity(&chunk_embeddings[i - 1], &chunk_embeddings[i]);
            let next_sim = cosine_similarity(&chunk_embeddings[i], &chunk_embeddings[i + 1]);
            let avg_neighbor = (prev_sim + next_sim) / 2.0;
            adjacent_similarity.map(|as_| {
                if as_ < avg_neighbor {
                    avg_neighbor - as_
                } else {
                    0.0
                }
            })
        } else {
            None
        };

        // 5. Determine extraction method
        let extraction_method = determine_extraction_method(
            i,
            parent_alignment,
            sibling_outlier_score,
            valley_prominence,
            cal,
        );

        let mut flags = Vec::new();
        if parent_alignment.is_some_and(|pa| pa < cal.parent_child_p25) {
            flags.push("low_parent_alignment".to_string());
        }
        if valley_prominence.is_some_and(|vp| vp > cal.adjacent_stddev) {
            flags.push("topic_boundary".to_string());
        }
        if sibling_outlier_score.is_some_and(|so| so > 2.0 * cal.adjacent_stddev) {
            flags.push("sibling_outlier".to_string());
        }

        results.push(ChunkLandscapeResult {
            chunk_index: i,
            parent_alignment,
            extraction_method,
            metrics: LandscapeMetrics {
                adjacent_similarity,
                sibling_outlier_score,
                graph_novelty: None,
                flags,
                valley_prominence,
            },
        });
    }

    results
}

/// Determine the extraction method based on landscape metrics.
fn determine_extraction_method(
    chunk_index: usize,
    parent_alignment: Option<f64>,
    sibling_outlier_score: Option<f64>,
    valley_prominence: Option<f64>,
    cal: &ModelCalibration,
) -> ExtractionMethod {
    // No parent alignment data (first ingestion, or no parent
    // embeddings available). Without a reference point we cannot
    // judge redundancy, so default to full extraction.
    let Some(pa) = parent_alignment else {
        tracing::debug!(
            chunk_index,
            "landscape: full extraction (no parent alignment)"
        );
        return ExtractionMethod::FullExtraction;
    };

    // High alignment with parent = redundant, skip extraction
    if pa > cal.parent_child_p75 {
        tracing::debug!(
            chunk_index,
            alignment = pa,
            threshold = cal.parent_child_p75,
            "landscape: skipping chunk (high parent alignment)"
        );
        return ExtractionMethod::EmbeddingLinkage;
    }

    // Very low alignment = highly novel content
    if pa < cal.parent_child_p25 {
        if sibling_outlier_score.is_some_and(|so| so > 2.0 * cal.adjacent_stddev) {
            tracing::debug!(
                chunk_index,
                alignment = pa,
                "landscape: full extraction with review (low alignment + outlier)"
            );
            return ExtractionMethod::FullExtractionWithReview;
        }
        tracing::debug!(
            chunk_index,
            alignment = pa,
            "landscape: full extraction (low parent alignment)"
        );
        return ExtractionMethod::FullExtraction;
    }

    // Topic boundary (valley) = potential new topic, extract
    if valley_prominence.is_some_and(|vp| vp > cal.adjacent_stddev) {
        tracing::debug!(
            chunk_index,
            valley_prominence,
            "landscape: full extraction (topic boundary)"
        );
        return ExtractionMethod::FullExtraction;
    }

    // Middle range = delta check
    tracing::debug!(
        chunk_index,
        alignment = pa,
        "landscape: delta check (moderate alignment)"
    );
    ExtractionMethod::DeltaCheck
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-10);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn single_chunk_always_extracts() {
        // A single chunk IS the entire document. Even with perfect
        // parent alignment, landscape should bypass and force
        // FullExtraction.
        let parent = vec![1.0, 0.5, 0.3];
        let child = vec![0.98, 0.52, 0.31];
        let results = analyze_landscape(&[child], &[Some(&parent)], None);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction
        );
        assert!(
            results[0]
                .metrics
                .flags
                .contains(&"single_chunk_bypass".to_string())
        );
    }

    #[test]
    fn single_chunk_no_parent_still_extracts() {
        let child = vec![1.0, 0.5, 0.3];
        let results = analyze_landscape(&[child], &[None], None);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction
        );
    }

    #[test]
    fn high_alignment_skips_extraction() {
        // Use 2 chunks so the single-chunk bypass doesn't trigger.
        let parent = vec![1.0, 0.5, 0.3];
        let child1 = vec![0.98, 0.52, 0.31]; // high alignment
        let child2 = vec![0.95, 0.48, 0.29]; // also high alignment
        let results = analyze_landscape(&[child1, child2], &[Some(&parent), Some(&parent)], None);
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::EmbeddingLinkage
        );
    }

    #[test]
    fn low_alignment_triggers_full_extraction() {
        // Use 2 chunks so the single-chunk bypass doesn't trigger.
        let parent = vec![1.0, 0.0, 0.0];
        let child1 = vec![0.0, 1.0, 0.0]; // low alignment
        let child2 = vec![0.0, 0.0, 1.0]; // low alignment
        let results = analyze_landscape(&[child1, child2], &[Some(&parent), Some(&parent)], None);
        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction | ExtractionMethod::FullExtractionWithReview
        ));
    }

    #[test]
    fn low_alignment_sets_flag() {
        let parent = vec![1.0, 0.0, 0.0];
        let child1 = vec![0.0, 1.0, 0.0];
        let child2 = vec![0.0, 0.0, 1.0];
        let results = analyze_landscape(&[child1, child2], &[Some(&parent), Some(&parent)], None);
        assert!(
            results[0]
                .metrics
                .flags
                .contains(&"low_parent_alignment".to_string())
        );
    }

    #[test]
    fn no_parent_uses_full_extraction() {
        // Without parent embeddings (first ingestion), landscape
        // cannot judge redundancy, so it defaults to full extraction.
        let child1 = vec![1.0, 0.5, 0.3];
        let child2 = vec![0.9, 0.4, 0.2];
        let results = analyze_landscape(&[child1, child2], &[None, None], None);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction
        );
    }

    #[test]
    fn extraction_method_as_str() {
        assert_eq!(
            ExtractionMethod::EmbeddingLinkage.as_str(),
            "embedding_linkage"
        );
        assert_eq!(ExtractionMethod::DeltaCheck.as_str(), "delta_check");
        assert_eq!(ExtractionMethod::FullExtraction.as_str(), "full_extraction");
        assert_eq!(
            ExtractionMethod::FullExtractionWithReview.as_str(),
            "full_extraction_with_review"
        );
    }

    #[test]
    fn multi_chunk_adjacent_similarity() {
        let chunks = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.9, 0.1, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        let parents: Vec<Option<&Vec<f64>>> = vec![None, None, None];
        let results = analyze_landscape(&chunks, &parents, None);
        assert_eq!(results.len(), 3);
        // First two are similar, so adjacent_similarity should
        // be high
        assert!(
            results[0]
                .metrics
                .adjacent_similarity
                .is_some_and(|s| s > 0.9)
        );
        // Second to third are very different
        assert!(
            results[1]
                .metrics
                .adjacent_similarity
                .is_some_and(|s| s < 0.2)
        );
        // Last chunk has no next sibling
        assert!(results[2].metrics.adjacent_similarity.is_none());
    }

    #[test]
    fn custom_calibration() {
        let cal = ModelCalibration {
            model_name: "test-model".to_string(),
            parent_child_p25: 0.30,
            parent_child_p50: 0.50,
            parent_child_p75: 0.70,
            adjacent_mean: 0.60,
            adjacent_stddev: 0.15,
            sample_size: 100,
        };
        // With lower p75 threshold, moderate alignment now skips.
        // Use 2 chunks to avoid single-chunk bypass.
        let parent = vec![1.0, 0.5, 0.3];
        let child1 = vec![0.98, 0.52, 0.31];
        let child2 = vec![0.95, 0.48, 0.29];
        let results = analyze_landscape(
            &[child1, child2],
            &[Some(&parent), Some(&parent)],
            Some(&cal),
        );
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::EmbeddingLinkage
        );
    }

    #[test]
    fn empty_input() {
        let results: Vec<ChunkLandscapeResult> = analyze_landscape(&[], &[], None);
        assert!(results.is_empty());
    }
}
