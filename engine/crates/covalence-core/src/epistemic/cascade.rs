//! TMS epistemic cascade — re-evaluate beliefs when support changes.
//!
//! Implements dependency-directed backtracking from spec 07: when a
//! source is retracted or deleted, claims that lost support have their
//! opinions recalculated from remaining evidence.
//!
//! The cascade operates in two phases:
//! 1. **Direct recalculation** — affected nodes/edges have their
//!    opinions re-fused from remaining active extractions.
//! 2. **Transitive propagation** — edges involving affected nodes
//!    are checked and their endpoint opinions propagated.

use crate::error::Result;
use crate::models::extraction::Extraction;
use crate::storage::traits::{EdgeRepo, ExtractionRepo, NodeRepo};
use crate::types::ids::{EdgeId, NodeId};
use crate::types::opinion::Opinion;

use super::fusion::cumulative_fuse;

/// Result of an epistemic cascade operation.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CascadeResult {
    /// Nodes whose opinions were recalculated from remaining support.
    pub nodes_recalculated: usize,
    /// Nodes set to vacuous opinion (lost all extraction support).
    pub nodes_vacuated: usize,
    /// Edges whose opinions were recalculated from remaining support.
    pub edges_recalculated: usize,
    /// Edges set to vacuous opinion (lost all extraction support).
    pub edges_vacuated: usize,
}

impl CascadeResult {
    /// Total number of entities affected by the cascade.
    pub fn total_affected(&self) -> usize {
        self.nodes_recalculated
            + self.nodes_vacuated
            + self.edges_recalculated
            + self.edges_vacuated
    }

    /// Merge another cascade result into this one.
    pub fn merge(&mut self, other: &CascadeResult) {
        self.nodes_recalculated += other.nodes_recalculated;
        self.nodes_vacuated += other.nodes_vacuated;
        self.edges_recalculated += other.edges_recalculated;
        self.edges_vacuated += other.edges_vacuated;
    }
}

/// Recalculate opinions for nodes that lost extraction support.
///
/// For each node:
/// - Zero active extractions remain → opinion set to vacuous
///   (b=0, d=0, u=1, a=0.5), implementing the "stale" marker
///   from the epistemic spec.
/// - Extractions remain → opinions fused from remaining extraction
///   confidences via cumulative fusion.
///
/// Call this AFTER deleting or superseding extractions from a
/// retracted source.
pub async fn recalculate_node_opinions<R>(repo: &R, node_ids: &[NodeId]) -> Result<CascadeResult>
where
    R: ExtractionRepo + NodeRepo + Sync,
{
    let mut result = CascadeResult::default();

    for &node_id in node_ids {
        let remaining =
            ExtractionRepo::list_active_for_entity(repo, "node", node_id.into_uuid()).await?;

        let Some(mut node) = NodeRepo::get(repo, node_id).await? else {
            continue;
        };

        if remaining.is_empty() {
            // Node lost all support — mark stale via vacuous opinion.
            node.confidence_breakdown = Some(Opinion::vacuous(0.5));
            NodeRepo::update(repo, &node).await?;
            result.nodes_vacuated += 1;
            tracing::debug!(
                node_id = %node_id,
                name = %node.canonical_name,
                "node opinion set to vacuous (all support retracted)"
            );
        } else {
            // Recalculate from remaining extraction evidence.
            let fused = fuse_extraction_confidences(&remaining);
            node.confidence_breakdown = Some(fused);
            NodeRepo::update(repo, &node).await?;
            result.nodes_recalculated += 1;
            tracing::debug!(
                node_id = %node_id,
                name = %node.canonical_name,
                remaining_extractions = remaining.len(),
                new_belief = fused.belief,
                new_uncertainty = fused.uncertainty,
                "node opinion recalculated from remaining support"
            );
        }
    }

    Ok(result)
}

/// Recalculate opinions for edges that lost extraction support.
///
/// Same logic as [`recalculate_node_opinions`] but for edge entities.
/// Also updates the scalar `confidence` field to match the new
/// projected probability.
pub async fn recalculate_edge_opinions<R>(repo: &R, edge_ids: &[EdgeId]) -> Result<CascadeResult>
where
    R: ExtractionRepo + EdgeRepo + Sync,
{
    let mut result = CascadeResult::default();

    for &edge_id in edge_ids {
        let remaining =
            ExtractionRepo::list_active_for_entity(repo, "edge", edge_id.into_uuid()).await?;

        let Some(mut edge) = EdgeRepo::get(repo, edge_id).await? else {
            continue;
        };

        if remaining.is_empty() {
            // Edge lost all support — mark stale.
            let vacuous = Opinion::vacuous(0.5);
            edge.confidence = vacuous.projected_probability();
            edge.confidence_breakdown = Some(vacuous);
            EdgeRepo::update(repo, &edge).await?;
            result.edges_vacuated += 1;
            tracing::debug!(
                edge_id = %edge_id,
                rel_type = %edge.rel_type,
                "edge opinion set to vacuous (all support retracted)"
            );
        } else {
            // Recalculate from remaining extraction evidence.
            let fused = fuse_extraction_confidences(&remaining);
            edge.confidence = fused.projected_probability();
            edge.confidence_breakdown = Some(fused);
            EdgeRepo::update(repo, &edge).await?;
            result.edges_recalculated += 1;
            tracing::debug!(
                edge_id = %edge_id,
                rel_type = %edge.rel_type,
                remaining_extractions = remaining.len(),
                new_confidence = edge.confidence,
                "edge opinion recalculated from remaining support"
            );
        }
    }

    Ok(result)
}

/// Convert extraction confidences to opinions and fuse them.
///
/// Each extraction's scalar confidence is mapped to an Opinion:
/// - `belief = confidence`
/// - `disbelief = (1 - confidence) × 0.3` — partial disbelief
/// - `uncertainty = 1 - belief - disbelief` — remaining ignorance
/// - `base_rate = 0.5` — uninformative prior
///
/// Multiple extractions are fused via Subjective Logic cumulative
/// fusion, which reduces uncertainty when independent sources agree
/// on the same claim.
fn fuse_extraction_confidences(extractions: &[Extraction]) -> Opinion {
    if extractions.is_empty() {
        return Opinion::vacuous(0.5);
    }

    let opinions: Vec<Opinion> = extractions
        .iter()
        .filter_map(|e| {
            let b = e.confidence.clamp(0.0, 1.0);
            let d = (1.0 - b) * 0.3;
            let u = 1.0 - b - d;
            Opinion::new(b, d, u, 0.5)
        })
        .collect();

    if opinions.is_empty() {
        return Opinion::vacuous(0.5);
    }

    if opinions.len() == 1 {
        return opinions[0];
    }

    // Cumulative fusion reduces uncertainty when multiple
    // independent sources agree on the same claim.
    let mut result = opinions[0];
    for op in &opinions[1..] {
        if let Some(fused) = cumulative_fuse(&result, op) {
            result = fused;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ids::ChunkId;

    #[test]
    fn fuse_empty_returns_vacuous() {
        let result = fuse_extraction_confidences(&[]);
        assert!((result.uncertainty - 1.0).abs() < 1e-6);
        assert!((result.belief).abs() < 1e-6);
    }

    #[test]
    fn fuse_single_extraction() {
        let ext = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.8,
        );
        let result = fuse_extraction_confidences(&[ext]);
        assert!((result.belief - 0.8).abs() < 1e-6);
        // disbelief = (1 - 0.8) * 0.3 = 0.06
        assert!((result.disbelief - 0.06).abs() < 1e-6);
        // uncertainty = 1 - 0.8 - 0.06 = 0.14
        assert!((result.uncertainty - 0.14).abs() < 1e-6);
    }

    #[test]
    fn fuse_multiple_reduces_uncertainty() {
        let ext1 = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.7,
        );
        let ext2 = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.8,
        );

        let single = fuse_extraction_confidences(&[ext1.clone()]);
        let fused = fuse_extraction_confidences(&[ext1, ext2]);

        // Cumulative fusion should reduce uncertainty
        assert!(fused.uncertainty < single.uncertainty);
    }

    #[test]
    fn fuse_preserves_opinion_constraint() {
        let ext1 = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.9,
        );
        let ext2 = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.6,
        );

        let result = fuse_extraction_confidences(&[ext1, ext2]);
        let sum = result.belief + result.disbelief + result.uncertainty;
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "opinion constraint violated: b+d+u = {sum}"
        );
    }

    #[test]
    fn cascade_result_merge() {
        let mut a = CascadeResult {
            nodes_recalculated: 3,
            nodes_vacuated: 1,
            edges_recalculated: 2,
            edges_vacuated: 0,
        };
        let b = CascadeResult {
            nodes_recalculated: 0,
            nodes_vacuated: 2,
            edges_recalculated: 1,
            edges_vacuated: 1,
        };
        a.merge(&b);
        assert_eq!(a.nodes_recalculated, 3);
        assert_eq!(a.nodes_vacuated, 3);
        assert_eq!(a.edges_recalculated, 3);
        assert_eq!(a.edges_vacuated, 1);
        assert_eq!(a.total_affected(), 10);
    }

    #[test]
    fn cascade_result_total_affected() {
        let r = CascadeResult {
            nodes_recalculated: 5,
            nodes_vacuated: 2,
            edges_recalculated: 3,
            edges_vacuated: 1,
        };
        assert_eq!(r.total_affected(), 11);
    }

    #[test]
    fn fuse_high_confidence_extractions() {
        // Two high-confidence extractions should produce
        // near-certain opinion.
        let ext1 = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.95,
        );
        let ext2 = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.92,
        );

        let result = fuse_extraction_confidences(&[ext1, ext2]);
        assert!(result.belief > 0.9);
        assert!(result.uncertainty < 0.05);
    }

    #[test]
    fn fuse_low_confidence_extractions() {
        let ext = Extraction::new(
            ChunkId::new(),
            crate::models::extraction::ExtractedEntityType::Node,
            uuid::Uuid::new_v4(),
            "test".into(),
            0.3,
        );

        let result = fuse_extraction_confidences(&[ext]);
        assert!((result.belief - 0.3).abs() < 1e-6);
        // disbelief = 0.7 * 0.3 = 0.21
        assert!((result.disbelief - 0.21).abs() < 1e-6);
        // uncertainty = 1 - 0.3 - 0.21 = 0.49
        assert!((result.uncertainty - 0.49).abs() < 1e-6);
    }
}
