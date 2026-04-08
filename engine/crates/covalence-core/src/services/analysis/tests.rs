//! Unit tests for the analysis module.

#[cfg(test)]
mod tests {
    use super::super::constants::{COMPONENT_DEFS, MODULE_PATH_MAPPINGS};
    use super::super::*;

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
            total_reachable: 1,
            truncated: false,
            node_limit_applied: 50,
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
