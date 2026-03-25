//! Component bootstrap and cross-domain linking.

use std::collections::HashMap;

use crate::error::Result;
use crate::ingestion::embedder::truncate_and_validate;
use crate::models::audit::{AuditAction, AuditLog};
use crate::models::edge::Edge;
use crate::models::node::Node;
use crate::storage::traits::{AnalysisRepo, AuditLogRepo, EdgeRepo, NodeRepo};
use crate::types::ids::NodeId;

use super::constants::{COMPONENT_DEFS, MODULE_PATH_MAPPINGS};
use super::{AnalysisService, BootstrapResult, LinkingResult};

impl AnalysisService {
    // ------------------------------------------------------------------
    // Phase 1: Component Bootstrap
    // ------------------------------------------------------------------

    /// Bootstrap the 9 well-known Component nodes.
    ///
    /// Creates `node_type = "component"` nodes for each logical subsystem.
    /// Skips components that already exist (by canonical_name match).
    /// Embeds the description and stores the embedding on the node.
    pub async fn bootstrap_components(&self) -> Result<BootstrapResult> {
        let mut created = 0u64;
        let mut existing = 0u64;
        let mut embedded = 0u64;

        for (name, description) in COMPONENT_DEFS {
            // Check if a component with this name already exists.
            let exists = AnalysisRepo::component_exists(&*self.repo, name).await?;

            if exists {
                existing += 1;
                tracing::debug!(component = %name, "component already exists, skipping");
                continue;
            }

            let mut node = Node::new(name.to_string(), "component".to_string());
            node.description = Some(description.to_string());
            node.properties = serde_json::json!({
                "domain": "architecture",
                "bootstrap": true,
            });

            NodeRepo::create(&*self.repo, &node).await?;
            created += 1;

            // Embed the description and store on the node.
            if let Some(ref embedder) = self.embedder {
                let texts = vec![description.to_string()];
                match embedder.embed(&texts).await {
                    Ok(embeddings) => {
                        if let Some(raw) = embeddings.first() {
                            let validated =
                                truncate_and_validate(raw, self.node_embed_dim, "node")?;
                            NodeRepo::update_embedding(&*self.repo, node.id, &validated).await?;
                            embedded += 1;
                            tracing::debug!(component = %name, "component embedded");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = %name,
                            error = %e,
                            "failed to embed component description"
                        );
                    }
                }
            }

            tracing::info!(component = %name, "component node created");
        }

        // Reload graph so new component nodes are visible.
        if created > 0 {
            self.graph.reload(self.repo.pool()).await?;
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "analysis:bootstrap_components".to_string(),
            serde_json::json!({
                "components_created": created,
                "components_existing": existing,
                "components_embedded": embedded,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(created, existing, embedded, "component bootstrap complete");
        Ok(BootstrapResult {
            components_created: created,
            components_existing: existing,
            components_embedded: embedded,
        })
    }

    // ------------------------------------------------------------------
    // Phase 2: Cross-Domain Linking
    // ------------------------------------------------------------------

    /// Create cross-domain bridge edges:
    /// - PART_OF_COMPONENT: code nodes -> component (via module path)
    /// - IMPLEMENTS_INTENT: component -> spec/design concepts (via embedding)
    /// - THEORETICAL_BASIS: component -> research concepts (via embedding)
    pub async fn link_domains(
        &self,
        min_similarity: f64,
        max_edges_per_component: i64,
    ) -> Result<LinkingResult> {
        let mut part_of = 0u64;
        let mut intent = 0u64;
        let mut basis = 0u64;
        let mut skipped = 0u64;

        // --- PART_OF_COMPONENT: module-path matching ---
        let (p, s) = self.link_part_of_component().await?;
        part_of += p;
        skipped += s;

        // --- IMPLEMENTS_INTENT + THEORETICAL_BASIS: semantic similarity ---
        let (i, b, s2) = self
            .link_semantic_bridges(min_similarity, max_edges_per_component)
            .await?;
        intent += i;
        basis += b;
        skipped += s2;

        // Reload graph sidecar.
        if part_of + intent + basis > 0 {
            self.graph.reload(self.repo.pool()).await?;
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "analysis:link_domains".to_string(),
            serde_json::json!({
                "part_of_edges": part_of,
                "intent_edges": intent,
                "basis_edges": basis,
                "skipped_existing": skipped,
                "min_similarity": min_similarity,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(
            part_of,
            intent,
            basis,
            skipped,
            "cross-domain linking complete"
        );
        Ok(LinkingResult {
            part_of_edges: part_of,
            intent_edges: intent,
            basis_edges: basis,
            skipped_existing: skipped,
        })
    }

    /// Create PART_OF_COMPONENT edges by matching code node source file
    /// paths against known module path prefixes.
    ///
    /// Since code nodes may not store `file_path` in their own properties,
    /// we trace back through extraction provenance to find the source URI.
    async fn link_part_of_component(&self) -> Result<(u64, u64)> {
        // Fetch code nodes from actual code sources.
        let code_nodes = AnalysisRepo::list_code_nodes_with_paths(
            &*self.repo,
            &self.domains.code_entity_class,
            &self.domains.code_domain,
        )
        .await?;

        // Fetch all component nodes.
        let components = AnalysisRepo::list_component_nodes(&*self.repo).await?;

        let comp_map: HashMap<&str, uuid::Uuid> = components
            .iter()
            .map(|(id, name, _)| (name.as_str(), *id))
            .collect();

        let mut created = 0u64;
        let mut skipped = 0u64;

        for (code_id, _code_name, file_path) in &code_nodes {
            // Normalize covalence:// URIs so
            // "covalence://engine/services/search.rs"
            // matches patterns like "src/services/search".
            let normalized = if file_path.starts_with("covalence://engine/") {
                file_path.replacen("covalence://engine/", "src/", 1)
            } else {
                file_path.clone()
            };

            // Find the best matching component by module path prefix.
            let component_name = MODULE_PATH_MAPPINGS
                .iter()
                .find(|(prefix, _)| normalized.contains(prefix))
                .map(|(_, comp)| *comp);

            let Some(comp_name) = component_name else {
                continue;
            };
            let Some(&comp_id) = comp_map.get(comp_name) else {
                continue;
            };

            // Check if edge already exists.
            let exists = AnalysisRepo::check_edge_exists_sp(
                &*self.repo,
                *code_id,
                comp_id,
                &self.bridges.part_of_component,
            )
            .await?;

            if exists {
                skipped += 1;
                continue;
            }

            let code_nid = NodeId::from_uuid(*code_id);
            let comp_nid = NodeId::from_uuid(comp_id);
            let mut edge = Edge::new(code_nid, comp_nid, self.bridges.part_of_component.clone());
            edge.properties = serde_json::json!({
                "bridge_type": "module_path",
                "file_path": file_path,
            });

            EdgeRepo::create(&*self.repo, &edge).await?;
            created += 1;
        }

        tracing::info!(
            created,
            skipped,
            code_nodes = code_nodes.len(),
            "PART_OF_COMPONENT linking complete"
        );
        Ok((created, skipped))
    }

    /// Create IMPLEMENTS_INTENT and THEORETICAL_BASIS edges between
    /// Components and concept/spec/research nodes via embedding similarity.
    async fn link_semantic_bridges(
        &self,
        min_similarity: f64,
        max_edges: i64,
    ) -> Result<(u64, u64, u64)> {
        // Fetch component nodes that have embeddings.
        let components = AnalysisRepo::list_component_nodes(&*self.repo).await?;

        if components.is_empty() {
            tracing::warn!("no embedded component nodes found — run bootstrap first");
            return Ok((0, 0, 0));
        }

        let threshold = 1.0 - min_similarity;
        let mut intent_created = 0u64;
        let mut basis_created = 0u64;
        let mut skipped = 0u64;

        // Run two separate queries per component so each domain
        // (spec vs research) gets its own budget of `max_edges`.
        for (comp_id, comp_name, _desc) in &components {
            // --- IMPLEMENTS_INTENT: spec/design concepts ---
            let spec_matches = AnalysisRepo::find_nearest_domain_nodes(
                &*self.repo,
                *comp_id,
                &self.domains.spec_domains,
                max_edges,
            )
            .await?;

            let (i, s) = self
                .create_bridge_edges(
                    *comp_id,
                    comp_name,
                    &spec_matches,
                    threshold,
                    &self.bridges.implements_intent,
                    "spec_to_component",
                )
                .await?;
            intent_created += i;
            skipped += s;

            // --- THEORETICAL_BASIS: research/theory concepts ---
            let research_matches = AnalysisRepo::find_nearest_domain_nodes(
                &*self.repo,
                *comp_id,
                &self.domains.research_domains,
                max_edges,
            )
            .await?;

            let (b, s) = self
                .create_bridge_edges(
                    *comp_id,
                    comp_name,
                    &research_matches,
                    threshold,
                    &self.bridges.theoretical_basis,
                    "research_to_component",
                )
                .await?;
            basis_created += b;
            skipped += s;
        }

        tracing::info!(
            intent_created,
            basis_created,
            skipped,
            "semantic bridge linking complete"
        );
        Ok((intent_created, basis_created, skipped))
    }

    /// Create bridge edges from ANN match results, skipping duplicates.
    ///
    /// **Important:** `matches` must be sorted by ascending distance so the
    /// early-exit `break` on `threshold` is correct.  All callers currently
    /// pass the vector returned by `query_embeddings`, which is pre-sorted.
    async fn create_bridge_edges(
        &self,
        comp_id: uuid::Uuid,
        comp_name: &str,
        matches: &[(uuid::Uuid, String, f64)],
        threshold: f64,
        rel_type: &str,
        bridge_type: &str,
    ) -> Result<(u64, u64)> {
        let mut created = 0u64;
        let mut skipped = 0u64;

        for (target_id, target_name, dist) in matches {
            if *dist > threshold {
                break;
            }

            let exists =
                AnalysisRepo::check_edge_exists_sp(&*self.repo, comp_id, *target_id, rel_type)
                    .await?;

            if exists {
                skipped += 1;
                continue;
            }

            let similarity = 1.0 - dist;
            let comp_nid = NodeId::from_uuid(comp_id);
            let target_nid = NodeId::from_uuid(*target_id);
            let mut edge = Edge::new(comp_nid, target_nid, rel_type.to_string());
            edge.confidence = similarity;
            edge.properties = serde_json::json!({
                "bridge_type": bridge_type,
                "cosine_similarity": similarity,
            });

            EdgeRepo::create(&*self.repo, &edge).await?;
            created += 1;

            tracing::debug!(
                component = %comp_name,
                target = %target_name,
                rel_type,
                similarity,
                "bridge edge created"
            );
        }

        Ok((created, skipped))
    }
}
