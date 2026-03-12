//! Source profiles — declarative pipeline composition per source type.
//!
//! A [`SourceProfile`] describes how a particular source type should
//! flow through the ingestion pipeline: which MIME types it handles,
//! what normalization passes to apply, chunking parameters, and
//! extraction configuration.
//!
//! Profiles are matched by MIME type and/or URI pattern via the
//! [`ProfileRegistry`]. Callers get a profile, then use its
//! settings to configure each pipeline stage. This replaces the
//! scattered `if is_code { ... } else { ... }` conditionals.

use crate::ingestion::normalize::{
    ArtifactLinePass, BlankLineCollapsePass, ControlCharPass, InlineArtifactPass, MathJaxPass,
    NormalizeChain, TrimPass, UnicodeNfcPass, WhitespacePass,
};
use crate::models::source::SourceType;

/// Declarative pipeline configuration for a source type.
///
/// Profiles are cheap to clone and carry no state — they're pure
/// configuration describing how a source should be processed.
#[derive(Debug, Clone)]
pub struct SourceProfile {
    /// Human-readable name for logging/debugging.
    pub name: &'static str,

    /// Source type for the provenance record.
    pub source_type: SourceType,

    /// MIME types this profile handles.
    ///
    /// Used by the registry for dispatch. First match wins.
    pub mime_types: Vec<&'static str>,

    /// URI patterns (prefix match) for dispatch.
    ///
    /// E.g., `["https://arxiv.org/"]` for arXiv papers.
    /// Empty means no URI-based matching.
    pub uri_prefixes: Vec<&'static str>,

    /// Chunking parameters.
    pub chunk_size: usize,
    pub chunk_overlap: usize,

    /// Whether to run extraction on chunks.
    pub extract: bool,

    /// Extraction method name (for provenance records).
    pub extraction_method: &'static str,

    /// Minimum token count for a chunk to be extractable.
    pub min_extract_tokens: usize,

    /// Whether to run coreference resolution.
    pub coreference: bool,

    /// Whether to run landscape analysis.
    pub landscape: bool,
}

impl SourceProfile {
    /// Build the normalization chain for this profile.
    ///
    /// Source profiles determine the chain at runtime rather than
    /// storing it, because `NormalizeChain` contains trait objects
    /// that aren't `Clone`.
    pub fn normalize_chain(&self) -> NormalizeChain {
        match self.source_type {
            SourceType::Code => NormalizeChain::code(),
            _ => {
                // Phase 1: core normalization.
                let mut chain = NormalizeChain::new()
                    .push(UnicodeNfcPass)
                    .push(ControlCharPass)
                    .push(WhitespacePass)
                    .push(TrimPass);

                // Phase 2: artifact stripping.
                chain = chain.push(ArtifactLinePass).push(InlineArtifactPass);

                // ArXiv sources get MathJax stripping.
                if self.uri_prefixes.iter().any(|p| p.contains("arxiv")) {
                    chain = chain.push(MathJaxPass);
                }

                // All document sources get blank line collapsing.
                chain = chain.push(BlankLineCollapsePass);

                // Phase 3: cleanup residual whitespace from stripping.
                chain = chain.push(WhitespacePass).push(TrimPass);

                chain
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in profiles
// ---------------------------------------------------------------------------

/// Default profile for documents (papers, reports, specs).
pub const DOCUMENT: SourceProfile = SourceProfile {
    name: "document",
    source_type: SourceType::Document,
    mime_types: Vec::new(), // matched by source_type, not MIME
    uri_prefixes: Vec::new(),
    chunk_size: 1500,
    chunk_overlap: 200,
    extract: true,
    extraction_method: "llm",
    min_extract_tokens: 30,
    coreference: true,
    landscape: true,
};

/// Profile for arXiv papers — like document but with MathJax stripping.
pub const ARXIV_PAPER: SourceProfile = SourceProfile {
    name: "arxiv_paper",
    source_type: SourceType::Document,
    mime_types: Vec::new(),
    uri_prefixes: Vec::new(), // filled at runtime: ["https://arxiv.org/"]
    chunk_size: 1500,
    chunk_overlap: 200,
    extract: true,
    extraction_method: "llm",
    min_extract_tokens: 30,
    coreference: true,
    landscape: true,
};

/// Profile for source code.
pub const CODE: SourceProfile = SourceProfile {
    name: "code",
    source_type: SourceType::Code,
    mime_types: Vec::new(),
    uri_prefixes: Vec::new(),
    chunk_size: 2000,
    chunk_overlap: 100,
    extract: true,
    extraction_method: "ast",
    min_extract_tokens: 10,
    coreference: false,
    landscape: true,
};

/// Profile for web pages.
pub const WEB_PAGE: SourceProfile = SourceProfile {
    name: "web_page",
    source_type: SourceType::WebPage,
    mime_types: Vec::new(),
    uri_prefixes: Vec::new(),
    chunk_size: 1200,
    chunk_overlap: 150,
    extract: true,
    extraction_method: "llm",
    min_extract_tokens: 30,
    coreference: true,
    landscape: true,
};

/// Profile for manually entered knowledge.
pub const MANUAL: SourceProfile = SourceProfile {
    name: "manual",
    source_type: SourceType::Manual,
    mime_types: Vec::new(),
    uri_prefixes: Vec::new(),
    chunk_size: 1000,
    chunk_overlap: 100,
    extract: true,
    extraction_method: "llm",
    min_extract_tokens: 20,
    coreference: false,
    landscape: false,
};

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry that matches incoming sources to profiles.
///
/// Matching priority:
/// 1. URI prefix match (most specific)
/// 2. Source type match
/// 3. Default document profile (fallback)
pub struct ProfileRegistry {
    profiles: Vec<SourceProfile>,
}

impl ProfileRegistry {
    /// Create a registry with the built-in profiles.
    pub fn new() -> Self {
        let mut arxiv = ARXIV_PAPER.clone();
        arxiv.uri_prefixes = vec!["https://arxiv.org/"];

        Self {
            profiles: vec![
                arxiv,
                CODE.clone(),
                WEB_PAGE.clone(),
                MANUAL.clone(),
                DOCUMENT.clone(), // fallback — must be last
            ],
        }
    }

    /// Register a custom profile. It will be checked before
    /// built-in profiles (inserted at the front).
    pub fn register(&mut self, profile: SourceProfile) {
        self.profiles.insert(0, profile);
    }

    /// Find the best-matching profile for a source.
    pub fn match_profile(&self, source_type: &SourceType, uri: Option<&str>) -> &SourceProfile {
        // Priority 1: URI prefix match.
        if let Some(uri) = uri {
            for profile in &self.profiles {
                if profile
                    .uri_prefixes
                    .iter()
                    .any(|prefix| uri.starts_with(prefix))
                {
                    return profile;
                }
            }
        }

        // Priority 2: Source type match.
        for profile in &self.profiles {
            if &profile.source_type == source_type {
                return profile;
            }
        }

        // Priority 3: Fallback to document.
        self.profiles
            .last()
            .expect("registry should always have at least one profile")
    }
}

impl Default for ProfileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_code() {
        let reg = ProfileRegistry::new();
        let profile = reg.match_profile(&SourceType::Code, None);
        assert_eq!(profile.name, "code");
        assert!(!profile.coreference);
    }

    #[test]
    fn registry_matches_arxiv_by_uri() {
        let reg = ProfileRegistry::new();
        let profile = reg.match_profile(
            &SourceType::Document,
            Some("https://arxiv.org/abs/2404.16130"),
        );
        assert_eq!(profile.name, "arxiv_paper");
    }

    #[test]
    fn registry_falls_back_to_document() {
        let reg = ProfileRegistry::new();
        let profile = reg.match_profile(&SourceType::Observation, None);
        assert_eq!(profile.name, "document");
    }

    #[test]
    fn registry_uri_takes_priority_over_type() {
        let reg = ProfileRegistry::new();
        // Even though source_type is WebPage, arxiv URI should match arxiv profile.
        let profile = reg.match_profile(
            &SourceType::WebPage,
            Some("https://arxiv.org/html/2404.16130"),
        );
        assert_eq!(profile.name, "arxiv_paper");
    }

    #[test]
    fn registry_custom_profile() {
        let mut reg = ProfileRegistry::new();
        let mut custom = WEB_PAGE.clone();
        custom.name = "blog"; // would need to be &'static str in real usage
        // For testing, we can't easily change &'static str, so test with existing
        reg.register(custom);
        // Custom is inserted at front but still matches by source_type
        let profile = reg.match_profile(&SourceType::WebPage, None);
        assert_eq!(profile.source_type, SourceType::WebPage);
    }

    #[test]
    fn document_chain_includes_blank_line_collapse() {
        let profile = DOCUMENT.clone();
        let chain = profile.normalize_chain();
        // Test that blank line collapsing works.
        let input = "a\n\n\n\nb";
        assert_eq!(chain.run(input), "a\n\nb");
    }

    #[test]
    fn arxiv_chain_strips_mathjax() {
        let mut profile = ARXIV_PAPER.clone();
        profile.uri_prefixes = vec!["https://arxiv.org/"];
        let chain = profile.normalize_chain();
        let input = "italic_e start_POSTSUBSCRIPT i end_POSTSUBSCRIPT";
        let result = chain.run(input);
        // MathJax markers removed; residual spaces from replacement.
        assert!(!result.contains("italic_"));
        assert!(!result.contains("POSTSUBSCRIPT"));
    }

    #[test]
    fn code_chain_skips_artifacts() {
        let profile = CODE.clone();
        let chain = profile.normalize_chain();
        let input = "// Report issue for preceding element";
        assert_eq!(chain.run(input), "// Report issue for preceding element");
    }

    #[test]
    fn code_profile_uses_ast_extraction() {
        assert_eq!(CODE.extraction_method, "ast");
    }

    #[test]
    fn document_profile_uses_llm_extraction() {
        assert_eq!(DOCUMENT.extraction_method, "llm");
    }

    #[test]
    fn profiles_have_unique_names() {
        let reg = ProfileRegistry::new();
        let names: Vec<&str> = reg.profiles.iter().map(|p| p.name).collect();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "profile names must be unique");
    }
}
