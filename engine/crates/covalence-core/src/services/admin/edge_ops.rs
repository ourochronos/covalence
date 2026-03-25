//! Edge synthesis and cross-domain bridging operations.

use crate::error::Result;
use crate::models::audit::{AuditAction, AuditLog};
use crate::storage::traits::{AdminRepo, AuditLogRepo, EdgeRepo};

use super::AdminService;

/// Result of co-occurrence edge synthesis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CooccurrenceResult {
    /// Number of synthetic edges created.
    pub edges_created: u64,
    /// Number of candidate pairs evaluated.
    pub candidates_evaluated: u64,
}

/// Result of code-to-concept bridge edge creation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeResult {
    /// Code-type nodes checked for bridging.
    pub code_nodes_checked: u64,
    /// New bridge edges created.
    pub edges_created: u64,
    /// Pairs skipped because an edge already exists.
    pub skipped_existing: u64,
}

impl AdminService {
    /// Synthesize co-occurrence edges from extraction provenance.
    ///
    /// Entities extracted from the same chunk co-occur in the source
    /// text. This method creates `co_occurs` edges between entity
    /// pairs that share at least `min_cooccurrences` chunks and where
    /// at least one entity has degree <= `max_degree` (poorly connected).
    ///
    /// Edges are marked `is_synthetic = true` with weight proportional
    /// to co-occurrence frequency. Existing edges (of any type) between
    /// the pair are respected — no duplicates are created.
    ///
    /// Returns counts of edges created vs skipped.
    pub async fn synthesize_cooccurrence_edges(
        &self,
        min_cooccurrences: i64,
        max_degree: i64,
    ) -> Result<CooccurrenceResult> {
        // Find co-occurring entity pairs via SP. The SP filters by
        // min co-occurrences, max degree, and excludes existing edges.
        let rows = AdminRepo::find_cooccurrence_pairs(
            &*self.repo,
            min_cooccurrences as i32,
            max_degree as i32,
        )
        .await?;

        let total_candidates = rows.len() as u64;
        let mut edges_created: u64 = 0;

        for (n1, n2, freq) in &rows {
            let source_id = crate::types::ids::NodeId::from_uuid(*n1);
            let target_id = crate::types::ids::NodeId::from_uuid(*n2);

            let mut edge =
                crate::models::edge::Edge::new(source_id, target_id, "co_occurs".to_string());
            edge.is_synthetic = true;
            // Weight: normalized co-occurrence frequency, capped at 1.0.
            edge.weight = (*freq as f64 / 5.0).min(1.0);
            // Confidence: proportional to frequency, lower baseline.
            edge.confidence = (0.3 + (*freq as f64 * 0.1)).min(0.9);
            edge.properties = serde_json::json!({
                "cooccurrence_count": freq,
                "synthesis_method": "extraction_provenance",
            });

            EdgeRepo::create(&*self.repo, &edge).await?;
            edges_created += 1;
        }

        if edges_created > 0 {
            tracing::info!(
                edges_created,
                total_candidates,
                min_cooccurrences,
                max_degree,
                "co-occurrence edge synthesis complete"
            );
            // Only reload if graph is active (has been loaded).
            // Workers run with an empty graph — reloading would
            // wastefully load the entire graph into memory.
            let stats = self.graph.stats().await?;
            if stats.node_count > 0 {
                self.graph.reload(self.repo.pool()).await?;
                tracing::info!("graph sidecar reloaded after edge synthesis");
            }
        } else {
            tracing::info!("co-occurrence synthesis: no new edges to create");
        }

        // Log the operation.
        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:synthesize_cooccurrence".to_string(),
            serde_json::json!({
                "edges_created": edges_created,
                "total_candidates": total_candidates,
                "min_cooccurrences": min_cooccurrences,
                "max_degree": max_degree,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(CooccurrenceResult {
            edges_created,
            candidates_evaluated: total_candidates,
        })
    }

    /// Create cross-domain bridge edges between code entities and
    /// prose concept nodes based on embedding similarity.
    ///
    /// Finds code-type nodes with embeddings and compares them against
    /// non-code nodes (concept, theory, method, etc.) using pgvector
    /// cosine distance. Creates `implements` edges for pairs above the
    /// similarity threshold, skipping pairs that already have an edge.
    pub async fn bridge_code_to_concepts(
        &self,
        min_similarity: f64,
        max_edges_per_node: i64,
    ) -> Result<BridgeResult> {
        let code_types: Vec<String> = self
            .config
            .as_ref()
            .map(|c| c.pipeline.code_node_types.clone())
            .unwrap_or_else(crate::config::PipelineConfig::default_code_node_types);

        // Fetch code nodes that have embeddings.
        let code_nodes =
            AdminRepo::list_code_nodes_with_embeddings(&*self.repo, &code_types).await?;

        if code_nodes.is_empty() {
            return Ok(BridgeResult {
                code_nodes_checked: 0,
                edges_created: 0,
                skipped_existing: 0,
            });
        }

        let code_nodes_checked = code_nodes.len() as u64;
        tracing::info!(code_nodes_checked, "bridging code nodes to concepts");

        let threshold = 1.0 - min_similarity; // cosine distance
        let mut edges_created = 0u64;
        let mut skipped_existing = 0u64;

        for (code_id, code_name, _code_type) in &code_nodes {
            // Find nearest non-code concept nodes by embedding similarity.
            let matches = AdminRepo::find_nearest_non_code_nodes(
                &*self.repo,
                *code_id,
                &code_types,
                max_edges_per_node,
            )
            .await?;

            for (concept_id, concept_name, dist) in &matches {
                if *dist > threshold {
                    break; // remaining will be worse
                }

                // Check if an edge already exists between these nodes.
                let exists =
                    AdminRepo::check_edge_exists(&*self.repo, *code_id, *concept_id, "implements")
                        .await?;

                if exists {
                    skipped_existing += 1;
                    continue;
                }

                let code_nid = crate::types::ids::NodeId::from_uuid(*code_id);
                let concept_nid = crate::types::ids::NodeId::from_uuid(*concept_id);
                let similarity = 1.0 - dist;

                let mut edge =
                    crate::models::edge::Edge::new(code_nid, concept_nid, "implements".to_string());
                edge.confidence = similarity;
                edge.properties = serde_json::json!({
                    "bridge_type": "code_to_concept",
                    "cosine_similarity": similarity,
                });

                EdgeRepo::create(&*self.repo, &edge).await?;
                edges_created += 1;

                tracing::debug!(
                    code = %code_name,
                    concept = %concept_name,
                    similarity = similarity,
                    "bridge edge created"
                );
            }
        }

        // Reload graph sidecar so new edges are visible to
        // graph-dimension searches.
        if edges_created > 0 {
            self.graph.reload(self.repo.pool()).await?;
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:bridge_code_to_concepts".to_string(),
            serde_json::json!({
                "code_nodes_checked": code_nodes_checked,
                "edges_created": edges_created,
                "skipped_existing": skipped_existing,
                "min_similarity": min_similarity,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(edges_created, skipped_existing, "bridge complete");
        Ok(BridgeResult {
            code_nodes_checked,
            edges_created,
            skipped_existing,
        })
    }
}
