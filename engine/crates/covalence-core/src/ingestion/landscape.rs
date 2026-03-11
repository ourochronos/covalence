//! Stage 6: Embedding landscape analysis.
//!
//! Analyzes parent-child alignment, adjacent similarity, and sibling
//! outlier scores to determine the extraction method for each chunk.
//! Embedding landscape topology decides which chunks warrant expensive
//! LLM extraction — not a blanket policy.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::ids::ChunkId;

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
/// * `is_first_ingestion` - Whether this source has no prior
///   extractions. On first ingestion, landscape gating is bypassed
///   and all chunks receive `FullExtraction` to avoid incorrectly
///   skipping extraction due to high intra-document parent-child
///   alignment.
///
/// # Returns
/// A [`ChunkLandscapeResult`] for each input chunk.
pub fn analyze_landscape(
    chunk_embeddings: &[Vec<f64>],
    parent_embeddings: &[Option<&Vec<f64>>],
    calibration: Option<&ModelCalibration>,
    is_first_ingestion: bool,
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

    // First-ingestion bypass: parent-child alignment within a
    // well-structured document is naturally high (0.78–0.91).
    // The P75 threshold incorrectly treats this as "redundant
    // with existing knowledge" even when there IS no existing
    // knowledge. On first ingestion, force FullExtraction for
    // all chunks and set graph_novelty = 1.0.
    if is_first_ingestion {
        tracing::debug!(
            chunks = chunk_embeddings.len(),
            "first ingestion — bypassing landscape gating, \
             forcing full extraction for all chunks"
        );
        for (i, chunk_emb) in chunk_embeddings.iter().enumerate() {
            let parent_alignment = parent_embeddings
                .get(i)
                .and_then(|pe| pe.as_ref())
                .map(|pe| cosine_similarity(chunk_emb, pe));
            results.push(ChunkLandscapeResult {
                chunk_index: i,
                parent_alignment,
                extraction_method: ExtractionMethod::FullExtraction,
                metrics: LandscapeMetrics {
                    adjacent_similarity: None,
                    sibling_outlier_score: None,
                    graph_novelty: Some(1.0),
                    flags: vec!["first_ingestion_bypass".to_string()],
                    valley_prominence: None,
                },
            });
        }
        return results;
    }

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
                graph_novelty: Some(1.0),
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

/// Default number of nearest neighbors for k-NN density.
const DEFAULT_KNN_K: usize = 5;

/// Per-chunk embedding landscape metrics computed after embedding.
///
/// These metrics capture the geometric relationship of each chunk's
/// embedding to its sibling chunks within the same source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkEmbeddingMetrics {
    /// Average cosine similarity to k-nearest sibling chunks.
    pub density: f64,
    /// 1.0 minus the maximum cosine similarity to any sibling chunk.
    pub uniqueness: f64,
    /// Shannon entropy of token (whitespace-split word) distribution.
    pub entropy: f64,
    /// Maximum cosine similarity to any sibling chunk (explicit
    /// complement of uniqueness).
    pub redundancy_score: f64,
    /// Cosine distance (1 - cos_sim) from the source centroid
    /// embedding.
    pub centroid_distance: f64,
}

/// Compute per-chunk landscape metrics for a batch of sibling
/// embeddings from the same source.
///
/// # Arguments
/// * `chunks` - Tuples of (chunk_id, embedding, text) for every
///   chunk in a single source.
/// * `k` - Number of nearest neighbors for density (clamped to
///   `chunks.len() - 1`).
///
/// Returns a mapping from [`ChunkId`] to the computed metrics as
/// a [`serde_json::Value`].
pub fn compute_chunk_landscape_metrics(
    chunks: &[(ChunkId, &[f64], &str)],
    k: usize,
) -> Vec<(ChunkId, serde_json::Value)> {
    if chunks.is_empty() {
        return Vec::new();
    }

    // Single chunk: metrics are trivially defined.
    if chunks.len() == 1 {
        let (id, _, text) = &chunks[0];
        let entropy = token_entropy(text);
        let metrics = ChunkEmbeddingMetrics {
            density: 1.0,
            uniqueness: 1.0,
            entropy,
            redundancy_score: 0.0,
            centroid_distance: 0.0,
        };
        let value = match serde_json::to_value(&metrics) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        return vec![(*id, value)];
    }

    // Compute centroid as element-wise mean of all embeddings.
    let dim = chunks[0].1.len();
    let mut centroid = vec![0.0f64; dim];
    for (_, emb, _) in chunks {
        for (c, &v) in centroid.iter_mut().zip(emb.iter()) {
            *c += v;
        }
    }
    let n = chunks.len() as f64;
    for c in &mut centroid {
        *c /= n;
    }

    // Precompute pairwise similarities.
    let len = chunks.len();
    let mut sim_matrix = vec![vec![0.0f64; len]; len];
    for i in 0..len {
        sim_matrix[i][i] = 1.0;
        for j in (i + 1)..len {
            let s = cosine_similarity(chunks[i].1, chunks[j].1);
            sim_matrix[i][j] = s;
            sim_matrix[j][i] = s;
        }
    }

    let effective_k = k.min(len - 1);

    let mut results = Vec::with_capacity(len);
    for i in 0..len {
        let (id, emb, text) = &chunks[i];

        // Gather similarities to all *other* chunks.
        let mut sims: Vec<f64> = (0..len)
            .filter(|&j| j != i)
            .map(|j| sim_matrix[i][j])
            .collect();
        sims.sort_unstable_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        // density: average of top-k similarities.
        let density: f64 = if effective_k > 0 {
            sims.iter().take(effective_k).copied().sum::<f64>() / effective_k as f64
        } else {
            0.0
        };

        // max similarity to any sibling.
        let max_sim = sims.first().copied().unwrap_or(0.0);
        let uniqueness = 1.0 - max_sim;
        let redundancy_score = max_sim;

        let centroid_sim = cosine_similarity(emb, &centroid);
        let centroid_distance = 1.0 - centroid_sim;

        let entropy = token_entropy(text);

        let metrics = ChunkEmbeddingMetrics {
            density,
            uniqueness,
            entropy,
            redundancy_score,
            centroid_distance,
        };
        if let Ok(value) = serde_json::to_value(&metrics) {
            results.push((*id, value));
        }
    }

    results
}

/// Convenience wrapper using the default k value.
pub fn compute_chunk_landscape_metrics_default(
    chunks: &[(ChunkId, &[f64], &str)],
) -> Vec<(ChunkId, serde_json::Value)> {
    compute_chunk_landscape_metrics(chunks, DEFAULT_KNN_K)
}

/// Compute Shannon entropy over whitespace-split token frequencies.
///
/// Returns 0.0 for empty text.
fn token_entropy(text: &str) -> f64 {
    let mut freq: HashMap<&str, usize> = HashMap::new();
    let mut total = 0usize;
    for word in text.split_whitespace() {
        *freq.entry(word).or_insert(0) += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    let total_f = total as f64;
    let mut entropy = 0.0f64;
    for &count in freq.values() {
        let p = count as f64 / total_f;
        entropy -= p * p.log2();
    }
    entropy
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
        // FullExtraction (source update path).
        let parent = vec![1.0, 0.5, 0.3];
        let child = vec![0.98, 0.52, 0.31];
        let results = analyze_landscape(&[child], &[Some(&parent)], None, false);
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
        let results = analyze_landscape(&[child], &[None], None, false);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction
        );
    }

    #[test]
    fn high_alignment_skips_extraction() {
        // Use 2 chunks so the single-chunk bypass doesn't trigger.
        // Source update path (not first ingestion).
        let parent = vec![1.0, 0.5, 0.3];
        let child1 = vec![0.98, 0.52, 0.31]; // high alignment
        let child2 = vec![0.95, 0.48, 0.29]; // also high alignment
        let results = analyze_landscape(
            &[child1, child2],
            &[Some(&parent), Some(&parent)],
            None,
            false,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::EmbeddingLinkage
        );
    }

    #[test]
    fn low_alignment_triggers_full_extraction() {
        // Use 2 chunks so the single-chunk bypass doesn't trigger.
        // Source update path (not first ingestion).
        let parent = vec![1.0, 0.0, 0.0];
        let child1 = vec![0.0, 1.0, 0.0]; // low alignment
        let child2 = vec![0.0, 0.0, 1.0]; // low alignment
        let results = analyze_landscape(
            &[child1, child2],
            &[Some(&parent), Some(&parent)],
            None,
            false,
        );
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
        let results = analyze_landscape(
            &[child1, child2],
            &[Some(&parent), Some(&parent)],
            None,
            false,
        );
        assert!(
            results[0]
                .metrics
                .flags
                .contains(&"low_parent_alignment".to_string())
        );
    }

    #[test]
    fn no_parent_uses_full_extraction() {
        // Without parent embeddings, landscape cannot judge
        // redundancy, so it defaults to full extraction.
        let child1 = vec![1.0, 0.5, 0.3];
        let child2 = vec![0.9, 0.4, 0.2];
        let results = analyze_landscape(&[child1, child2], &[None, None], None, false);
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
        let results = analyze_landscape(&chunks, &parents, None, false);
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
            false,
        );
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::EmbeddingLinkage
        );
    }

    #[test]
    fn empty_input() {
        let results: Vec<ChunkLandscapeResult> = analyze_landscape(&[], &[], None, false);
        assert!(results.is_empty());
    }

    #[test]
    fn empty_input_first_ingestion() {
        let results: Vec<ChunkLandscapeResult> = analyze_landscape(&[], &[], None, true);
        assert!(results.is_empty());
    }

    // --- First-ingestion bypass tests ---

    #[test]
    fn first_ingestion_forces_full_extraction_multi_chunk() {
        // On first ingestion, even high parent-child alignment
        // should NOT cause chunks to be skipped. This is the
        // core bug fix: intra-document alignment is naturally
        // high (0.78–0.91) and was being misinterpreted as
        // "redundant with existing knowledge."
        let parent = vec![1.0, 0.5, 0.3];
        let child1 = vec![0.98, 0.52, 0.31]; // high alignment
        let child2 = vec![0.95, 0.48, 0.29]; // also high
        let child3 = vec![0.90, 0.55, 0.28]; // also high
        let results = analyze_landscape(
            &[child1, child2, child3],
            &[Some(&parent), Some(&parent), Some(&parent)],
            None,
            true, // first ingestion
        );
        assert_eq!(results.len(), 3);
        for r in &results {
            assert_eq!(
                r.extraction_method,
                ExtractionMethod::FullExtraction,
                "chunk {} should be FullExtraction on first ingestion",
                r.chunk_index,
            );
            assert!(
                r.metrics
                    .flags
                    .contains(&"first_ingestion_bypass".to_string()),
                "chunk {} should have first_ingestion_bypass flag",
                r.chunk_index,
            );
            assert_eq!(
                r.metrics.graph_novelty,
                Some(1.0),
                "chunk {} should have graph_novelty = 1.0",
                r.chunk_index,
            );
        }
    }

    #[test]
    fn first_ingestion_single_chunk() {
        // First ingestion + single chunk: both bypasses apply,
        // first_ingestion_bypass takes precedence since it's
        // checked first.
        let parent = vec![1.0, 0.5, 0.3];
        let child = vec![0.98, 0.52, 0.31];
        let results = analyze_landscape(&[child], &[Some(&parent)], None, true);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction
        );
        assert!(
            results[0]
                .metrics
                .flags
                .contains(&"first_ingestion_bypass".to_string())
        );
        assert_eq!(results[0].metrics.graph_novelty, Some(1.0));
    }

    #[test]
    fn first_ingestion_preserves_parent_alignment() {
        // Even though first ingestion bypasses gating, it should
        // still compute and store parent_alignment for metrics.
        let parent = vec![1.0, 0.0, 0.0];
        let child1 = vec![1.0, 0.0, 0.0]; // perfect alignment
        let child2 = vec![0.0, 1.0, 0.0]; // orthogonal
        let results = analyze_landscape(
            &[child1, child2],
            &[Some(&parent), Some(&parent)],
            None,
            true,
        );
        assert_eq!(results.len(), 2);
        // First chunk: perfect alignment with parent
        assert!(
            results[0]
                .parent_alignment
                .is_some_and(|pa| (pa - 1.0).abs() < 1e-10)
        );
        // Second chunk: orthogonal to parent
        assert!(
            results[1]
                .parent_alignment
                .is_some_and(|pa| pa.abs() < 1e-10)
        );
        // Both should still be FullExtraction
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::FullExtraction
        );
        assert_eq!(
            results[1].extraction_method,
            ExtractionMethod::FullExtraction
        );
    }

    #[test]
    fn first_ingestion_no_parent_embeddings() {
        // First ingestion without parent embeddings. Should still
        // force FullExtraction.
        let child1 = vec![1.0, 0.5, 0.3];
        let child2 = vec![0.9, 0.4, 0.2];
        let results = analyze_landscape(&[child1, child2], &[None, None], None, true);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.extraction_method, ExtractionMethod::FullExtraction);
            assert!(r.parent_alignment.is_none());
            assert_eq!(r.metrics.graph_novelty, Some(1.0));
        }
    }

    #[test]
    fn source_update_high_alignment_skips() {
        // Contrast with first_ingestion: same high-alignment
        // embeddings on a source update (not first ingestion)
        // should be gated as EmbeddingLinkage.
        let parent = vec![1.0, 0.5, 0.3];
        let child1 = vec![0.98, 0.52, 0.31];
        let child2 = vec![0.95, 0.48, 0.29];
        let results = analyze_landscape(
            &[child1, child2],
            &[Some(&parent), Some(&parent)],
            None,
            false, // source update
        );
        assert_eq!(
            results[0].extraction_method,
            ExtractionMethod::EmbeddingLinkage,
            "source update with high alignment should skip"
        );
    }

    // --- Tests for compute_chunk_landscape_metrics ---

    #[test]
    fn chunk_metrics_empty_input() {
        let results = compute_chunk_landscape_metrics(&[], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn chunk_metrics_single_chunk() {
        let id = ChunkId::new();
        let emb = vec![1.0, 0.0, 0.0];
        let text = "hello world hello";
        let chunks = vec![(id, emb.as_slice(), text)];

        let results = compute_chunk_landscape_metrics(&chunks, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, id);

        let m: ChunkEmbeddingMetrics =
            serde_json::from_value(results[0].1.clone()).expect("valid json");
        assert!((m.density - 1.0).abs() < 1e-10);
        assert!((m.uniqueness - 1.0).abs() < 1e-10);
        assert!((m.redundancy_score - 0.0).abs() < 1e-10);
        assert!((m.centroid_distance - 0.0).abs() < 1e-10);
        // Entropy should be > 0 for text with multiple distinct tokens
        assert!(m.entropy > 0.0);
    }

    #[test]
    fn chunk_metrics_identical_embeddings() {
        let id1 = ChunkId::new();
        let id2 = ChunkId::new();
        let emb = vec![1.0, 0.0, 0.0];
        let chunks = vec![
            (id1, emb.as_slice(), "foo bar"),
            (id2, emb.as_slice(), "baz qux"),
        ];

        let results = compute_chunk_landscape_metrics(&chunks, 5);
        assert_eq!(results.len(), 2);

        let m: ChunkEmbeddingMetrics =
            serde_json::from_value(results[0].1.clone()).expect("valid json");
        // Identical embeddings: max similarity = 1.0
        assert!((m.redundancy_score - 1.0).abs() < 1e-10);
        assert!(m.uniqueness.abs() < 1e-10);
        // Density with 1 neighbor = 1.0
        assert!((m.density - 1.0).abs() < 1e-10);
        // Same as centroid, so distance = 0
        assert!(m.centroid_distance.abs() < 1e-10);
    }

    #[test]
    fn chunk_metrics_orthogonal_embeddings() {
        let id1 = ChunkId::new();
        let id2 = ChunkId::new();
        let id3 = ChunkId::new();
        let e1 = vec![1.0, 0.0, 0.0];
        let e2 = vec![0.0, 1.0, 0.0];
        let e3 = vec![0.0, 0.0, 1.0];
        let chunks = vec![
            (id1, e1.as_slice(), "alpha"),
            (id2, e2.as_slice(), "beta"),
            (id3, e3.as_slice(), "gamma"),
        ];

        let results = compute_chunk_landscape_metrics(&chunks, 5);
        assert_eq!(results.len(), 3);

        let m: ChunkEmbeddingMetrics =
            serde_json::from_value(results[0].1.clone()).expect("valid json");
        // Orthogonal vectors: similarity = 0 to all others
        assert!(m.density.abs() < 1e-10);
        assert!((m.uniqueness - 1.0).abs() < 1e-10);
        assert!(m.redundancy_score.abs() < 1e-10);
        // Centroid is (1/3, 1/3, 1/3), cos_sim with (1,0,0) =
        // 1/3 / (1 * sqrt(1/3)) = 1/sqrt(3)
        let expected_sim = 1.0 / 3.0_f64.sqrt();
        let expected_dist = 1.0 - expected_sim;
        assert!((m.centroid_distance - expected_dist).abs() < 1e-6);
    }

    #[test]
    fn chunk_metrics_knn_clamps_to_available() {
        // k=5 but only 2 other chunks available
        let id1 = ChunkId::new();
        let id2 = ChunkId::new();
        let id3 = ChunkId::new();
        let e1 = vec![1.0, 0.0];
        let e2 = vec![0.8, 0.6];
        let e3 = vec![0.0, 1.0];
        let chunks = vec![
            (id1, e1.as_slice(), "one"),
            (id2, e2.as_slice(), "two"),
            (id3, e3.as_slice(), "three"),
        ];

        let results = compute_chunk_landscape_metrics(&chunks, 5);
        // Should still work — k gets clamped to 2
        assert_eq!(results.len(), 3);
        let m: ChunkEmbeddingMetrics =
            serde_json::from_value(results[0].1.clone()).expect("valid json");
        // density = average of 2 similarities
        let sim_12 = cosine_similarity(&e1, &e2);
        let sim_13 = cosine_similarity(&e1, &e3);
        let expected = (sim_12 + sim_13) / 2.0;
        assert!((m.density - expected).abs() < 1e-6);
    }

    #[test]
    fn token_entropy_empty_text() {
        assert!((token_entropy("") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn token_entropy_single_token() {
        // All same token: p=1, entropy = -1*log2(1) = 0
        assert!((token_entropy("a a a a") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn token_entropy_uniform_tokens() {
        // 4 distinct tokens each appearing once:
        // H = -4*(1/4 * log2(1/4)) = 2.0
        let h = token_entropy("a b c d");
        assert!((h - 2.0).abs() < 1e-10);
    }

    #[test]
    fn chunk_metrics_default_k() {
        let id = ChunkId::new();
        let emb = vec![1.0, 0.0];
        let chunks = vec![(id, emb.as_slice(), "word")];
        let results = compute_chunk_landscape_metrics_default(&chunks);
        assert_eq!(results.len(), 1);
    }
}
