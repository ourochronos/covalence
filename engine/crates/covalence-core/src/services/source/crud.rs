//! CRUD operations for sources — get, list, delete, publish, count.

use crate::error::{Error, Result};
use crate::models::source::Source;
use crate::storage::traits::{
    ChunkRepo, EdgeRepo, ExtractionRepo, LedgerRepo, NodeAliasRepo, NodeRepo, SectionRepo,
    SourceRepo, StatementRepo, UnresolvedEntityRepo,
};
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{NodeId, SourceId};

use super::SourceService;
use super::epistemic_cascade;

/// Result of a cascading delete operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteResult {
    /// Whether the source was found and deleted.
    pub deleted: bool,
    /// Number of chunks deleted alongside the source.
    pub chunks_deleted: u64,
    /// Number of extraction records deleted.
    pub extractions_deleted: u64,
    /// Number of orphaned nodes deleted.
    pub nodes_deleted: u64,
    /// Number of edges deleted (from orphaned nodes).
    pub edges_deleted: u64,
    /// Number of statements deleted (statement-first pipeline).
    pub statements_deleted: u64,
    /// Number of sections deleted (statement-first pipeline).
    pub sections_deleted: u64,
    /// Number of surviving nodes whose opinions were recalculated.
    pub nodes_recalculated: usize,
    /// Number of surviving edges whose opinions were recalculated.
    pub edges_recalculated: usize,
}

impl SourceService {
    /// Get a source by ID.
    pub async fn get(&self, id: SourceId) -> Result<Option<Source>> {
        SourceRepo::get(&*self.repo, id).await
    }

    /// List sources with pagination.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Source>> {
        SourceRepo::list(&*self.repo, limit, offset).await
    }

    /// Delete a source and cascade through all dependent entities.
    ///
    /// Cascade order:
    /// 1. Collect affected node IDs from extractions
    /// 2. Delete extractions for this source's chunks and statements
    /// 3. Nullify `node_aliases.source_chunk_id` references
    /// 4. Delete statements, sections, and unresolved entities
    /// 5. Delete chunks and projection ledger
    /// 6. For each affected node with zero remaining active
    ///    extractions: delete aliases, edges, then the node.
    ///    For nodes with remaining extractions: update
    ///    `mention_count`
    /// 7. Delete the source record
    pub async fn delete(&self, id: SourceId) -> Result<DeleteResult> {
        // Step 1: Collect affected entity IDs before we delete
        // anything. These are needed for both structural cleanup
        // and epistemic cascade.
        let affected_node_ids = ExtractionRepo::list_node_ids_by_source(&*self.repo, id).await?;
        // Note: affected_edge_ids is a superset — it may include
        // edges later deleted by EdgeRepo::delete_by_node in step 6.
        // The batch cascade handles this gracefully: get_many omits
        // deleted IDs (absent from DB), so they are skipped.
        let affected_edge_ids = ExtractionRepo::list_edge_ids_by_source(&*self.repo, id).await?;

        // Step 2: Delete extractions (must precede chunk deletion
        // due to FK on extractions.chunk_id).
        let extractions_deleted = ExtractionRepo::delete_by_source(&*self.repo, id).await?;

        // Step 3: Nullify alias chunk references (must precede
        // chunk deletion due to FK on
        // node_aliases.source_chunk_id).
        NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?;

        // Step 4: Delete statements and sections (statement-first
        // pipeline). Must follow extraction deletion since extractions
        // may reference statements via FK.
        let statements_deleted = StatementRepo::delete_by_source(&*self.repo, id).await?;
        let sections_deleted = SectionRepo::delete_by_source(&*self.repo, id).await?;
        UnresolvedEntityRepo::delete_by_source(&*self.repo, id).await?;

        // Step 5: Delete chunks and projection ledger.
        let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, id).await?;
        LedgerRepo::delete_by_source(&*self.repo, id).await?;

        // Step 6: Handle affected nodes. For each node, check if
        // it still has active extractions from other sources.
        let mut nodes_deleted: u64 = 0;
        let mut edges_deleted: u64 = 0;
        let mut surviving_node_ids: Vec<NodeId> = Vec::new();

        for node_id in &affected_node_ids {
            let remaining =
                ExtractionRepo::count_active_by_entity(&*self.repo, "node", node_id.into_uuid())
                    .await?;

            if remaining == 0 {
                // Orphaned node — delete aliases, edges, then
                // the node itself.
                NodeAliasRepo::delete_by_node(&*self.repo, *node_id).await?;
                edges_deleted += EdgeRepo::delete_by_node(&*self.repo, *node_id).await?;
                if NodeRepo::delete(&*self.repo, *node_id).await? {
                    nodes_deleted += 1;
                }
            } else {
                // Node survives — track for epistemic cascade.
                surviving_node_ids.push(*node_id);
                // Update mention_count to reflect the remaining
                // extraction count.
                if let Some(mut node) = NodeRepo::get(&*self.repo, *node_id).await? {
                    node.mention_count = remaining as i32;
                    NodeRepo::update(&*self.repo, &node).await?;
                }
            }
        }

        // Step 6b: Epistemic cascade — recalculate opinions for
        // surviving entities that lost extraction support (#105).
        // This implements TMS dependency-directed backtracking:
        // claims that lost their sole source become vacuous,
        // claims with remaining support are re-fused.
        let mut nodes_recalculated: usize = 0;
        let mut edges_recalculated: usize = 0;

        if !surviving_node_ids.is_empty() || !affected_edge_ids.is_empty() {
            let cascade =
                epistemic_cascade(&self.repo, &surviving_node_ids, &affected_edge_ids).await;
            match cascade {
                Ok(result) => {
                    nodes_recalculated = result.nodes_recalculated + result.nodes_vacuated;
                    edges_recalculated = result.edges_recalculated + result.edges_vacuated;
                    if result.total_affected() > 0 {
                        tracing::info!(
                            source_id = %id,
                            nodes_recalculated = result.nodes_recalculated,
                            nodes_vacuated = result.nodes_vacuated,
                            edges_recalculated = result.edges_recalculated,
                            edges_vacuated = result.edges_vacuated,
                            "epistemic cascade complete"
                        );
                    }
                }
                Err(e) => {
                    // Cascade failure is non-fatal — structural
                    // cleanup already succeeded.
                    tracing::warn!(
                        source_id = %id,
                        error = %e,
                        "epistemic cascade failed (non-fatal)"
                    );
                }
            }
        }

        // Step 7: Delete the source record.
        let deleted = SourceRepo::delete(&*self.repo, id).await?;

        Ok(DeleteResult {
            deleted,
            chunks_deleted,
            extractions_deleted,
            nodes_deleted,
            edges_deleted,
            statements_deleted,
            sections_deleted,
            nodes_recalculated,
            edges_recalculated,
        })
    }

    /// Publish a source by promoting its clearance level from
    /// `LocalStrict` (0) to `FederatedTrusted` (1).
    ///
    /// Returns an error if the source is not found or is already at a
    /// clearance level above `LocalStrict`.
    pub async fn publish(&self, id: SourceId) -> Result<Source> {
        let mut source =
            SourceRepo::get(&*self.repo, id)
                .await?
                .ok_or_else(|| Error::NotFound {
                    entity_type: "source",
                    id: id.to_string(),
                })?;

        if source.clearance_level != ClearanceLevel::LocalStrict {
            return Err(Error::InvalidInput(format!(
                "source {} is already at clearance level {}",
                id, source.clearance_level
            )));
        }

        source.clearance_level = ClearanceLevel::FederatedTrusted;
        SourceRepo::update(&*self.repo, &source).await?;
        Ok(source)
    }

    /// Count total number of sources.
    pub async fn count(&self) -> Result<i64> {
        SourceRepo::count(&*self.repo).await
    }

    /// Get all chunks for a source.
    pub async fn get_chunks(
        &self,
        source_id: SourceId,
    ) -> Result<Vec<crate::models::chunk::Chunk>> {
        ChunkRepo::list_by_source(&*self.repo, source_id).await
    }
}
