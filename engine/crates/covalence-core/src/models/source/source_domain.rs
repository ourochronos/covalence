//! Knowledge domain classification for sources.

use serde::{Deserialize, Serialize};

/// Knowledge domain classification for sources.
///
/// Classifies a source's role in the knowledge graph. Set at ingestion
/// time via `derive_domain()` based on URI patterns and source type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDomain {
    /// Source code files (.rs, .go, etc.).
    Code,
    /// Specification documents (spec/*.md).
    Spec,
    /// Architecture decisions, design docs, project meta.
    Design,
    /// Academic papers, external knowledge.
    Research,
    /// Third-party documentation, API references.
    External,
}

impl SourceDomain {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Spec => "spec",
            Self::Design => "design",
            Self::Research => "research",
            Self::External => "external",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "code" => Some(Self::Code),
            "spec" => Some(Self::Spec),
            "design" => Some(Self::Design),
            "research" => Some(Self::Research),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_domain_roundtrip() {
        let domains = [
            SourceDomain::Code,
            SourceDomain::Spec,
            SourceDomain::Design,
            SourceDomain::Research,
            SourceDomain::External,
        ];
        for d in &domains {
            let s = d.as_str();
            let parsed = SourceDomain::from_str_opt(s);
            assert_eq!(parsed, Some(d.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn source_domain_from_str_unknown() {
        assert!(SourceDomain::from_str_opt("unknown").is_none());
        assert!(SourceDomain::from_str_opt("").is_none());
    }

    #[test]
    fn source_domain_serde_roundtrip() {
        let d = SourceDomain::Research;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"research\"");
        let parsed: SourceDomain = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SourceDomain::Research);
    }
}
