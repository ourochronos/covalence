//! Community summary generation and storage.
//!
//! LLM-generated summaries for each community, stored as nodes
//! with `node_type = "community_summary"` and linked to community
//! members via `SUMMARIZES` edges.

use serde::{Deserialize, Serialize};

/// Request to generate a community summary.
#[derive(Debug, Clone)]
pub struct CommunitySummaryInput {
    /// Community ID.
    pub community_id: usize,
    /// Core level of the community.
    pub core_level: usize,
    /// Entity names in this community.
    pub entity_names: Vec<String>,
    /// Key relationship descriptions.
    pub relationships: Vec<String>,
    /// Representative chunk content from community members.
    pub representative_chunks: Vec<String>,
}

/// Generated community summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitySummary {
    /// Community ID.
    pub community_id: usize,
    /// Generated title.
    pub title: String,
    /// Generated summary text.
    pub summary: String,
    /// Key findings/themes identified.
    pub key_themes: Vec<String>,
}

/// Trait for generating community summaries.
#[async_trait::async_trait]
pub trait SummaryGenerator: Send + Sync {
    /// Generate a summary for a community.
    async fn generate(
        &self,
        input: &CommunitySummaryInput,
    ) -> crate::error::Result<CommunitySummary>;
}

/// Simple concatenation-based summary generator (no LLM).
///
/// Produces summaries by joining entity names and relationship
/// descriptions. Useful for testing and as a baseline.
pub struct ConcatSummaryGenerator;

#[async_trait::async_trait]
impl SummaryGenerator for ConcatSummaryGenerator {
    async fn generate(
        &self,
        input: &CommunitySummaryInput,
    ) -> crate::error::Result<CommunitySummary> {
        let title = format!(
            "Community {} (core level {})",
            input.community_id, input.core_level
        );

        let mut summary = String::new();
        if !input.entity_names.is_empty() {
            summary.push_str("Key entities: ");
            summary.push_str(&input.entity_names.join(", "));
            summary.push_str(". ");
        }
        if !input.relationships.is_empty() {
            summary.push_str("Relationships: ");
            summary.push_str(&input.relationships.join("; "));
            summary.push_str(". ");
        }

        let key_themes = input.entity_names.clone();

        Ok(CommunitySummary {
            community_id: input.community_id,
            title,
            summary,
            key_themes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concat_summary_basic() {
        let generator = ConcatSummaryGenerator;
        let input = CommunitySummaryInput {
            community_id: 42,
            core_level: 2,
            entity_names: vec!["Alice".to_string(), "Bob".to_string()],
            relationships: vec!["Alice works with Bob".to_string()],
            representative_chunks: vec![],
        };

        let result = generator.generate(&input).await.unwrap();
        assert_eq!(result.community_id, 42);
        assert_eq!(result.title, "Community 42 (core level 2)");
        assert!(result.summary.contains("Alice"));
        assert!(result.summary.contains("Bob"));
        assert!(result.summary.contains("Alice works with Bob"));
        assert_eq!(result.key_themes.len(), 2);
        assert_eq!(result.key_themes[0], "Alice");
        assert_eq!(result.key_themes[1], "Bob");
    }

    #[tokio::test]
    async fn concat_summary_empty_entities() {
        let generator = ConcatSummaryGenerator;
        let input = CommunitySummaryInput {
            community_id: 1,
            core_level: 0,
            entity_names: vec![],
            relationships: vec![],
            representative_chunks: vec![],
        };

        let result = generator.generate(&input).await.unwrap();
        assert_eq!(result.community_id, 1);
        assert!(result.summary.is_empty());
        assert!(result.key_themes.is_empty());
    }

    #[tokio::test]
    async fn concat_summary_entities_only() {
        let generator = ConcatSummaryGenerator;
        let input = CommunitySummaryInput {
            community_id: 5,
            core_level: 1,
            entity_names: vec!["Rust".to_string()],
            relationships: vec![],
            representative_chunks: vec![],
        };

        let result = generator.generate(&input).await.unwrap();
        assert!(result.summary.starts_with("Key entities:"));
        assert!(!result.summary.contains("Relationships:"));
    }
}
