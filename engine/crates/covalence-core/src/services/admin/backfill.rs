//! Garbage collection, noise cleanup, Tier 5 resolution, embedding
//! backfill, opinion seeding, and code node summarization.

use crate::error::{Error, Result};
use crate::models::audit::{AuditAction, AuditLog};
use crate::storage::traits::{AuditLogRepo, EdgeRepo, NodeAliasRepo, NodeRepo};

use super::AdminService;

/// Result of a provenance-based garbage collection pass.
///
/// Nodes that lost all active (non-superseded) extraction grounding
/// are evicted along with their edges and aliases.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GcResult {
    /// Number of ungrounded nodes evicted.
    pub nodes_evicted: u64,
    /// Number of edges removed (from evicted nodes).
    pub edges_removed: u64,
    /// Number of aliases removed (from evicted nodes).
    pub aliases_removed: u64,
}

/// A noise entity identified for cleanup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoiseEntityInfo {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical entity name.
    pub canonical_name: String,
    /// Entity type.
    pub node_type: String,
    /// Number of edges connected to this node.
    pub edge_count: u64,
}

/// Result of noise entity cleanup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoiseCleanupResult {
    /// Number of noise nodes identified.
    pub nodes_identified: u64,
    /// Number of nodes actually deleted (0 in dry-run mode).
    pub nodes_deleted: u64,
    /// Number of edges removed (0 in dry-run mode).
    pub edges_removed: u64,
    /// Number of aliases removed (0 in dry-run mode).
    pub aliases_removed: u64,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Details of identified noise entities.
    pub entities: Vec<NoiseEntityInfo>,
}

/// Result of backfilling node embeddings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackfillResult {
    /// Total nodes found without embeddings.
    pub total_missing: u64,
    /// Nodes successfully embedded.
    pub embedded: u64,
    /// Nodes that failed to embed.
    pub failed: u64,
}

/// Result of seeding epistemic opinions.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SeedOpinionsResult {
    /// Nodes that received computed opinions from extractions.
    pub nodes_seeded: u64,
    /// Nodes set to vacuous opinion (no extractions).
    pub nodes_vacuous: u64,
    /// Edges that received computed opinions from extractions.
    pub edges_seeded: u64,
    /// Edges set to vacuous opinion (no extractions).
    pub edges_vacuous: u64,
}

/// Result of LLM code node summarization.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodeSummaryResult {
    /// Code nodes found without semantic summaries.
    pub nodes_found: u64,
    /// Nodes successfully summarized and re-embedded.
    pub summarized: u64,
    /// Nodes where LLM summary failed.
    pub failed: u64,
}

impl AdminService {
    /// Run provenance-based garbage collection.
    ///
    /// Finds all nodes where every extraction has been superseded
    /// (no active extractions remain) and evicts them along with
    /// their edges and aliases. Returns counts of evicted entities.
    pub async fn garbage_collect_nodes(&self) -> Result<GcResult> {
        let ungrounded = NodeRepo::list_ungrounded(&*self.repo).await?;

        if ungrounded.is_empty() {
            tracing::info!("gc: no ungrounded nodes found");
            return Ok(GcResult {
                nodes_evicted: 0,
                edges_removed: 0,
                aliases_removed: 0,
            });
        }

        tracing::info!(count = ungrounded.len(), "gc: evicting ungrounded nodes");

        let mut nodes_evicted: u64 = 0;
        let mut edges_removed: u64 = 0;
        let mut aliases_removed: u64 = 0;

        for node in &ungrounded {
            // Delete aliases first (no FK constraints from aliases
            // to edges, but clean up before node deletion).
            aliases_removed += NodeAliasRepo::delete_by_node(&*self.repo, node.id).await?;

            // Delete edges involving this node.
            edges_removed += EdgeRepo::delete_by_node(&*self.repo, node.id).await?;

            // Delete the node itself.
            if NodeRepo::delete(&*self.repo, node.id).await? {
                nodes_evicted += 1;
            }
        }

        tracing::info!(
            nodes_evicted,
            edges_removed,
            aliases_removed,
            "gc: provenance-based garbage collection complete"
        );

        Ok(GcResult {
            nodes_evicted,
            edges_removed,
            aliases_removed,
        })
    }

    /// Retroactively clean noise entities from the graph.
    ///
    /// Scans all nodes through the `is_noise_entity()` filter and
    /// optionally removes matches along with their edges and aliases.
    /// In dry-run mode (default), only reports what would be deleted.
    pub async fn cleanup_noise_entities(&self, dry_run: bool) -> Result<NoiseCleanupResult> {
        use super::super::noise_filter::is_noise_entity;

        // Fetch all nodes (id, canonical_name, node_type).
        let rows: Vec<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT id, canonical_name, node_type FROM nodes")
                .fetch_all(self.repo.pool())
                .await?;

        // Identify noise entities.
        let mut noise: Vec<(uuid::Uuid, String, String)> = Vec::new();
        for (id, name, ntype) in &rows {
            if is_noise_entity(name, ntype) {
                noise.push((*id, name.clone(), ntype.clone()));
            }
        }

        if noise.is_empty() {
            tracing::info!("noise cleanup: no noise entities found");
            return Ok(NoiseCleanupResult {
                nodes_identified: 0,
                nodes_deleted: 0,
                edges_removed: 0,
                aliases_removed: 0,
                dry_run,
                entities: Vec::new(),
            });
        }

        tracing::info!(
            count = noise.len(),
            dry_run,
            "noise cleanup: identified noise entities"
        );

        // Count edges per noise node for reporting.
        let mut entities: Vec<NoiseEntityInfo> = Vec::new();
        for (id, name, ntype) in &noise {
            let edge_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM edges \
                 WHERE source_node_id = $1 \
                    OR target_node_id = $1",
            )
            .bind(id)
            .fetch_one(self.repo.pool())
            .await?;

            entities.push(NoiseEntityInfo {
                node_id: *id,
                canonical_name: name.clone(),
                node_type: ntype.clone(),
                edge_count: edge_count as u64,
            });
        }

        // Sort by edge count descending for visibility.
        entities.sort_by(|a, b| b.edge_count.cmp(&a.edge_count));

        let nodes_identified = entities.len() as u64;

        if dry_run {
            return Ok(NoiseCleanupResult {
                nodes_identified,
                nodes_deleted: 0,
                edges_removed: 0,
                aliases_removed: 0,
                dry_run: true,
                entities,
            });
        }

        // Delete: aliases -> edges -> nodes (FK order).
        let mut nodes_deleted: u64 = 0;
        let mut edges_removed: u64 = 0;
        let mut aliases_removed: u64 = 0;

        for entity in &entities {
            aliases_removed += NodeAliasRepo::delete_by_node(
                &*self.repo,
                crate::types::ids::NodeId::from_uuid(entity.node_id),
            )
            .await?;

            // Nullify invalidated_by FK references pointing at edges
            // we are about to delete, to avoid FK violation.
            sqlx::query(
                "UPDATE edges SET invalidated_by = NULL \
                 WHERE invalidated_by IN ( \
                     SELECT id FROM edges \
                     WHERE source_node_id = $1 \
                        OR target_node_id = $1 \
                 )",
            )
            .bind(entity.node_id)
            .execute(self.repo.pool())
            .await?;

            edges_removed += EdgeRepo::delete_by_node(
                &*self.repo,
                crate::types::ids::NodeId::from_uuid(entity.node_id),
            )
            .await?;

            // Clear unresolved_entities FK references before deletion
            // to avoid FK violation if this node was a Tier 5 target.
            sqlx::query(
                "UPDATE unresolved_entities \
                 SET resolved_node_id = NULL \
                 WHERE resolved_node_id = $1",
            )
            .bind(entity.node_id)
            .execute(self.repo.pool())
            .await?;

            if NodeRepo::delete(
                &*self.repo,
                crate::types::ids::NodeId::from_uuid(entity.node_id),
            )
            .await?
            {
                nodes_deleted += 1;
            }
        }

        tracing::info!(
            nodes_deleted,
            edges_removed,
            aliases_removed,
            "noise cleanup: retroactive cleanup complete"
        );

        // Reload graph sidecar to reflect deletions.
        if nodes_deleted > 0 {
            self.graph.reload(self.repo.pool()).await?;
        }

        // Audit log.
        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:cleanup_noise_entities".to_string(),
            serde_json::json!({
                "nodes_identified": nodes_identified,
                "nodes_deleted": nodes_deleted,
                "edges_removed": edges_removed,
                "aliases_removed": aliases_removed,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(NoiseCleanupResult {
            nodes_identified,
            nodes_deleted,
            edges_removed,
            aliases_removed,
            dry_run: false,
            entities,
        })
    }

    /// Run Tier 5 HDBSCAN batch resolution on the
    /// unresolved_entities pool.
    ///
    /// Fetches pending entities, embeds names, clusters with HDBSCAN,
    /// and resolves each cluster to a canonical node. Noise entities
    /// are promoted to individual nodes.
    pub async fn resolve_tier5(
        &self,
        min_cluster_size: Option<usize>,
    ) -> Result<crate::consolidation::tier5::Tier5Report> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured for Tier 5 resolution".into()))?;

        let node_embed_dim = self
            .config
            .as_ref()
            .map(|c| c.embedding.table_dims.node)
            .unwrap_or(256);

        let config = crate::consolidation::tier5::Tier5Config {
            min_cluster_size: min_cluster_size.unwrap_or(2),
            node_embed_dim,
        };

        crate::consolidation::tier5::resolve_tier5(&self.repo, embedder.as_ref(), &config).await
    }

    /// Backfill embeddings for nodes that are missing them.
    ///
    /// Fetches all node IDs with `embedding IS NULL`, generates
    /// embeddings from `canonical_name: description` text, and
    /// stores them via `update_embedding`.
    pub async fn backfill_node_embeddings(&self) -> Result<BackfillResult> {
        use crate::ingestion::embedder::truncate_and_validate;

        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured".into()))?;

        let node_dim = self
            .config
            .as_ref()
            .map(|c| c.embedding.table_dims.node)
            .unwrap_or(256);

        // Fetch nodes missing embeddings via SP.
        let rows: Vec<(uuid::Uuid, String, Option<String>)> =
            sqlx::query_as("SELECT * FROM sp_list_nodes_without_embeddings($1)")
                .bind(i32::MAX)
                .fetch_all(self.repo.pool())
                .await?;

        if rows.is_empty() {
            return Ok(BackfillResult {
                total_missing: 0,
                embedded: 0,
                failed: 0,
            });
        }

        let total_missing = rows.len();
        tracing::info!(total_missing, "backfilling node embeddings");

        // Batch embed in chunks of 100.
        let mut embedded = 0u64;
        let mut failed = 0u64;
        for batch in rows.chunks(100) {
            let texts: Vec<String> = batch
                .iter()
                .map(|(_, name, desc)| match desc {
                    Some(d) if !d.is_empty() => {
                        format!("{name}: {d}")
                    }
                    _ => name.clone(),
                })
                .collect();

            match embedder.embed(&texts).await {
                Ok(embeddings) => {
                    for ((id, _, _), emb) in batch.iter().zip(embeddings.iter()) {
                        match truncate_and_validate(emb, node_dim, "nodes") {
                            Ok(truncated) => {
                                let nid = crate::types::ids::NodeId::from_uuid(*id);
                                if let Err(e) =
                                    NodeRepo::update_embedding(&*self.repo, nid, &truncated).await
                                {
                                    tracing::warn!(
                                        node_id = %id,
                                        error = %e,
                                        "embed store failed"
                                    );
                                    failed += 1;
                                } else {
                                    embedded += 1;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    node_id = %id,
                                    error = %e,
                                    "truncate failed"
                                );
                                failed += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e, "batch embed failed"
                    );
                    failed += batch.len() as u64;
                }
            }
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:backfill_node_embeddings".to_string(),
            serde_json::json!({
                "total_missing": total_missing,
                "embedded": embedded,
                "failed": failed,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(embedded, failed, "backfill complete");
        Ok(BackfillResult {
            total_missing: total_missing as u64,
            embedded,
            failed,
        })
    }

    /// Seed epistemic opinions on all nodes and edges from their
    /// extraction records.
    ///
    /// Uses the batch cascade functions to compute Subjective Logic
    /// opinions via cumulative fusion across all active extractions.
    /// Nodes/edges with no extractions get vacuous opinions.
    pub async fn seed_opinions(&self) -> Result<SeedOpinionsResult> {
        use crate::epistemic::cascade::{recalculate_edge_opinions, recalculate_node_opinions};
        use crate::types::ids::{EdgeId, NodeId};

        // Fetch all node IDs.
        let node_uuids: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM nodes")
            .fetch_all(self.repo.pool())
            .await?;
        let node_ids: Vec<NodeId> = node_uuids.iter().map(|u| NodeId::from_uuid(*u)).collect();

        // Fetch all edge IDs.
        let edge_uuids: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM edges WHERE NOT is_synthetic")
                .fetch_all(self.repo.pool())
                .await?;
        let edge_ids: Vec<EdgeId> = edge_uuids.iter().map(|u| EdgeId::from_uuid(*u)).collect();

        tracing::info!(
            nodes = node_ids.len(),
            edges = edge_ids.len(),
            "seeding opinions"
        );

        // Process nodes in batches of 500.
        let mut node_result = crate::epistemic::cascade::CascadeResult::default();
        for batch in node_ids.chunks(500) {
            let r = recalculate_node_opinions(&*self.repo, batch).await?;
            node_result.merge(&r);
        }

        // Process edges in batches of 500.
        let mut edge_result = crate::epistemic::cascade::CascadeResult::default();
        for batch in edge_ids.chunks(500) {
            let r = recalculate_edge_opinions(&*self.repo, batch).await?;
            edge_result.merge(&r);
        }

        let result = SeedOpinionsResult {
            nodes_seeded: node_result.nodes_recalculated as u64,
            nodes_vacuous: node_result.nodes_vacuated as u64,
            edges_seeded: edge_result.edges_recalculated as u64,
            edges_vacuous: edge_result.edges_vacuated as u64,
        };

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:seed_opinions".to_string(),
            serde_json::json!({
                "nodes_seeded": result.nodes_seeded,
                "nodes_vacuous": result.nodes_vacuous,
                "edges_seeded": result.edges_seeded,
                "edges_vacuous": result.edges_vacuous,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(
            nodes_seeded = result.nodes_seeded,
            nodes_vacuous = result.nodes_vacuous,
            edges_seeded = result.edges_seeded,
            edges_vacuous = result.edges_vacuous,
            "opinion seeding complete"
        );
        Ok(result)
    }

    /// Generate LLM semantic summaries for code-type nodes and
    /// re-embed them using the summary text.
    ///
    /// Finds nodes with code entity types (struct, function, trait,
    /// enum, impl_block, constant, macro, module, class) that don't
    /// already have a `semantic_summary` in their properties. For
    /// each, sends `canonical_name + description` to the LLM for a
    /// plain-English summary, stores it in
    /// `properties.semantic_summary`, and re-generates the embedding
    /// from the summary text.
    pub async fn summarize_code_nodes(&self) -> Result<CodeSummaryResult> {
        use crate::ingestion::embedder::truncate_and_validate;

        let chat = self
            .chat_backend
            .as_ref()
            .ok_or_else(|| Error::Config("no chat backend configured".into()))?;
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured".into()))?;
        let node_dim = self
            .config
            .as_ref()
            .map(|c| c.embedding.table_dims.node)
            .unwrap_or(256);

        let code_types = [
            "struct",
            "function",
            "trait",
            "enum",
            "impl_block",
            "constant",
            "macro",
            "module",
            "class",
        ];

        // Fetch code nodes without semantic summaries.
        type CodeNodeRow = (
            uuid::Uuid,
            String,
            String,
            Option<String>,
            Option<serde_json::Value>,
        );
        let rows: Vec<CodeNodeRow> = sqlx::query_as(
            "SELECT id, canonical_name, node_type, \
                    description, properties \
                 FROM nodes \
                 WHERE node_type = ANY($1) \
                   AND (properties IS NULL \
                        OR properties->>'semantic_summary' \
                           IS NULL)",
        )
        .bind(&code_types[..])
        .fetch_all(self.repo.pool())
        .await?;

        if rows.is_empty() {
            return Ok(CodeSummaryResult {
                nodes_found: 0,
                summarized: 0,
                failed: 0,
            });
        }

        let nodes_found = rows.len() as u64;
        tracing::info!(nodes_found, "summarizing code nodes");

        let system_prompt = "You are a code documentation assistant. Given a \
            code entity (struct, function, trait, interface, \
            etc.) with its name and description, write a \
            concise 1-3 sentence natural language summary of \
            what it does. Focus on purpose and behavior, not \
            syntax. Use domain terminology that would appear \
            in design docs or specifications. Do not use \
            markdown formatting.";

        let mut summarized = 0u64;
        let mut failed = 0u64;

        for (id, name, node_type, desc, props) in &rows {
            let user_prompt = format!(
                "Entity: {name}\nType: {node_type}\n\
                 Description: {}",
                desc.as_deref().unwrap_or("(none)")
            );

            match chat.chat(system_prompt, &user_prompt, false, 0.3).await {
                Ok(resp) => {
                    let summary = resp.text.trim().to_string();
                    if summary.is_empty() {
                        failed += 1;
                        continue;
                    }

                    // Store semantic_summary in properties.
                    let mut new_props = props.clone().unwrap_or(serde_json::json!({}));
                    new_props["semantic_summary"] = serde_json::json!(&summary);

                    sqlx::query(
                        "UPDATE nodes SET properties = $2 \
                         WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&new_props)
                    .execute(self.repo.pool())
                    .await?;

                    // Re-embed using the summary text.
                    let embed_text = format!("{name}: {summary}");
                    match embedder.embed(&[embed_text]).await {
                        Ok(embeddings) => {
                            if let Some(emb) = embeddings.first() {
                                match truncate_and_validate(emb, node_dim, "nodes") {
                                    Ok(truncated) => {
                                        let nid = crate::types::ids::NodeId::from_uuid(*id);
                                        if let Err(e) =
                                            NodeRepo::update_embedding(&*self.repo, nid, &truncated)
                                                .await
                                        {
                                            tracing::warn!(
                                                node = %name,
                                                error = %e,
                                                "embedding storage after summary failed"
                                            );
                                            failed += 1;
                                            continue;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            node = %name,
                                            error = %e,
                                            "embedding truncation after summary failed"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                node = %name,
                                error = %e,
                                "re-embed after summary failed"
                            );
                        }
                    }

                    summarized += 1;
                    if summarized % 50 == 0 {
                        tracing::info!(summarized, "code summary progress");
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        node = %name,
                        error = %e,
                        "LLM summary failed"
                    );
                    failed += 1;
                }
            }
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:summarize_code_nodes".to_string(),
            serde_json::json!({
                "nodes_found": nodes_found,
                "summarized": summarized,
                "failed": failed,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(summarized, failed, "code summary complete");
        Ok(CodeSummaryResult {
            nodes_found,
            summarized,
            failed,
        })
    }
}
