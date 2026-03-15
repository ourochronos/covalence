//! Cross-domain analysis service — bridges research, spec, and code domains.
//!
//! Implements the Component model from spec/12-code-ingestion.md and the
//! six analysis capabilities from spec/13-cross-domain-analysis.md.

use std::collections::HashMap;
use std::sync::Arc;

use petgraph::visit::EdgeRef;

use crate::error::{Error, Result};
use crate::graph::SharedGraph;
use crate::graph::sync::full_reload;
use crate::ingestion::Embedder;
use crate::ingestion::embedder::truncate_and_validate;
use crate::models::audit::{AuditAction, AuditLog};
use crate::models::edge::Edge;
use crate::models::node::Node;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{AuditLogRepo, EdgeRepo, NodeRepo};
use crate::types::ids::NodeId;

/// Well-known logical components of the Covalence system.
///
/// Each entry: (canonical_name, description used for embedding + intent matching).
const COMPONENT_DEFS: &[(&str, &str)] = &[
    (
        "Ingestion Pipeline",
        "Source ingestion pipeline: file reading, HTML/PDF conversion, \
         chunking, embedding generation, entity extraction, and graph \
         construction from unstructured documents.",
    ),
    (
        "Statement Pipeline",
        "Statement-first extraction: windowed LLM statement extraction, \
         coreference resolution, offset projection, HAC clustering into \
         sections, source summary compilation.",
    ),
    (
        "Search Fusion",
        "Multi-dimensional fused search: vector, lexical, temporal, graph, \
         structural, and global search dimensions combined via convex \
         combination fusion with coverage multiplier and reranking.",
    ),
    (
        "Entity Resolution",
        "Five-tier entity resolution cascade: exact match, alias lookup, \
         vector cosine similarity, fuzzy trigram matching, and HDBSCAN \
         batch clustering for entity deduplication.",
    ),
    (
        "Graph Sidecar",
        "In-memory petgraph directed graph sidecar: node/edge index, \
         PageRank, TrustRank, community detection via label propagation, \
         neighborhood traversal, and topology analysis.",
    ),
    (
        "Epistemic Model",
        "Subjective Logic epistemic framework: opinion tuples (b,d,u,a), \
         Dempster-Shafer fusion, DF-QuAD argumentation, Bayesian memory \
         retention decay, confidence propagation and convergence.",
    ),
    (
        "Consolidation",
        "Knowledge consolidation: batch consolidation for merging duplicate \
         nodes, deep consolidation for ontology refinement, RAPTOR \
         recursive summarization, graph-batch consolidation.",
    ),
    (
        "API Layer",
        "Axum HTTP API server: RESTful endpoints under /api/v1, OpenAPI \
         spec via utoipa, Swagger UI, MCP tool interface, request routing \
         and middleware.",
    ),
    (
        "CLI",
        "Go CLI tool (cove): Cobra subcommands for source management, \
         search, node inspection, graph analysis, admin operations, \
         memory, and copilot integration.",
    ),
];

/// Module path prefixes mapped to component names for PART_OF_COMPONENT linking.
///
/// Code nodes whose `properties.file_path` starts with one of these
/// prefixes are linked to the corresponding Component.
const MODULE_PATH_MAPPINGS: &[(&str, &str)] = &[
    // Ingestion Pipeline
    ("src/ingestion/pipeline", "Ingestion Pipeline"),
    ("src/ingestion/embedder", "Ingestion Pipeline"),
    ("src/ingestion/converter", "Ingestion Pipeline"),
    ("src/ingestion/chunker", "Ingestion Pipeline"),
    ("src/ingestion/normalize", "Ingestion Pipeline"),
    ("src/ingestion/source_profile", "Ingestion Pipeline"),
    // Statement Pipeline
    ("src/ingestion/statement", "Statement Pipeline"),
    ("src/ingestion/chat_backend", "Statement Pipeline"),
    ("src/ingestion/section", "Statement Pipeline"),
    ("src/services/statement_pipeline", "Statement Pipeline"),
    // Search Fusion
    ("src/search", "Search Fusion"),
    ("src/services/search", "Search Fusion"),
    // Entity Resolution
    ("src/ingestion/resolver", "Entity Resolution"),
    // Graph Sidecar
    ("src/graph", "Graph Sidecar"),
    // Epistemic Model
    ("src/epistemic", "Epistemic Model"),
    // Consolidation
    ("src/consolidation", "Consolidation"),
    ("src/services/consolidation", "Consolidation"),
    // API Layer
    ("src/routes", "API Layer"),
    ("src/handlers", "API Layer"),
    ("src/middleware", "API Layer"),
    ("src/state", "API Layer"),
    ("src/openapi", "API Layer"),
    // CLI
    ("cmd/", "CLI"),
    ("internal/", "CLI"),
    // Services (general)
    ("src/services/admin", "API Layer"),
    ("src/services/source", "Ingestion Pipeline"),
    ("src/services/node", "Graph Sidecar"),
    ("src/services/edge", "Graph Sidecar"),
    ("src/services/noise_filter", "Ingestion Pipeline"),
];

/// Result of Component bootstrapping.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BootstrapResult {
    /// Components created as new nodes.
    pub components_created: u64,
    /// Components that already existed (skipped).
    pub components_existing: u64,
    /// Components that were embedded.
    pub components_embedded: u64,
}

/// Result of cross-domain linking.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkingResult {
    /// PART_OF_COMPONENT edges created (code → component).
    pub part_of_edges: u64,
    /// IMPLEMENTS_INTENT edges created (component → spec).
    pub intent_edges: u64,
    /// THEORETICAL_BASIS edges created (component → research).
    pub basis_edges: u64,
    /// Edges skipped because they already exist.
    pub skipped_existing: u64,
}

/// A single coverage item (orphaned code or unimplemented spec).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageItem {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// File path (for code nodes).
    pub file_path: Option<String>,
    /// Why this item is flagged.
    pub reason: String,
}

/// Result of coverage analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageResult {
    /// Code nodes with no path to any Component.
    pub orphan_code: Vec<CoverageItem>,
    /// Spec/concept nodes with no IMPLEMENTS_INTENT edges.
    pub unimplemented_specs: Vec<CoverageItem>,
    /// Fraction of spec topics that have implementation coverage.
    pub coverage_score: f64,
}

/// A component with its erosion metric.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErosionItem {
    /// Component node UUID.
    pub component_id: uuid::Uuid,
    /// Component name.
    pub component_name: String,
    /// Component description (design intent).
    pub spec_intent: String,
    /// Drift score: 1 - mean(cosine similarity) of child code nodes.
    pub drift_score: f64,
    /// Code nodes that diverge most from the component's intent.
    pub divergent_nodes: Vec<DivergentNode>,
}

/// A code node that diverges from its parent component's intent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DivergentNode {
    /// Code node UUID.
    pub node_id: uuid::Uuid,
    /// Code node name.
    pub name: String,
    /// Semantic summary of the code node.
    pub summary: Option<String>,
    /// Cosine distance from the component's embedding.
    pub distance: f64,
}

/// Result of erosion detection.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErosionResult {
    /// Components with drift above the threshold.
    pub eroded_components: Vec<ErosionItem>,
    /// Total components analyzed.
    pub total_components: u64,
}

/// Cross-domain analysis service.
pub struct AnalysisService {
    repo: Arc<PgRepo>,
    graph: SharedGraph,
    embedder: Option<Arc<dyn Embedder>>,
    node_embed_dim: usize,
}

impl AnalysisService {
    /// Create a new analysis service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self {
            repo,
            graph,
            embedder: None,
            node_embed_dim: 256,
        }
    }

    /// Set the embedder for component description embedding.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Set the target node embedding dimension.
    pub fn with_node_embed_dim(mut self, dim: usize) -> Self {
        self.node_embed_dim = dim;
        self
    }

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
    /// - PART_OF_COMPONENT: code nodes → component (via module path)
    /// - IMPLEMENTS_INTENT: component → spec/design concepts (via embedding)
    /// - THEORETICAL_BASIS: component → research concepts (via embedding)
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

        // Fetch code nodes joined with their source URI via provenance.
        // If a node has file_path in properties, use that; otherwise
        // fall back to the source URI via extractions → chunks → sources.
        let code_nodes: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT DISTINCT n.id, n.canonical_name, \
                    COALESCE( \
                      n.properties->>'file_path', \
                      (SELECT s.uri FROM extractions ex \
                       JOIN chunks c ON ex.chunk_id = c.id \
                       JOIN sources s ON c.source_id = s.id \
                       WHERE ex.entity_id = n.id LIMIT 1), \
                      '' \
                    ) AS path \
             FROM nodes n WHERE n.node_type = ANY($1)",
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
            // Find the best matching component by module path prefix.
            let component_name = MODULE_PATH_MAPPINGS
                .iter()
                .find(|(prefix, _)| file_path.contains(prefix))
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
            let research_matches: Vec<(uuid::Uuid, String, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, \
                        (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
                 FROM nodes n \
                 WHERE n.node_type NOT IN ('struct','function','trait','enum', \
                       'impl_block','constant','macro','module','class','component') \
                   AND n.embedding IS NOT NULL \
                   AND n.id != $1 \
                   AND NOT EXISTS ( \
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

    // ------------------------------------------------------------------
    // Capability 6: Coverage Analysis
    // ------------------------------------------------------------------

    /// Detect orphaned code (no Component parent) and unimplemented specs
    /// (spec concepts with no IMPLEMENTS_INTENT edges).
    pub async fn coverage_analysis(&self) -> Result<CoverageResult> {
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

        // Orphan code: code nodes with no PART_OF_COMPONENT edge.
        let orphan_rows: Vec<(uuid::Uuid, String, String, String)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                    COALESCE(n.properties->>'file_path', '') \
             FROM nodes n \
             WHERE n.node_type = ANY($1) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM edges e \
                 WHERE e.source_node_id = n.id \
                   AND e.rel_type = 'PART_OF_COMPONENT' \
               ) \
             ORDER BY n.canonical_name \
             LIMIT 200",
        )
        .bind(&code_types[..])
        .fetch_all(self.repo.pool())
        .await?;

        let orphan_code: Vec<CoverageItem> = orphan_rows
            .into_iter()
            .map(|(id, name, ntype, path)| CoverageItem {
                node_id: id,
                name,
                node_type: ntype,
                file_path: if path.is_empty() { None } else { Some(path) },
                reason: "No PART_OF_COMPONENT edge to any Component".to_string(),
            })
            .collect();

        // Unimplemented specs: concept/entity nodes from spec sources
        // with no inbound IMPLEMENTS_INTENT edges.
        let unimpl_rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT DISTINCT n.id, n.canonical_name, n.node_type \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON ex.chunk_id = c.id \
             JOIN sources s ON c.source_id = s.id \
             WHERE n.node_type NOT IN ('struct','function','trait','enum', \
                   'impl_block','constant','macro','module','class','component') \
               AND (s.uri LIKE '%spec/%' OR s.uri LIKE '%docs/adr/%') \
               AND NOT EXISTS ( \
                 SELECT 1 FROM edges e \
                 WHERE e.target_node_id = n.id \
                   AND e.rel_type = 'IMPLEMENTS_INTENT' \
               ) \
             ORDER BY n.canonical_name \
             LIMIT 200",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let unimplemented_specs: Vec<CoverageItem> = unimpl_rows
            .into_iter()
            .map(|(id, name, ntype)| CoverageItem {
                node_id: id,
                name,
                node_type: ntype,
                file_path: None,
                reason: "Spec concept with no IMPLEMENTS_INTENT edge".to_string(),
            })
            .collect();

        // Coverage score: (spec concepts with implementation / total spec concepts).
        let total_spec: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT n.id) \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON ex.chunk_id = c.id \
             JOIN sources s ON c.source_id = s.id \
             WHERE n.node_type NOT IN ('struct','function','trait','enum', \
                   'impl_block','constant','macro','module','class','component') \
               AND (s.uri LIKE '%spec/%' OR s.uri LIKE '%docs/adr/%')",
        )
        .fetch_one(self.repo.pool())
        .await?;

        let implemented: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT n.id) \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON ex.chunk_id = c.id \
             JOIN sources s ON c.source_id = s.id \
             WHERE n.node_type NOT IN ('struct','function','trait','enum', \
                   'impl_block','constant','macro','module','class','component') \
               AND (s.uri LIKE '%spec/%' OR s.uri LIKE '%docs/adr/%') \
               AND EXISTS ( \
                 SELECT 1 FROM edges e \
                 WHERE e.target_node_id = n.id \
                   AND e.rel_type = 'IMPLEMENTS_INTENT' \
               )",
        )
        .fetch_one(self.repo.pool())
        .await?;

        let coverage_score = if total_spec > 0 {
            implemented as f64 / total_spec as f64
        } else {
            0.0
        };

        tracing::info!(
            orphan_code = orphan_code.len(),
            unimplemented_specs = unimplemented_specs.len(),
            coverage_score,
            total_spec,
            implemented,
            "coverage analysis complete"
        );

        Ok(CoverageResult {
            orphan_code,
            unimplemented_specs,
            coverage_score,
        })
    }

    // ------------------------------------------------------------------
    // Capability 2: Architecture Erosion Detection
    // ------------------------------------------------------------------

    /// Detect components where code has drifted from design intent.
    ///
    /// For each Component with an embedding, compute the mean cosine distance
    /// between the component's embedding and all code nodes linked via
    /// PART_OF_COMPONENT. Components above the threshold are flagged.
    pub async fn detect_erosion(&self, threshold: f64) -> Result<ErosionResult> {
        // Fetch all component nodes with embeddings.
        let components: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT id, canonical_name, COALESCE(description, '') \
             FROM nodes \
             WHERE node_type = 'component' AND embedding IS NOT NULL",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let total_components = components.len() as u64;
        let mut eroded = Vec::new();

        for (comp_id, comp_name, description) in &components {
            // Find all code nodes linked to this component via
            // PART_OF_COMPONENT that have embeddings.
            let code_nodes: Vec<(uuid::Uuid, String, Option<String>, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, \
                        n.properties->>'semantic_summary', \
                        (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
                 FROM nodes n \
                 JOIN edges e ON e.source_node_id = n.id \
                 WHERE e.target_node_id = $1 \
                   AND e.rel_type = 'PART_OF_COMPONENT' \
                   AND n.embedding IS NOT NULL \
                 ORDER BY dist DESC",
            )
            .bind(comp_id)
            .fetch_all(self.repo.pool())
            .await?;

            if code_nodes.is_empty() {
                continue;
            }

            let avg_dist: f64 =
                code_nodes.iter().map(|(_, _, _, d)| d).sum::<f64>() / code_nodes.len() as f64;
            let drift_score = avg_dist; // 0 = perfect alignment, 1 = orthogonal

            if drift_score < threshold {
                continue;
            }

            // Top 5 most divergent nodes.
            let divergent_nodes: Vec<DivergentNode> = code_nodes
                .iter()
                .take(5)
                .map(|(id, name, summary, dist)| DivergentNode {
                    node_id: *id,
                    name: name.clone(),
                    summary: summary.clone(),
                    distance: *dist,
                })
                .collect();

            eroded.push(ErosionItem {
                component_id: *comp_id,
                component_name: comp_name.clone(),
                spec_intent: description.clone(),
                drift_score,
                divergent_nodes,
            });
        }

        eroded.sort_by(|a, b| {
            b.drift_score
                .partial_cmp(&a.drift_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        tracing::info!(
            total = total_components,
            eroded = eroded.len(),
            threshold,
            "erosion detection complete"
        );

        Ok(ErosionResult {
            eroded_components: eroded,
            total_components,
        })
    }

    // ------------------------------------------------------------------
    // Capability 4: Blast-Radius Simulation
    // ------------------------------------------------------------------

    /// Maximum affected nodes to collect before stopping BFS early.
    const BLAST_RADIUS_NODE_CAP: usize = 500;

    /// Estimate the blast radius of changing a given node.
    ///
    /// Traverses the graph sidecar outward from the target node up to
    /// `max_hops` hops, collecting affected nodes grouped by hop distance.
    /// Stops early if `BLAST_RADIUS_NODE_CAP` nodes are collected.
    pub async fn blast_radius(
        &self,
        target_name: &str,
        max_hops: usize,
    ) -> Result<BlastRadiusResult> {
        // Find the target node by canonical name.
        let target: Option<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT id, canonical_name, node_type FROM nodes \
             WHERE LOWER(canonical_name) = LOWER($1) LIMIT 1",
        )
        .bind(target_name)
        .fetch_optional(self.repo.pool())
        .await?;

        let Some((target_id, target_canonical, target_type)) = target else {
            return Err(Error::NotFound {
                entity_type: "node",
                id: target_name.to_string(),
            });
        };

        // Find the component this node belongs to.
        let component: Option<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name \
             FROM nodes n \
             JOIN edges e ON e.target_node_id = n.id \
             WHERE e.source_node_id = $1 \
               AND e.rel_type = 'PART_OF_COMPONENT' \
             LIMIT 1",
        )
        .bind(target_id)
        .fetch_optional(self.repo.pool())
        .await?;

        // BFS through the graph sidecar.
        let graph = self.graph.read().await;
        let mut affected: Vec<BlastRadiusHop> = Vec::new();
        let mut visited = std::collections::HashSet::new();

        if let Some(start_idx) = graph.node_index(target_id) {
            visited.insert(start_idx);
            let mut frontier = vec![start_idx];

            let mut total_collected = 0usize;

            for hop in 1..=max_hops {
                if total_collected >= Self::BLAST_RADIUS_NODE_CAP {
                    break;
                }
                let mut next_frontier = Vec::new();
                let mut hop_nodes = Vec::new();

                for &idx in &frontier {
                    // Outgoing edges.
                    for edge_ref in graph.graph().edges(idx) {
                        let neighbor = edge_ref.target();
                        if visited.insert(neighbor) {
                            if let Some(meta) = graph.graph().node_weight(neighbor) {
                                hop_nodes.push(AffectedNode {
                                    node_id: meta.id,
                                    name: meta.canonical_name.clone(),
                                    node_type: meta.node_type.clone(),
                                    relationship: edge_ref.weight().rel_type.clone(),
                                });
                            }
                            next_frontier.push(neighbor);
                        }
                    }

                    // Incoming edges (callers, etc.).
                    use petgraph::Direction;
                    for edge_ref in graph.graph().edges_directed(idx, Direction::Incoming) {
                        let neighbor = edge_ref.source();
                        if visited.insert(neighbor) {
                            if let Some(meta) = graph.graph().node_weight(neighbor) {
                                hop_nodes.push(AffectedNode {
                                    node_id: meta.id,
                                    name: meta.canonical_name.clone(),
                                    node_type: meta.node_type.clone(),
                                    relationship: format!(
                                        "{} (incoming)",
                                        edge_ref.weight().rel_type
                                    ),
                                });
                            }
                            next_frontier.push(neighbor);
                        }
                    }
                }

                total_collected += hop_nodes.len();
                if !hop_nodes.is_empty() {
                    affected.push(BlastRadiusHop {
                        hop_distance: hop,
                        nodes: hop_nodes,
                    });
                }
                frontier = next_frontier;
            }
        }

        let total_affected: usize = affected.iter().map(|h| h.nodes.len()).sum();

        Ok(BlastRadiusResult {
            target: TargetInfo {
                node_id: target_id,
                name: target_canonical,
                node_type: target_type,
                component: component.map(|(_, name)| name),
            },
            affected_by_hop: affected,
            total_affected,
        })
    }
}

/// Target node info for blast radius.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TargetInfo {
    /// Target node UUID.
    pub node_id: uuid::Uuid,
    /// Target node name.
    pub name: String,
    /// Target node type.
    pub node_type: String,
    /// Parent component name, if any.
    pub component: Option<String>,
}

/// A node affected by the blast radius.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AffectedNode {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// Relationship type connecting to the blast origin.
    pub relationship: String,
}

/// Nodes affected at a specific hop distance.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlastRadiusHop {
    /// Hop distance from the target.
    pub hop_distance: usize,
    /// Nodes at this hop distance.
    pub nodes: Vec<AffectedNode>,
}

/// Result of blast-radius simulation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlastRadiusResult {
    /// The target node being analyzed.
    pub target: TargetInfo,
    /// Affected nodes grouped by hop distance.
    pub affected_by_hop: Vec<BlastRadiusHop>,
    /// Total number of affected nodes.
    pub total_affected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_defs_are_unique() {
        let names: Vec<&str> = COMPONENT_DEFS.iter().map(|(n, _)| *n).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "duplicate component names");
    }

    #[test]
    fn module_path_mappings_reference_valid_components() {
        let comp_names: Vec<&str> = COMPONENT_DEFS.iter().map(|(n, _)| *n).collect();
        for (prefix, comp) in MODULE_PATH_MAPPINGS {
            assert!(
                comp_names.contains(comp),
                "module path mapping {:?} references unknown component {:?}",
                prefix,
                comp
            );
        }
    }

    #[test]
    fn bootstrap_result_serializes() {
        let result = BootstrapResult {
            components_created: 5,
            components_existing: 4,
            components_embedded: 5,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("components_created"));
    }

    #[test]
    fn linking_result_serializes() {
        let result = LinkingResult {
            part_of_edges: 10,
            intent_edges: 5,
            basis_edges: 3,
            skipped_existing: 2,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("part_of_edges"));
        assert!(json.contains("intent_edges"));
    }

    #[test]
    fn coverage_item_serializes() {
        let item = CoverageItem {
            node_id: uuid::Uuid::new_v4(),
            name: "test_fn".into(),
            node_type: "function".into(),
            file_path: Some("src/test.rs".into()),
            reason: "orphaned".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("test_fn"));
    }

    #[test]
    fn erosion_item_serializes() {
        let item = ErosionItem {
            component_id: uuid::Uuid::new_v4(),
            component_name: "Search Fusion".into(),
            spec_intent: "RRF fusion".into(),
            drift_score: 0.42,
            divergent_nodes: vec![DivergentNode {
                node_id: uuid::Uuid::new_v4(),
                name: "fuse_results".into(),
                summary: Some("CC fusion".into()),
                distance: 0.55,
            }],
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("Search Fusion"));
        assert!(json.contains("0.42"));
    }

    #[test]
    fn blast_radius_result_serializes() {
        let result = BlastRadiusResult {
            target: TargetInfo {
                node_id: uuid::Uuid::new_v4(),
                name: "run_pipeline".into(),
                node_type: "function".into(),
                component: Some("Ingestion Pipeline".into()),
            },
            affected_by_hop: vec![BlastRadiusHop {
                hop_distance: 1,
                nodes: vec![AffectedNode {
                    node_id: uuid::Uuid::new_v4(),
                    name: "embed_batch".into(),
                    node_type: "function".into(),
                    relationship: "CALLS".into(),
                }],
            }],
            total_affected: 1,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("run_pipeline"));
        assert!(json.contains("embed_batch"));
    }
}
