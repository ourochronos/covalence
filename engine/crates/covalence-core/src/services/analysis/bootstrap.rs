//! Component bootstrap and cross-domain linking.

use std::collections::HashMap;

use crate::error::Result;
use crate::graph::sync::full_reload;
use crate::ingestion::embedder::truncate_and_validate;
use crate::models::audit::{AuditAction, AuditLog};
use crate::models::edge::Edge;
use crate::models::node::Node;
use crate::storage::traits::{AuditLogRepo, EdgeRepo, NodeRepo};
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
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM nodes \
                 WHERE LOWER(canonical_name) = LOWER($1) \
                   AND node_type = 'component')",
            )
            .bind(name)
            .fetch_one(self.repo.pool())
            .await?;

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
            full_reload(self.repo.pool(), self.graph.clone()).await?;
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
            full_reload(self.repo.pool(), self.graph.clone()).await?;
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

        // Fetch code nodes from actual code sources. We require provenance
        // linking to a source with source_type='code' to avoid counting
        // function/struct entities mentioned in research papers or specs.
        let code_nodes: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT DISTINCT n.id, n.canonical_name, \
                    COALESCE( \
                      n.properties->>'file_path', \
                      (SELECT s.uri FROM extractions ex \
                       JOIN chunks c ON ex.chunk_id = c.id \
                       JOIN sources s ON c.source_id = s.id \
                       WHERE ex.entity_id = n.id \
                       ORDER BY CASE WHEN s.source_type = 'code' \
                                     THEN 0 ELSE 1 END \
                       LIMIT 1), \
                      '' \
                    ) AS path \
             FROM nodes n \
             WHERE n.node_type = ANY($1) \
               AND EXISTS ( \
                 SELECT 1 FROM extractions ex \
                 JOIN chunks c ON ex.chunk_id = c.id \
                 JOIN sources s ON c.source_id = s.id \
                 WHERE ex.entity_id = n.id \
                   AND s.source_type = 'code' \
               )",
        )
        .bind(&code_types[..])
        .fetch_all(self.repo.pool())
        .await?;

        // Fetch all component nodes.
        let components: Vec<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT id, canonical_name FROM nodes WHERE node_type = 'component'")
                .fetch_all(self.repo.pool())
                .await?;

        let comp_map: HashMap<&str, uuid::Uuid> = components
            .iter()
            .map(|(id, name)| (name.as_str(), *id))
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
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM edges \
                 WHERE source_node_id = $1 AND target_node_id = $2 \
                   AND rel_type = 'PART_OF_COMPONENT')",
            )
            .bind(code_id)
            .bind(comp_id)
            .fetch_one(self.repo.pool())
            .await?;

            if exists {
                skipped += 1;
                continue;
            }

            let code_nid = NodeId::from_uuid(*code_id);
            let comp_nid = NodeId::from_uuid(comp_id);
            let mut edge = Edge::new(code_nid, comp_nid, "PART_OF_COMPONENT".to_string());
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
        let components: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT id, canonical_name, COALESCE(description, '') \
             FROM nodes \
             WHERE node_type = 'component' AND embedding IS NOT NULL",
        )
        .fetch_all(self.repo.pool())
        .await?;

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
            let spec_matches: Vec<(uuid::Uuid, String, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, \
                        (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
                 FROM nodes n \
                 WHERE n.node_type NOT IN ('struct','function','trait','enum', \
                       'impl_block','constant','macro','module','class','component') \
                   AND n.embedding IS NOT NULL \
                   AND n.id != $1 \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON ex.chunk_id = c.id \
                     JOIN sources s ON c.source_id = s.id \
                     WHERE ex.entity_id = n.id \
                       AND (s.uri LIKE '%spec/%' OR s.uri LIKE '%docs/adr/%' \
                            OR s.uri LIKE '%VISION%' OR s.uri LIKE '%CLAUDE%') \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(comp_id)
            .bind(max_edges)
            .fetch_all(self.repo.pool())
            .await?;

            let (i, s) = self
                .create_bridge_edges(
                    *comp_id,
                    comp_name,
                    &spec_matches,
                    threshold,
                    "IMPLEMENTS_INTENT",
                    "spec_to_component",
                )
                .await?;
            intent_created += i;
            skipped += s;

            // --- THEORETICAL_BASIS: research/theory concepts ---
            // Include entities that appear in at least one non-spec source.
            // An entity merged across spec+research should be eligible for
            // THEORETICAL_BASIS edges (the research provenance is real).
            let research_matches: Vec<(uuid::Uuid, String, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, \
                        (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
                 FROM nodes n \
                 WHERE n.node_type NOT IN ('struct','function','trait','enum', \
                       'impl_block','constant','macro','module','class','component') \
                   AND n.embedding IS NOT NULL \
                   AND n.id != $1 \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON ex.chunk_id = c.id \
                     JOIN sources s ON c.source_id = s.id \
                     WHERE ex.entity_id = n.id \
                       AND s.uri NOT LIKE '%spec/%' \
                       AND s.uri NOT LIKE '%docs/adr/%' \
                       AND s.uri NOT LIKE '%VISION%' \
                       AND s.uri NOT LIKE '%CLAUDE%' \
                       AND s.source_type != 'code' \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(comp_id)
            .bind(max_edges)
            .fetch_all(self.repo.pool())
            .await?;

            let (b, s) = self
                .create_bridge_edges(
                    *comp_id,
                    comp_name,
                    &research_matches,
                    threshold,
                    "THEORETICAL_BASIS",
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

            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM edges \
                 WHERE source_node_id = $1 AND target_node_id = $2 \
                   AND rel_type = $3)",
            )
            .bind(comp_id)
            .bind(target_id)
            .bind(rel_type)
            .fetch_one(self.repo.pool())
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
