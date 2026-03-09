//! Fixture loading and types for evaluation test data.

use serde::{Deserialize, Serialize};

use crate::error::{EvalError, Result};

/// Top-level fixture file format.
///
/// Contains a test document, expected extraction annotations,
/// and search queries with relevance judgments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalFixture {
    /// The raw document text to be processed.
    pub document: String,
    /// Expected entities from extraction.
    pub expected_entities: Vec<ExpectedEntity>,
    /// Expected relationships from extraction.
    pub expected_relationships: Vec<ExpectedRelationship>,
    /// Search queries with ground-truth results.
    pub queries: Vec<QueryFixture>,
}

/// An expected entity for extraction evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedEntity {
    /// Canonical entity name.
    pub name: String,
    /// Entity type classification.
    pub entity_type: String,
}

/// An expected relationship for extraction evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedRelationship {
    /// Source entity name.
    pub source: String,
    /// Target entity name.
    pub target: String,
    /// Relationship type.
    pub rel_type: String,
}

/// A search query with expected results and relevance grades.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryFixture {
    /// The query text.
    pub query: String,
    /// Expected result IDs in ranked order.
    pub expected_ids: Vec<String>,
    /// Relevance grade for each result (0 = irrelevant, higher = more
    /// relevant). Indices correspond to `expected_ids`.
    pub relevance_grades: Vec<u32>,
}

/// Load an evaluation fixture from a JSON file path.
pub fn load_fixture(path: &str) -> Result<EvalFixture> {
    let content =
        std::fs::read_to_string(path).map_err(|e| EvalError::Fixture(format!("{path}: {e}")))?;
    let fixture: EvalFixture = serde_json::from_str(&content)?;
    Ok(fixture)
}

/// Load an evaluation fixture from a JSON string.
pub fn load_fixture_from_str(json: &str) -> Result<EvalFixture> {
    let fixture: EvalFixture = serde_json::from_str(json)?;
    Ok(fixture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fixture_json() -> &'static str {
        include_str!("../fixtures/sample.json")
    }

    #[test]
    fn load_sample_fixture() {
        let fixture = load_fixture_from_str(sample_fixture_json()).expect("fixture should parse");
        assert!(!fixture.document.is_empty(), "document should not be empty");
        assert!(
            !fixture.expected_entities.is_empty(),
            "should have expected entities"
        );
        assert!(!fixture.queries.is_empty(), "should have query fixtures");
    }

    #[test]
    fn fixture_entities_have_types() {
        let fixture = load_fixture_from_str(sample_fixture_json()).expect("fixture should parse");
        for entity in &fixture.expected_entities {
            assert!(!entity.name.is_empty(), "entity name should not be empty");
            assert!(
                !entity.entity_type.is_empty(),
                "entity type should not be empty"
            );
        }
    }

    #[test]
    fn fixture_queries_have_grades() {
        let fixture = load_fixture_from_str(sample_fixture_json()).expect("fixture should parse");
        for query in &fixture.queries {
            assert_eq!(
                query.expected_ids.len(),
                query.relevance_grades.len(),
                "expected_ids and relevance_grades must match"
            );
        }
    }

    #[test]
    fn load_nonexistent_file_returns_error() {
        let result = load_fixture("/nonexistent/path.json");
        assert!(result.is_err());
    }
}
