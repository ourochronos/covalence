//! Type classification for ingested sources.

use serde::{Deserialize, Serialize};

/// Type classification for ingested sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    /// Published document (paper, report, book).
    Document,
    /// Web page or blog post.
    WebPage,
    /// Chat transcript or conversation log.
    Conversation,
    /// Source code or repository.
    Code,
    /// API response or structured data feed.
    Api,
    /// Manually entered knowledge.
    Manual,
    /// Tool output (CLI, scripts, automated systems).
    ToolOutput,
    /// Direct observation or measurement.
    Observation,
}

impl SourceType {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::WebPage => "web_page",
            Self::Conversation => "conversation",
            Self::Code => "code",
            Self::Api => "api",
            Self::Manual => "manual",
            Self::ToolOutput => "tool_output",
            Self::Observation => "observation",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "document" => Some(Self::Document),
            "web_page" => Some(Self::WebPage),
            "conversation" => Some(Self::Conversation),
            "code" => Some(Self::Code),
            "api" => Some(Self::Api),
            "manual" => Some(Self::Manual),
            "tool_output" => Some(Self::ToolOutput),
            "observation" => Some(Self::Observation),
            _ => None,
        }
    }

    /// Initial trust prior as `(alpha, beta)` from the spec.
    ///
    /// Based on the Beta-binomial trust model in `spec/07-epistemic-model.md`.
    pub fn initial_trust(&self) -> (f64, f64) {
        match self {
            Self::Document => (4.0, 1.0),     // 0.80 prior
            Self::ToolOutput => (3.5, 1.5),   // 0.70 prior
            Self::Code => (3.5, 1.5),         // 0.70 prior (same as tool)
            Self::Api => (3.5, 1.5),          // 0.70 prior
            Self::WebPage => (3.0, 2.0),      // 0.60 prior
            Self::Manual => (3.0, 2.0),       // 0.60 prior
            Self::Conversation => (2.5, 2.5), // 0.50 prior
            Self::Observation => (2.0, 3.0),  // 0.40 prior
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_type_roundtrip() {
        let types = [
            SourceType::Document,
            SourceType::WebPage,
            SourceType::Conversation,
            SourceType::Code,
            SourceType::Api,
            SourceType::Manual,
            SourceType::ToolOutput,
            SourceType::Observation,
        ];
        for st in &types {
            let s = st.as_str();
            let parsed = SourceType::from_str_opt(s);
            assert_eq!(parsed, Some(st.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn source_type_from_str_unknown() {
        assert!(SourceType::from_str_opt("unknown").is_none());
        assert!(SourceType::from_str_opt("").is_none());
        assert!(SourceType::from_str_opt("Document").is_none()); // case-sensitive
    }

    #[test]
    fn source_type_serde_roundtrip() {
        let st = SourceType::WebPage;
        let json = serde_json::to_string(&st).unwrap();
        assert_eq!(json, "\"web_page\"");
        let parsed: SourceType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceType::WebPage);
    }

    #[test]
    fn initial_trust_priors() {
        // Document should have highest trust
        let (a, b) = SourceType::Document.initial_trust();
        assert!((a / (a + b) - 0.80).abs() < 0.01);

        // Conversation should be lower
        let (a, b) = SourceType::Conversation.initial_trust();
        assert!((a / (a + b) - 0.50).abs() < 0.01);

        // Observation should be lowest
        let (a, b) = SourceType::Observation.initial_trust();
        assert!((a / (a + b) - 0.40).abs() < 0.01);
    }
}
