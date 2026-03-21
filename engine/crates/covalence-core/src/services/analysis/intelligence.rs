//! Blast-radius simulation, implementation verification, and dialectical
//! critique.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::graph::engine::BfsOptions;
use crate::ingestion::ChatBackend;
use crate::ingestion::embedder::truncate_and_validate;

use super::AnalysisService;

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

impl AnalysisService {
    // ------------------------------------------------------------------
    // Capability 4: Blast-Radius Simulation
    // ------------------------------------------------------------------

    /// Maximum affected nodes to collect before stopping BFS early.
    const BLAST_RADIUS_NODE_CAP: usize = 500;

    /// Resolve a node by name with fuzzy fallback.
    ///
    /// Resolution order:
    /// 1. Exact case-insensitive match on `canonical_name` via
    ///    `sp_resolve_node_by_name`.
    /// 2. Substring match via `sp_resolve_node_fuzzy`, picking the
    ///    row with the highest `mention_count`.
    async fn resolve_node_by_name(&self, name: &str) -> Result<(uuid::Uuid, String, String)> {
        // Step 1: exact case-insensitive match via stored procedure.
        let exact: Option<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT * FROM sp_resolve_node_by_name($1)")
                .bind(name)
                .fetch_optional(self.repo.pool())
                .await?;

        if let Some(row) = exact {
            return Ok(row);
        }

        // Step 2: substring match via stored procedure.
        let fuzzy: Option<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT * FROM sp_resolve_node_fuzzy($1, $2)")
                .bind(name)
                .bind(1_i32)
                .fetch_optional(self.repo.pool())
                .await?;

        fuzzy.ok_or_else(|| Error::NotFound {
            entity_type: "node",
            id: name.to_string(),
        })
    }

    /// Estimate the blast radius of changing a given node.
    ///
    /// Traverses the graph sidecar outward from the target node up to
    /// `max_hops` hops, collecting affected nodes grouped by hop distance.
    /// Stops early if `BLAST_RADIUS_NODE_CAP` nodes are collected.
    ///
    /// When `include_invalidated` is true, nodes reachable through
    /// invalidated edges are also returned (at hop distance 1) so
    /// that the blast radius reflects historically-connected nodes
    /// that may still be affected by changes.
    pub async fn blast_radius(
        &self,
        target_name: &str,
        max_hops: usize,
        include_invalidated: bool,
    ) -> Result<BlastRadiusResult> {
        // Find the target node by name (exact, then fuzzy fallback).
        let (target_id, target_canonical, target_type) =
            self.resolve_node_by_name(target_name).await?;

        // Find the component this node belongs to via stored procedure.
        let component: Option<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT * FROM sp_get_node_component($1)")
                .bind(target_id)
                .fetch_optional(self.repo.pool())
                .await?;

        // BFS through the graph engine trait.
        let bfs_opts = BfsOptions {
            max_hops,
            skip_synthetic: false,
            deny_rel_types: Vec::new(),
        };
        let bfs_nodes = self.graph.bfs_neighborhood(target_id, bfs_opts).await?;

        // Collect IDs already present from BFS to avoid duplicates.
        let mut seen_ids: std::collections::HashSet<uuid::Uuid> =
            bfs_nodes.iter().map(|n| n.id).collect();
        seen_ids.insert(target_id);

        // Group BFS results by hop distance for the blast radius response,
        // capping total collected at BLAST_RADIUS_NODE_CAP.
        let mut affected: Vec<BlastRadiusHop> = Vec::new();
        let mut total_collected = 0usize;
        for hop in 1..=max_hops {
            if total_collected >= Self::BLAST_RADIUS_NODE_CAP {
                break;
            }
            let hop_nodes: Vec<AffectedNode> = bfs_nodes
                .iter()
                .filter(|n| n.hops == hop)
                .take(Self::BLAST_RADIUS_NODE_CAP - total_collected)
                .map(|n| AffectedNode {
                    node_id: n.id,
                    name: n.name.clone(),
                    node_type: n.node_type.clone(),
                    relationship: String::new(),
                })
                .collect();
            total_collected += hop_nodes.len();
            if !hop_nodes.is_empty() {
                affected.push(BlastRadiusHop {
                    hop_distance: hop,
                    nodes: hop_nodes,
                });
            }
        }

        // When include_invalidated is set, query PG for nodes
        // reachable through invalidated edges at hop distance 1.
        if include_invalidated && total_collected < Self::BLAST_RADIUS_NODE_CAP {
            let remaining = Self::BLAST_RADIUS_NODE_CAP - total_collected;
            let invalidated_neighbors: Vec<(uuid::Uuid, String, String, String)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, n.node_type, \
                            e.rel_type \
                     FROM edges e \
                     JOIN nodes n ON n.id = CASE \
                         WHEN e.source_node_id = $1 \
                             THEN e.target_node_id \
                         ELSE e.source_node_id END \
                     WHERE e.invalid_at IS NOT NULL \
                       AND (e.source_node_id = $1 OR e.target_node_id = $1) \
                     LIMIT $2",
            )
            .bind(target_id)
            .bind(remaining as i64)
            .fetch_all(self.repo.pool())
            .await?;

            let mut inv_nodes: Vec<AffectedNode> = Vec::new();
            for (id, name, node_type, relationship) in invalidated_neighbors {
                if seen_ids.contains(&id) {
                    continue;
                }
                seen_ids.insert(id);
                inv_nodes.push(AffectedNode {
                    node_id: id,
                    name,
                    node_type,
                    relationship: format!("{} (invalidated)", relationship),
                });
            }

            if !inv_nodes.is_empty() {
                // Merge into hop-1 if it exists, otherwise create it.
                if let Some(hop1) = affected.iter_mut().find(|h| h.hop_distance == 1) {
                    hop1.nodes.extend(inv_nodes);
                } else {
                    affected.insert(
                        0,
                        BlastRadiusHop {
                            hop_distance: 1,
                            nodes: inv_nodes,
                        },
                    );
                }
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
                       AND s.domain = ANY($2) \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT 10",
            )
            .bind(&query_truncated)
            .bind(&self.domains.research_domains)
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
             WHERE comp.entity_class = 'analysis' \
               AND e.rel_type = $2 \
               AND e.target_node_id = ANY($1)",
        )
        .bind(&research_ids)
        .bind(&self.bridges.theoretical_basis)
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

        // Find code nodes linked to these components via
        // PART_OF_COMPONENT.
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
                 WHERE e.rel_type = $4 \
                   AND e.target_node_id = ANY($2) \
                   AND n.embedding IS NOT NULL \
                 ORDER BY dist ASC \
                 LIMIT $3",
                )
                .bind(&query_truncated)
                .bind(&comp_ids)
                .bind(Self::MAX_VERIFY_CODE_NODES)
                .bind(&self.bridges.part_of_component)
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
                       AND s.domain = ANY($3) \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(&proposal_truncated)
            .bind(Self::MAX_CRITIQUE_EVIDENCE)
            .bind(&self.domains.research_domains)
            .fetch_all(self.repo.pool())
            .await?;

        // Spec/design evidence.
        let spec_evidence: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                        n.description, \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.entity_class = 'domain' \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND s.domain = ANY($3) \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
        )
        .bind(&proposal_truncated)
        .bind(Self::MAX_CRITIQUE_EVIDENCE)
        .bind(&self.domains.spec_domains)
        .fetch_all(self.repo.pool())
        .await?;

        // Code evidence.
        let code_evidence: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                        COALESCE(n.properties->>'semantic_summary', \
                                 n.description), \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.entity_class = $3 \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND s.domain = $4 \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
        )
        .bind(&proposal_truncated)
        .bind(Self::MAX_CRITIQUE_EVIDENCE)
        .bind(&self.domains.code_entity_class)
        .bind(&self.domains.code_domain)
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

        let system = "You are a critical design reviewer for a knowledge \
                       engine called Covalence. Given a design proposal and \
                       evidence from the system's research papers, spec \
                       documents, and codebase, generate a structured \
                       dialectical critique. Be specific, cite evidence by \
                       name, and identify both counter-arguments and \
                       supporting arguments.";

        let user = format!(
            "## Design Proposal\n{proposal}\n\n\
             ## Evidence from the Knowledge Graph\n\
             {}\n{}\n{}\n\n\
             Respond with a JSON object:\n\
             {{\n\
               \"counter_arguments\": [\n\
                 {{\"claim\": \"...\", \"evidence\": [\"...\"], \
             \"strength\": \"strong|moderate|weak\"}}\n\
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

        let chat_resp = backend.chat(system, &user, true, 0.3).await?;
        let response = chat_resp.text;

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
