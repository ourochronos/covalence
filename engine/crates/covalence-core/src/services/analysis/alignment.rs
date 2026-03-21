//! Cross-domain alignment analysis.
//!
//! Compares entities across spec, design, code, and research domains
//! to surface misalignments: code ahead of spec, spec ahead of code,
//! design contradicted by research, and stale design docs.
//!
//! General-purpose: uses `domain` and `primary_domain` fields, not
//! hardcoded paths. Works for any project that classifies sources into
//! spec/design/code/research domains.

use crate::error::Result;

use super::AnalysisService;

/// Request parameters for alignment analysis.
#[derive(Debug, Clone)]
pub struct AlignmentRequest {
    /// Which checks to run. Empty = all.
    pub checks: Vec<String>,
    /// Minimum embedding similarity for matching (default 0.4).
    pub min_similarity: f64,
    /// Max items per check (default 20).
    pub limit: i64,
}

impl Default for AlignmentRequest {
    fn default() -> Self {
        Self {
            checks: Vec::new(),
            min_similarity: 0.4,
            limit: 20,
        }
    }
}

/// A single misalignment finding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlignmentItem {
    /// Category of misalignment.
    pub check: String,
    /// Name of the entity or concept.
    pub name: String,
    /// Domain of the entity (code, spec, design, research).
    pub domain: String,
    /// Node type (function, concept, etc.).
    pub node_type: String,
    /// Similarity score to the closest entity in the compared domain.
    pub closest_match_score: Option<f64>,
    /// Name of the closest match, if any.
    pub closest_match_name: Option<String>,
    /// Domain of the closest match.
    pub closest_match_domain: Option<String>,
    /// Brief explanation.
    pub reason: String,
}

/// Full alignment report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlignmentReport {
    /// Code entities with no matching spec concept.
    pub code_ahead: Vec<AlignmentItem>,
    /// Spec concepts with no implementing code.
    pub spec_ahead: Vec<AlignmentItem>,
    /// Design decisions potentially contradicted by research.
    pub design_contradicted: Vec<AlignmentItem>,
    /// Design docs whose descriptions diverge from code reality.
    pub stale_design: Vec<AlignmentItem>,
}

impl AnalysisService {
    /// Run cross-domain alignment analysis.
    pub async fn alignment_report(&self, req: &AlignmentRequest) -> Result<AlignmentReport> {
        let run_all = req.checks.is_empty();
        let threshold = 1.0 - req.min_similarity;

        let code_ahead = if run_all || req.checks.iter().any(|c| c == "code_ahead") {
            self.check_code_ahead(threshold, req.limit).await?
        } else {
            Vec::new()
        };

        let spec_ahead = if run_all || req.checks.iter().any(|c| c == "spec_ahead") {
            self.check_spec_ahead(req.limit).await?
        } else {
            Vec::new()
        };

        let design_contradicted =
            if run_all || req.checks.iter().any(|c| c == "design_contradicted") {
                self.check_design_contradicted(threshold, req.limit).await?
            } else {
                Vec::new()
            };

        let stale_design = if run_all || req.checks.iter().any(|c| c == "stale_design") {
            self.check_stale_design(threshold, req.limit).await?
        } else {
            Vec::new()
        };

        Ok(AlignmentReport {
            code_ahead,
            spec_ahead,
            design_contradicted,
            stale_design,
        })
    }

    /// Find code entities with no matching spec concept.
    ///
    /// These are functions/structs that exist in code but aren't
    /// described in any specification document.
    async fn check_code_ahead(
        &self,
        distance_threshold: f64,
        limit: i64,
    ) -> Result<Vec<AlignmentItem>> {
        // Code entities with embeddings that have no close match
        // in the spec domain.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, String, String, Option<f64>, Option<String>)> =
            sqlx::query_as("SELECT * FROM sp_find_code_ahead($1, $2)")
                .bind(distance_threshold)
                .bind(limit)
                .fetch_all(self.repo.pool())
                .await?;

        Ok(rows
            .into_iter()
            .map(|(name, ntype, domain, dist, closest)| AlignmentItem {
                check: "code_ahead".to_string(),
                reason: match &closest {
                    Some(c) => format!(
                        "Code entity with no close spec match (nearest: {c}, distance: {:.3})",
                        dist.unwrap_or(1.0)
                    ),
                    None => "Code entity with no spec/design concept nearby".to_string(),
                },
                name,
                domain,
                node_type: ntype,
                closest_match_score: dist.map(|d| 1.0 - d),
                closest_match_name: closest,
                closest_match_domain: Some("spec/design".to_string()),
            })
            .collect())
    }

    /// Find spec concepts with no implementing code.
    ///
    /// Uses primary_domain to identify true spec concepts (not
    /// cross-cutting entities that happen to be mentioned in specs).
    async fn check_spec_ahead(&self, limit: i64) -> Result<Vec<AlignmentItem>> {
        let rows: Vec<(String, String, i32)> =
            sqlx::query_as("SELECT * FROM sp_check_spec_ahead($1)")
                .bind(limit)
                .fetch_all(self.repo.pool())
                .await?;

        Ok(rows
            .into_iter()
            .map(|(name, ntype, mentions)| AlignmentItem {
                check: "spec_ahead".to_string(),
                reason: format!(
                    "Spec concept mentioned {mentions} times with no {} edge",
                    self.bridges.implements_intent
                ),
                name,
                domain: "spec".to_string(),
                node_type: ntype,
                closest_match_score: None,
                closest_match_name: None,
                closest_match_domain: None,
            })
            .collect())
    }

    /// Find design decisions that may be contradicted by research.
    ///
    /// Compares design entity embeddings against research entities
    /// and flags pairs that are semantically close (same topic) but
    /// might describe conflicting approaches.
    async fn check_design_contradicted(
        &self,
        distance_threshold: f64,
        limit: i64,
    ) -> Result<Vec<AlignmentItem>> {
        // Find design entities that have close research matches —
        // these are candidates for contradiction (same topic,
        // potentially different conclusion).
        let rows: Vec<(String, String, f64, String)> =
            sqlx::query_as("SELECT * FROM sp_find_design_contradictions($1, $2)")
                .bind(distance_threshold)
                .bind(limit)
                .fetch_all(self.repo.pool())
                .await?;

        Ok(rows
            .into_iter()
            .map(|(name, ntype, dist, research_name)| AlignmentItem {
                check: "design_contradicted".to_string(),
                reason: format!(
                    "Design decision close to research concept '{research_name}' \
                     (similarity: {:.3}) — may need review",
                    1.0 - dist
                ),
                name,
                domain: "design".to_string(),
                node_type: ntype,
                closest_match_score: Some(1.0 - dist),
                closest_match_name: Some(research_name),
                closest_match_domain: Some("research".to_string()),
            })
            .collect())
    }

    /// Find design docs whose descriptions diverge from code reality.
    ///
    /// Flags design docs where linked code entities were updated more
    /// recently, suggesting the design may be stale relative to the code.
    async fn check_stale_design(
        &self,
        _distance_threshold: f64,
        limit: i64,
    ) -> Result<Vec<AlignmentItem>> {
        // Design sources linked to code entities via extractions.
        // Flag when the newest linked code entity was updated after
        // the design source (code evolved, design didn't).
        let rows: Vec<(String, String, f64, String)> =
            sqlx::query_as("SELECT * FROM sp_find_stale_design($1)")
                .bind(limit)
                .fetch_all(self.repo.pool())
                .await?;

        Ok(rows
            .into_iter()
            .map(|(name, ntype, days_behind, code_entities)| {
                let entities_preview = if code_entities.len() > 80 {
                    format!("{}...", &code_entities[..77])
                } else {
                    code_entities
                };
                AlignmentItem {
                    check: "stale_design".to_string(),
                    reason: format!(
                        "Design doc is {days_behind:.1} days behind linked code. \
                         Code entities: {entities_preview}"
                    ),
                    name,
                    domain: "design".to_string(),
                    node_type: ntype,
                    closest_match_score: None,
                    closest_match_name: None,
                    closest_match_domain: Some("code".to_string()),
                }
            })
            .collect())
    }
}
