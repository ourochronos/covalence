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
use crate::ingestion::ChatBackend;
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

/// Module path segments mapped to component names for PART_OF_COMPONENT linking.
///
/// Code nodes whose source URI contains one of these path segments
/// are linked to the corresponding Component.
const MODULE_PATH_MAPPINGS: &[(&str, &str)] = &[
    // Ingestion Pipeline — specific submodules first, then catch-all
    ("src/ingestion/pipeline", "Ingestion Pipeline"),
    ("src/ingestion/embedder", "Ingestion Pipeline"),
    ("src/ingestion/converter", "Ingestion Pipeline"),
    ("src/ingestion/chunker", "Ingestion Pipeline"),
    ("src/ingestion/code_chunker", "Ingestion Pipeline"),
    ("src/ingestion/normalize", "Ingestion Pipeline"),
    ("src/ingestion/source_profile", "Ingestion Pipeline"),
    ("src/ingestion/extractor", "Ingestion Pipeline"),
    ("src/ingestion/ast_extractor", "Ingestion Pipeline"),
    ("src/ingestion/sidecar_extractor", "Ingestion Pipeline"),
    ("src/ingestion/gliner_extractor", "Ingestion Pipeline"),
    ("src/ingestion/openai_embedder", "Ingestion Pipeline"),
    ("src/ingestion/voyage", "Ingestion Pipeline"),
    ("src/ingestion/landscape", "Ingestion Pipeline"),
    ("src/ingestion/projection", "Ingestion Pipeline"),
    ("src/ingestion/parser", "Ingestion Pipeline"),
    ("ingestion_helpers", "Ingestion Pipeline"),
    ("chunk_quality", "Ingestion Pipeline"),
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
    ("types/opinion", "Epistemic Model"),
    // Consolidation
    ("src/consolidation", "Consolidation"),
    ("src/services/consolidation", "Consolidation"),
    // API Layer
    ("src/routes", "API Layer"),
    ("src/handlers", "API Layer"),
    ("src/middleware", "API Layer"),
    ("src/state", "API Layer"),
    ("src/openapi", "API Layer"),
    ("covalence-api", "API Layer"),
    // CLI
    ("cmd/", "CLI"),
    ("internal/", "CLI"),
    // Services (general)
    ("src/services/admin", "API Layer"),
    ("src/services/source", "Ingestion Pipeline"),
    ("src/services/node", "Graph Sidecar"),
    ("src/services/edge", "Graph Sidecar"),
    ("src/services/noise_filter", "Ingestion Pipeline"),
    ("src/services/analysis", "Graph Sidecar"),
    ("src/services/memory", "API Layer"),
    // Core domain models & types — part of Graph Sidecar (data model)
    ("src/models/", "Graph Sidecar"),
    ("src/types/", "Graph Sidecar"),
    ("src/error", "Graph Sidecar"),
    ("src/config", "API Layer"),
    // Storage layer
    ("src/storage/", "Graph Sidecar"),
    // Eval harness
    ("covalence-eval", "Ingestion Pipeline"),
    // Migrations
    ("covalence-migrations", "API Layer"),
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
    chat_backend: Option<Arc<dyn ChatBackend>>,
    node_embed_dim: usize,
}

impl AnalysisService {
    /// Create a new analysis service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self {
            repo,
            graph,
            embedder: None,
            chat_backend: None,
            node_embed_dim: 256,
        }
    }

    /// Set the embedder for component description embedding.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Set the chat backend for LLM-driven analysis.
    pub fn with_chat_backend(mut self, backend: Option<Arc<dyn ChatBackend>>) -> Self {
        self.chat_backend = backend;
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
        use petgraph::Direction;
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

    // ------------------------------------------------------------------
    // Capability 3: Whitespace Roadmap (Gap Analysis)
    // ------------------------------------------------------------------

    /// Maximum research gaps to return.
    const MAX_WHITESPACE_GAPS: usize = 50;

    /// Detect research areas with no corresponding implementation.
    ///
    /// Groups research-domain nodes by their source article and checks
    /// whether any node in each group is connected to a Component via
    /// THEORETICAL_BASIS edges. Sources with zero bridges are "whitespace"
    /// — theory we've studied but haven't acted on.
    pub async fn whitespace_roadmap(
        &self,
        min_cluster_size: usize,
        domain_filter: Option<&str>,
    ) -> Result<WhitespaceResult> {
        // Find research sources: document-type sources that are NOT spec/ADR/vision.
        // Group their extracted nodes and check for bridge edges.
        let rows: Vec<(uuid::Uuid, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT s.id, s.title, s.uri, \
                    COUNT(DISTINCT n.id) AS node_count, \
                    COUNT(DISTINCT CASE WHEN e.id IS NOT NULL THEN n.id END) AS bridged \
             FROM sources s \
             JOIN chunks c ON c.source_id = s.id \
             JOIN extractions ex ON ex.chunk_id = c.id \
             JOIN nodes n ON n.id = ex.entity_id \
             LEFT JOIN edges e ON e.target_node_id = n.id \
                               AND e.rel_type = 'THEORETICAL_BASIS' \
             WHERE s.source_type = 'document' \
               AND COALESCE(s.uri, '') NOT LIKE '%spec/%' \
               AND COALESCE(s.uri, '') NOT LIKE '%docs/adr/%' \
               AND COALESCE(s.uri, '') NOT LIKE '%VISION%' \
               AND COALESCE(s.uri, '') NOT LIKE '%CLAUDE%' \
               AND COALESCE(s.uri, '') NOT LIKE '%MILESTONES%' \
               AND ($3::text IS NULL \
                    OR LOWER(s.title) LIKE '%' || LOWER($3::text) || '%' \
                    OR LOWER(COALESCE(s.uri, '')) LIKE '%' || LOWER($3::text) || '%') \
             GROUP BY s.id, s.title, s.uri \
             HAVING COUNT(DISTINCT n.id) >= $1 \
             ORDER BY COUNT(DISTINCT n.id) DESC \
             LIMIT $2",
        )
        .bind(min_cluster_size as i64)
        .bind(Self::MAX_WHITESPACE_GAPS as i64)
        .bind(domain_filter)
        .fetch_all(self.repo.pool())
        .await?;

        let mut gaps = Vec::new();
        let mut total_research = 0u64;
        let mut total_unbridged = 0u64;

        for (source_id, title, uri, node_count, bridged_count) in &rows {
            // Domain filter is now applied in SQL ($3 parameter), so
            // all rows here already match the filter.
            total_research += 1;

            if *bridged_count > 0 {
                continue; // This source has at least one bridge edge.
            }

            total_unbridged += 1;

            // Fetch representative node names for this source.
            let node_names: Vec<(String, String)> = sqlx::query_as(
                "SELECT DISTINCT n.canonical_name, n.node_type \
                 FROM nodes n \
                 JOIN extractions ex ON ex.entity_id = n.id \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 WHERE c.source_id = $1 \
                 ORDER BY n.canonical_name \
                 LIMIT 10",
            )
            .bind(source_id)
            .fetch_all(self.repo.pool())
            .await?;

            // Check for IMPLEMENTS_INTENT connections (spec coverage).
            let spec_connected: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT comp.canonical_name \
                 FROM nodes comp \
                 JOIN edges e ON e.source_node_id = comp.id \
                 WHERE e.rel_type = 'IMPLEMENTS_INTENT' \
                   AND comp.node_type = 'component' \
                   AND e.target_node_id IN ( \
                     SELECT n.id FROM nodes n \
                     JOIN extractions ex ON ex.entity_id = n.id \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     WHERE c.source_id = $1 \
                   )",
            )
            .bind(source_id)
            .fetch_all(self.repo.pool())
            .await?;

            // Find Component nodes connected via THEORETICAL_BASIS
            // to any entity extracted from this source.
            let comp_connected: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT comp.canonical_name \
                 FROM nodes comp \
                 JOIN edges e ON e.source_node_id = comp.id \
                 WHERE e.rel_type = 'THEORETICAL_BASIS' \
                   AND comp.node_type = 'component' \
                   AND e.target_node_id IN ( \
                     SELECT n.id FROM nodes n \
                     JOIN extractions ex ON ex.entity_id = n.id \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     WHERE c.source_id = $1 \
                   )",
            )
            .bind(source_id)
            .fetch_all(self.repo.pool())
            .await?;

            gaps.push(WhitespaceGap {
                source_id: *source_id,
                title: title.clone(),
                uri: uri.clone(),
                node_count: *node_count as u64,
                representative_nodes: node_names
                    .into_iter()
                    .map(|(name, ntype)| WhitespaceNode {
                        name,
                        node_type: ntype,
                    })
                    .collect(),
                connected_components: comp_connected.into_iter().map(|(name,)| name).collect(),
                connected_spec_topics: spec_connected.into_iter().map(|(name,)| name).collect(),
                assessment: if *node_count > 10 {
                    format!(
                        "Dense research cluster ({} entities) with zero THEORETICAL_BASIS \
                         bridge edges to any component.",
                        node_count
                    )
                } else {
                    format!(
                        "{} entities with no bridge edges to any component.",
                        node_count
                    )
                },
            });
        }

        // Sort by node count descending (densest unbridged clusters first).
        gaps.sort_by(|a, b| b.node_count.cmp(&a.node_count));

        let whitespace_score = if total_research > 0 {
            total_unbridged as f64 / total_research as f64
        } else {
            0.0
        };

        tracing::info!(
            total_research,
            total_unbridged,
            whitespace_score = format!("{:.1}%", whitespace_score * 100.0),
            gaps = gaps.len(),
            "whitespace roadmap analysis complete"
        );

        Ok(WhitespaceResult {
            gaps,
            total_research_sources: total_research,
            unbridged_sources: total_unbridged,
            whitespace_score,
        })
    }

    // ------------------------------------------------------------------
    // Capability 1: Research-to-Execution Verification
    // ------------------------------------------------------------------

    /// Maximum code nodes to compare per component.
    const MAX_VERIFY_CODE_NODES: i64 = 20;

    /// Verify whether code implementation aligns with research claims.
    ///
    /// Finds research nodes matching the query, traces through
    /// THEORETICAL_BASIS edges to Components, then through
    /// PART_OF_COMPONENT edges to code nodes. Compares research
    /// statement embeddings with code semantic summary embeddings
    /// to find alignment and divergence.
    pub async fn verify_implementation(
        &self,
        research_query: &str,
        component_filter: Option<&str>,
    ) -> Result<VerificationResult> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured".into()))?;

        // Embed the research query.
        let query_embeddings = embedder.embed(&[research_query.to_string()]).await?;
        let query_vec = query_embeddings
            .first()
            .ok_or_else(|| Error::Config("embedder returned empty result".into()))?;
        let query_truncated = truncate_and_validate(query_vec, self.node_embed_dim, "node")?;

        // Find research-domain nodes closest to the query.
        let research_nodes: Vec<(uuid::Uuid, String, String, Option<String>, f64)> =
            sqlx::query_as(
                "SELECT n.id, n.canonical_name, n.node_type, \
                        n.properties->>'semantic_summary', \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.node_type != 'component' \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND s.source_type = 'document' \
                       AND COALESCE(s.uri, '') NOT LIKE '%spec/%' \
                       AND COALESCE(s.uri, '') NOT LIKE '%docs/adr/%' \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT 10",
            )
            .bind(&query_truncated)
            .fetch_all(self.repo.pool())
            .await?;

        if research_nodes.is_empty() {
            return Ok(VerificationResult {
                research_query: research_query.to_string(),
                research_matches: Vec::new(),
                code_matches: Vec::new(),
                alignment_score: None,
                component: None,
            });
        }

        // Find which Components these research nodes connect to via
        // THEORETICAL_BASIS.
        let research_ids: Vec<uuid::Uuid> = research_nodes.iter().map(|(id, ..)| *id).collect();
        let components: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT DISTINCT comp.id, comp.canonical_name \
             FROM nodes comp \
             JOIN edges e ON e.source_node_id = comp.id \
             WHERE comp.node_type = 'component' \
               AND e.rel_type = 'THEORETICAL_BASIS' \
               AND e.target_node_id = ANY($1)",
        )
        .bind(&research_ids)
        .fetch_all(self.repo.pool())
        .await?;

        // Apply component filter if specified.
        let filtered_components: Vec<(uuid::Uuid, String)> = if let Some(filter) = component_filter
        {
            let lower = filter.to_lowercase();
            components
                .into_iter()
                .filter(|(_, name)| name.to_lowercase().contains(&lower))
                .collect()
        } else {
            components
        };

        let component_name = filtered_components.first().map(|(_, n)| n.clone());

        // Find code nodes linked to these components via PART_OF_COMPONENT.
        let comp_ids: Vec<uuid::Uuid> = filtered_components.iter().map(|(id, _)| *id).collect();
        let code_nodes: Vec<(uuid::Uuid, String, String, Option<String>, f64)> =
            if comp_ids.is_empty() {
                Vec::new()
            } else {
                sqlx::query_as(
                    "SELECT n.id, n.canonical_name, n.node_type, \
                        n.properties->>'semantic_summary', \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 JOIN edges e ON e.source_node_id = n.id \
                 WHERE e.rel_type = 'PART_OF_COMPONENT' \
                   AND e.target_node_id = ANY($2) \
                   AND n.embedding IS NOT NULL \
                 ORDER BY dist ASC \
                 LIMIT $3",
                )
                .bind(&query_truncated)
                .bind(&comp_ids)
                .bind(Self::MAX_VERIFY_CODE_NODES)
                .fetch_all(self.repo.pool())
                .await?
            };

        // Compute alignment score: mean cosine similarity between
        // research nodes and code nodes (via the query vector as proxy).
        let alignment_score = if !research_nodes.is_empty() && !code_nodes.is_empty() {
            let research_mean: f64 = research_nodes.iter().map(|(.., d)| 1.0 - d).sum::<f64>()
                / research_nodes.len() as f64;
            let code_mean: f64 =
                code_nodes.iter().map(|(.., d)| 1.0 - d).sum::<f64>() / code_nodes.len() as f64;
            Some((research_mean + code_mean) / 2.0)
        } else {
            None
        };

        let research_matches: Vec<VerificationMatch> = research_nodes
            .into_iter()
            .map(|(id, name, ntype, summary, dist)| VerificationMatch {
                node_id: id,
                name,
                node_type: ntype,
                summary,
                distance: dist,
                domain: "research".to_string(),
            })
            .collect();

        let code_matches: Vec<VerificationMatch> = code_nodes
            .into_iter()
            .map(|(id, name, ntype, summary, dist)| VerificationMatch {
                node_id: id,
                name,
                node_type: ntype,
                summary,
                distance: dist,
                domain: "code".to_string(),
            })
            .collect();

        tracing::info!(
            query = %research_query,
            research = research_matches.len(),
            code = code_matches.len(),
            alignment = ?alignment_score,
            component = ?component_name,
            "research-to-execution verification complete"
        );

        Ok(VerificationResult {
            research_query: research_query.to_string(),
            research_matches,
            code_matches,
            alignment_score,
            component: component_name,
        })
    }

    // ------------------------------------------------------------------
    // Capability 5: Dialectical Design Partner
    // ------------------------------------------------------------------

    /// Maximum evidence nodes per search direction.
    const MAX_CRITIQUE_EVIDENCE: i64 = 15;

    /// Generate a dialectical critique of a design proposal.
    ///
    /// Embeds the proposal text and searches the graph for semantically
    /// related evidence across all three domains (research, spec, code).
    /// When a chat backend is available, uses LLM synthesis to generate
    /// structured counter-arguments and supporting arguments.
    pub async fn critique(&self, proposal: &str) -> Result<CritiqueResult> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured".into()))?;

        // Embed the proposal text.
        let proposal_embeddings = embedder.embed(&[proposal.to_string()]).await?;
        let proposal_vec = proposal_embeddings
            .first()
            .ok_or_else(|| Error::Config("embedder returned empty result".into()))?;
        let proposal_truncated = truncate_and_validate(proposal_vec, self.node_embed_dim, "node")?;

        // Search for related evidence across all domains.
        // Research evidence (non-spec, non-code documents).
        let research_evidence: Vec<(uuid::Uuid, String, String, Option<String>, f64)> =
            sqlx::query_as(
                "SELECT n.id, n.canonical_name, n.node_type, \
                        n.description, \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.node_type NOT IN ('component') \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND s.source_type = 'document' \
                       AND COALESCE(s.uri, '') NOT LIKE '%spec/%' \
                       AND COALESCE(s.uri, '') NOT LIKE '%docs/adr/%' \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(&proposal_truncated)
            .bind(Self::MAX_CRITIQUE_EVIDENCE)
            .fetch_all(self.repo.pool())
            .await?;

        // Spec/design evidence.
        let spec_evidence: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                        n.description, \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.node_type NOT IN ('component') \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND (s.uri LIKE '%spec/%' OR s.uri LIKE '%docs/adr/%') \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
        )
        .bind(&proposal_truncated)
        .bind(Self::MAX_CRITIQUE_EVIDENCE)
        .fetch_all(self.repo.pool())
        .await?;

        // Code evidence (code-type sources).
        let code_evidence: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                        COALESCE(n.properties->>'semantic_summary', \
                                 n.description), \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.node_type NOT IN ('component') \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND s.source_type = 'code' \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
        )
        .bind(&proposal_truncated)
        .bind(Self::MAX_CRITIQUE_EVIDENCE)
        .fetch_all(self.repo.pool())
        .await?;

        let to_evidence = |rows: Vec<(uuid::Uuid, String, String, Option<String>, f64)>,
                           domain: &str|
         -> Vec<CritiqueEvidence> {
            rows.into_iter()
                .map(|(id, name, ntype, desc, dist)| CritiqueEvidence {
                    node_id: id,
                    name,
                    node_type: ntype,
                    description: desc,
                    distance: dist,
                    domain: domain.to_string(),
                })
                .collect()
        };

        let all_research = to_evidence(research_evidence, "research");
        let all_spec = to_evidence(spec_evidence, "spec");
        let all_code = to_evidence(code_evidence, "code");

        // If a chat backend is available, ask the LLM to synthesize
        // a dialectical critique from the evidence.
        let synthesis = if let Some(ref backend) = self.chat_backend {
            self.synthesize_critique(backend, proposal, &all_research, &all_spec, &all_code)
                .await
                .ok() // Non-fatal: return evidence without synthesis on LLM failure.
        } else {
            None
        };

        tracing::info!(
            research = all_research.len(),
            spec = all_spec.len(),
            code = all_code.len(),
            has_synthesis = synthesis.is_some(),
            "dialectical critique complete"
        );

        Ok(CritiqueResult {
            proposal: proposal.to_string(),
            research_evidence: all_research,
            spec_evidence: all_spec,
            code_evidence: all_code,
            synthesis,
        })
    }

    /// Use the chat backend to synthesize a structured critique.
    async fn synthesize_critique(
        &self,
        backend: &Arc<dyn ChatBackend>,
        proposal: &str,
        research: &[CritiqueEvidence],
        spec: &[CritiqueEvidence],
        code: &[CritiqueEvidence],
    ) -> Result<CritiqueSynthesis> {
        let evidence_summary = |items: &[CritiqueEvidence], label: &str| -> String {
            if items.is_empty() {
                return format!("No {label} evidence found.");
            }
            let mut s = format!("**{label} evidence:**\n");
            for (i, e) in items.iter().take(5).enumerate() {
                s.push_str(&format!(
                    "{}. {} ({}, dist={:.3}){}\n",
                    i + 1,
                    e.name,
                    e.node_type,
                    e.distance,
                    e.description
                        .as_deref()
                        .map(|d| {
                            let mut end = d.len().min(120);
                            while end > 0 && !d.is_char_boundary(end) {
                                end -= 1;
                            }
                            format!(": {}", &d[..end])
                        })
                        .unwrap_or_default()
                ));
            }
            s
        };

        let system = "You are a critical design reviewer for a knowledge engine called \
                       Covalence. Given a design proposal and evidence from the system's \
                       research papers, spec documents, and codebase, generate a structured \
                       dialectical critique. Be specific, cite evidence by name, and \
                       identify both counter-arguments and supporting arguments.";

        let user = format!(
            "## Design Proposal\n{proposal}\n\n\
             ## Evidence from the Knowledge Graph\n\
             {}\n{}\n{}\n\n\
             Respond with a JSON object:\n\
             {{\n\
               \"counter_arguments\": [\n\
                 {{\"claim\": \"...\", \"evidence\": [\"...\"], \"strength\": \"strong|moderate|weak\"}}\n\
               ],\n\
               \"supporting_arguments\": [\n\
                 {{\"claim\": \"...\", \"evidence\": [\"...\"]}}\n\
               ],\n\
               \"recommendation\": \"...\"\n\
             }}",
            evidence_summary(research, "Research"),
            evidence_summary(spec, "Spec/Design"),
            evidence_summary(code, "Code"),
        );

        let response = backend.chat(system, &user, true, 0.3).await?;

        // Parse the LLM response as JSON.
        let synthesis: CritiqueSynthesis = serde_json::from_str(&response)
            .or_else(|_| {
                // Try to extract JSON from markdown code block.
                let trimmed = response.trim();
                let json_str = if let Some(start) = trimmed.find('{') {
                    if let Some(end) = trimmed.rfind('}') {
                        &trimmed[start..=end]
                    } else {
                        trimmed
                    }
                } else {
                    trimmed
                };
                serde_json::from_str(json_str)
            })
            .map_err(|e| {
                let mut end = response.len().min(200);
                while end > 0 && !response.is_char_boundary(end) {
                    end -= 1;
                }
                tracing::warn!(
                    error = %e,
                    response_preview = &response[..end],
                    "failed to parse critique synthesis"
                );
                Error::Ingestion(format!("failed to parse LLM critique: {e}"))
            })?;

        Ok(synthesis)
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

// ------------------------------------------------------------------
// Whitespace roadmap types
// ------------------------------------------------------------------

/// A node representative in a whitespace gap.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitespaceNode {
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
}

/// A research cluster with no bridge edges to any Component.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitespaceGap {
    /// Source UUID.
    pub source_id: uuid::Uuid,
    /// Source title.
    pub title: String,
    /// Source URI.
    pub uri: Option<String>,
    /// Number of entities extracted from this source.
    pub node_count: u64,
    /// Representative entity names from the source.
    pub representative_nodes: Vec<WhitespaceNode>,
    /// Components connected via any bridge edge.
    pub connected_components: Vec<String>,
    /// Spec topics connected via IMPLEMENTS_INTENT.
    pub connected_spec_topics: Vec<String>,
    /// Human-readable gap assessment.
    pub assessment: String,
}

/// Result of whitespace roadmap analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitespaceResult {
    /// Research clusters with no bridge edges.
    pub gaps: Vec<WhitespaceGap>,
    /// Total research sources analyzed.
    pub total_research_sources: u64,
    /// Sources with zero bridge edges.
    pub unbridged_sources: u64,
    /// Fraction of research sources that are unbridged.
    pub whitespace_score: f64,
}

// ------------------------------------------------------------------
// Research-to-execution verification types
// ------------------------------------------------------------------

/// A matched node in verification analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerificationMatch {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// Semantic summary or description.
    pub summary: Option<String>,
    /// Cosine distance from the query.
    pub distance: f64,
    /// Domain: "research" or "code".
    pub domain: String,
}

/// Result of research-to-execution verification.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerificationResult {
    /// The research query searched for.
    pub research_query: String,
    /// Matched research-domain nodes.
    pub research_matches: Vec<VerificationMatch>,
    /// Matched code-domain nodes via Component bridges.
    pub code_matches: Vec<VerificationMatch>,
    /// Alignment score (mean cosine similarity across domains).
    pub alignment_score: Option<f64>,
    /// Component that bridges the domains (if found).
    pub component: Option<String>,
}

// ------------------------------------------------------------------
// Dialectical critique types
// ------------------------------------------------------------------

/// A piece of evidence from the knowledge graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CritiqueEvidence {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// Node description or summary.
    pub description: Option<String>,
    /// Cosine distance from the proposal embedding.
    pub distance: f64,
    /// Domain: "research", "spec", or "code".
    pub domain: String,
}

/// A counter-argument in the critique.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CounterArgument {
    /// The claim being made against the proposal.
    pub claim: String,
    /// Evidence supporting the counter-argument.
    pub evidence: Vec<String>,
    /// Strength of the argument: "strong", "moderate", or "weak".
    pub strength: String,
}

/// A supporting argument in the critique.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SupportingArgument {
    /// The claim supporting the proposal.
    pub claim: String,
    /// Evidence supporting this argument.
    pub evidence: Vec<String>,
}

/// LLM-synthesized dialectical critique.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CritiqueSynthesis {
    /// Arguments against the proposal.
    pub counter_arguments: Vec<CounterArgument>,
    /// Arguments supporting the proposal.
    pub supporting_arguments: Vec<SupportingArgument>,
    /// Overall recommendation.
    pub recommendation: String,
}

/// Result of dialectical critique analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CritiqueResult {
    /// The original proposal text.
    pub proposal: String,
    /// Research-domain evidence related to the proposal.
    pub research_evidence: Vec<CritiqueEvidence>,
    /// Spec/design evidence related to the proposal.
    pub spec_evidence: Vec<CritiqueEvidence>,
    /// Code evidence related to the proposal.
    pub code_evidence: Vec<CritiqueEvidence>,
    /// LLM-synthesized critique (None if no chat backend available).
    pub synthesis: Option<CritiqueSynthesis>,
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

    #[test]
    fn whitespace_gap_serializes() {
        let gap = WhitespaceGap {
            source_id: uuid::Uuid::new_v4(),
            title: "HDBSCAN Paper".into(),
            uri: Some("https://arxiv.org/abs/hdbscan".into()),
            node_count: 12,
            representative_nodes: vec![WhitespaceNode {
                name: "HDBSCAN".into(),
                node_type: "algorithm".into(),
            }],
            connected_components: Vec::new(),
            connected_spec_topics: Vec::new(),
            assessment: "Dense research cluster".into(),
        };
        let json = serde_json::to_string(&gap).unwrap();
        assert!(json.contains("HDBSCAN Paper"));
        assert!(json.contains("hdbscan"));
    }

    #[test]
    fn verification_result_serializes() {
        let result = VerificationResult {
            research_query: "HDBSCAN clustering".into(),
            research_matches: vec![VerificationMatch {
                node_id: uuid::Uuid::new_v4(),
                name: "HDBSCAN".into(),
                node_type: "algorithm".into(),
                summary: Some("Hierarchical density-based clustering".into()),
                distance: 0.15,
                domain: "research".into(),
            }],
            code_matches: vec![VerificationMatch {
                node_id: uuid::Uuid::new_v4(),
                name: "run_hdbscan".into(),
                node_type: "function".into(),
                summary: Some("Runs HDBSCAN batch clustering".into()),
                distance: 0.25,
                domain: "code".into(),
            }],
            alignment_score: Some(0.82),
            component: Some("Entity Resolution".into()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("HDBSCAN clustering"));
        assert!(json.contains("0.82"));
    }

    #[test]
    fn critique_synthesis_roundtrips() {
        let synthesis = CritiqueSynthesis {
            counter_arguments: vec![CounterArgument {
                claim: "Redundant with statement extraction".into(),
                evidence: vec!["ADR-0015 statement 14".into()],
                strength: "strong".into(),
            }],
            supporting_arguments: vec![SupportingArgument {
                claim: "Improves chunk quality".into(),
                evidence: vec!["LlamaIndex eval".into()],
            }],
            recommendation: "Consider only if maintaining dual pipeline".into(),
        };
        let json = serde_json::to_string(&synthesis).unwrap();
        let parsed: CritiqueSynthesis = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.counter_arguments.len(), 1);
        assert_eq!(parsed.supporting_arguments.len(), 1);
        assert_eq!(parsed.recommendation, synthesis.recommendation);
    }

    #[test]
    fn critique_evidence_serializes() {
        let evidence = CritiqueEvidence {
            node_id: uuid::Uuid::new_v4(),
            name: "RRF".into(),
            node_type: "concept".into(),
            description: Some("Reciprocal Rank Fusion".into()),
            distance: 0.2,
            domain: "research".into(),
        };
        let json = serde_json::to_string(&evidence).unwrap();
        assert!(json.contains("RRF"));
        assert!(json.contains("research"));
    }
}
