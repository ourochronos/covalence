//! Stage 3: Composable normalization pipeline.
//!
//! All formats convert to Markdown as the canonical intermediate
//! representation. Normalization is decomposed into small, composable
//! passes via the [`NormalizePass`] trait. Each pass does one thing
//! (Unicode NFC, whitespace collapsing, artifact stripping, etc.)
//! and a [`NormalizeChain`] composes them into a pipeline.
//!
//! Source profiles can select which passes to apply and in what
//! order, enabling per-source-type normalization without writing
//! new code.

use unicode_normalization::UnicodeNormalization;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A single normalization pass over text.
///
/// Passes are small (10-30 lines), composable, and stateless.
/// Each pass receives the output of the previous pass and returns
/// its transformation. The name is used for tracing and debugging.
pub trait NormalizePass: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Apply this normalization pass to the input text.
    fn apply(&self, text: &str) -> String;
}

// ---------------------------------------------------------------------------
// Chain
// ---------------------------------------------------------------------------

/// A composable chain of normalization passes.
///
/// Passes execute in order; each receives the output of the
/// previous pass. An empty chain returns the input unchanged.
pub struct NormalizeChain {
    passes: Vec<Box<dyn NormalizePass>>,
}

impl NormalizeChain {
    /// Create an empty chain.
    pub fn new() -> Self {
        Self { passes: Vec::new() }
    }

    /// Append a pass to the chain.
    pub fn push(mut self, pass: impl NormalizePass + 'static) -> Self {
        self.passes.push(Box::new(pass));
        self
    }

    /// Run all passes in sequence.
    pub fn run(&self, text: &str) -> String {
        let mut result = text.to_string();
        for pass in &self.passes {
            result = pass.apply(&result);
        }
        result
    }

    /// The default chain for document sources.
    ///
    /// Two-phase approach:
    /// 1. Core normalization (NFC, control chars, whitespace, trim)
    /// 2. Artifact stripping (lines, inline, MathJax)
    /// 3. Cleanup (collapse residual multi-spaces from stripping, trim)
    pub fn default_document() -> Self {
        Self::new()
            // Phase 1: core normalization.
            .push(UnicodeNfcPass)
            .push(ControlCharPass)
            .push(WhitespacePass)
            .push(TrimPass)
            // Phase 2: artifact stripping.
            .push(ArtifactLinePass)
            .push(InlineArtifactPass)
            .push(MathJaxPass)
            // Phase 3: cleanup after stripping.
            .push(WhitespacePass)
            .push(TrimPass)
    }

    /// Minimal chain for code sources (no artifact/mathjax stripping).
    pub fn code() -> Self {
        Self::new()
            .push(UnicodeNfcPass)
            .push(ControlCharPass)
            .push(TrimPass)
    }
}

impl Default for NormalizeChain {
    fn default() -> Self {
        Self::default_document()
    }
}

// ---------------------------------------------------------------------------
// Built-in passes
// ---------------------------------------------------------------------------

/// Unicode NFC normalization.
///
/// Converts decomposed sequences (e.g., `e` + combining accent) to
/// their precomposed form (`é`).
pub struct UnicodeNfcPass;

impl NormalizePass for UnicodeNfcPass {
    fn name(&self) -> &str {
        "unicode_nfc"
    }

    fn apply(&self, text: &str) -> String {
        text.nfc().collect()
    }
}

/// Strip control characters, preserving `\n` and `\t`.
///
/// Removes null bytes, escape sequences, and other non-printable
/// characters that pollute downstream processing.
pub struct ControlCharPass;

impl NormalizePass for ControlCharPass {
    fn name(&self) -> &str {
        "control_char_strip"
    }

    fn apply(&self, text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect()
    }
}

/// Collapse runs of whitespace to a single space, preserving `\n`.
///
/// Tabs and multiple spaces become a single space. Newlines are
/// kept as-is (they're structural in Markdown).
pub struct WhitespacePass;

impl NormalizePass for WhitespacePass {
    fn name(&self) -> &str {
        "whitespace_collapse"
    }

    fn apply(&self, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut prev_space = false;

        for ch in text.chars() {
            if ch == '\n' {
                prev_space = false;
                result.push(ch);
            } else if ch.is_whitespace() {
                if !prev_space {
                    result.push(' ');
                    prev_space = true;
                }
            } else {
                prev_space = false;
                result.push(ch);
            }
        }

        result
    }
}

/// Trim leading and trailing whitespace.
pub struct TrimPass;

impl NormalizePass for TrimPass {
    fn name(&self) -> &str {
        "trim"
    }

    fn apply(&self, text: &str) -> String {
        text.trim().to_string()
    }
}

/// Remove whole lines that start with known web-scraping artifact
/// prefixes (case-insensitive).
pub struct ArtifactLinePass;

/// Lines (or line prefixes) that are ArXiv/web-scraping artifacts.
/// If a line starts with any of these (case-insensitive), it is
/// removed entirely.
const ARTIFACT_LINE_PREFIXES: &[&str] = &[
    "report issue for preceding element",
    "html conversions sometimes display errors",
    "authors: achieve the best html results",
];

impl NormalizePass for ArtifactLinePass {
    fn name(&self) -> &str {
        "artifact_lines"
    }

    fn apply(&self, text: &str) -> String {
        text.lines()
            .filter(|line| {
                let lower = line.trim().to_lowercase();
                !ARTIFACT_LINE_PREFIXES
                    .iter()
                    .any(|prefix| lower.starts_with(prefix))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Strip inline artifact substrings that may be concatenated with
/// real content (not on their own line).
pub struct InlineArtifactPass;

/// Inline artifact substrings that should be removed even when
/// concatenated with real content (not on their own line).
const ARTIFACT_INLINE_PATTERNS: &[&str] = &[
    "Report issue for preceding element",
    "report issue for preceding element",
];

impl NormalizePass for InlineArtifactPass {
    fn name(&self) -> &str {
        "inline_artifacts"
    }

    fn apply(&self, text: &str) -> String {
        let mut result = text.to_string();
        for pattern in ARTIFACT_INLINE_PATTERNS {
            result = result.replace(pattern, "");
        }
        result
    }
}

/// Strip MathJax accessibility text markers from arXiv HTML
/// conversions.
///
/// These are verbose expansions of mathematical notation
/// (e.g., `italic_e start_POSTSUBSCRIPT italic_i end_POSTSUBSCRIPT`).
pub struct MathJaxPass;

/// MathJax accessibility text markers produced when stripping HTML from
/// arXiv papers. Includes both raw and Markdown-escaped (`\_`) variants
/// since the HTML→Markdown converter may escape underscores.
const MATHJAX_MARKERS: &[&str] = &[
    // Raw markers (from direct HTML text extraction).
    "start_POSTSUBSCRIPT",
    "end_POSTSUBSCRIPT",
    "start_POSTSUPERSCRIPT",
    "end_POSTSUPERSCRIPT",
    "italic_",
    "caligraphic_",
    "bold_italic_",
    "roman_",
    "bold_",
    "start_CELL",
    "end_CELL",
    "start_ROW",
    "end_ROW",
    // Markdown-escaped variants (\_).
    r"start\_POSTSUBSCRIPT",
    r"end\_POSTSUBSCRIPT",
    r"start\_POSTSUPERSCRIPT",
    r"end\_POSTSUPERSCRIPT",
    r"italic\_",
    r"caligraphic\_",
    r"bold\_italic\_",
    r"roman\_",
    r"bold\_",
    r"start\_CELL",
    r"end\_CELL",
    r"start\_ROW",
    r"end\_ROW",
];

impl NormalizePass for MathJaxPass {
    fn name(&self) -> &str {
        "mathjax"
    }

    fn apply(&self, text: &str) -> String {
        let mut result = text.to_string();
        for marker in MATHJAX_MARKERS {
            result = result.replace(marker, "");
        }
        result
    }
}

/// Collapse runs of 3+ blank lines to 2 (preserving paragraph
/// boundaries without excessive gaps).
pub struct BlankLineCollapsePass;

impl NormalizePass for BlankLineCollapsePass {
    fn name(&self) -> &str {
        "blank_line_collapse"
    }

    fn apply(&self, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut consecutive_blank = 0u32;

        for line in text.split('\n') {
            if line.trim().is_empty() {
                consecutive_blank += 1;
                // Keep at most 1 blank line (which creates a \n\n
                // paragraph break together with the preceding \n).
                if consecutive_blank <= 1 {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(line);
                }
            } else {
                consecutive_blank = 0;
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(line);
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Convenience wrappers (backward compat)
// ---------------------------------------------------------------------------

/// Normalize text for consistent processing.
///
/// Steps:
/// 1. Unicode NFC normalization
/// 2. Strip control characters (keep `\n`)
/// 3. Collapse multiple whitespace to single space (preserving `\n`)
/// 4. Trim leading/trailing whitespace
///
/// This is the convenience wrapper matching the original monolithic
/// function. For composable use, see [`NormalizeChain`].
pub fn normalize(text: &str) -> String {
    // Apply only the core normalization passes (no artifact stripping).
    let chain = NormalizeChain::new()
        .push(UnicodeNfcPass)
        .push(ControlCharPass)
        .push(WhitespacePass)
        .push(TrimPass);
    chain.run(text)
}

/// Remove known web-scraping artifact lines and inline patterns
/// from normalized markdown. Applied after Unicode normalization.
///
/// Also collapses residual multi-spaces left by marker removal
/// and trims the result.
///
/// This is the convenience wrapper matching the original monolithic
/// function. For composable use, see [`NormalizeChain`].
pub fn strip_artifacts(text: &str) -> String {
    let chain = NormalizeChain::new()
        .push(ArtifactLinePass)
        .push(InlineArtifactPass)
        .push(MathJaxPass)
        .push(WhitespacePass)
        .push(TrimPass);
    chain.run(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- normalize() backward compat ---

    #[test]
    fn collapses_whitespace() {
        assert_eq!(normalize("hello   world"), "hello world");
    }

    #[test]
    fn preserves_newlines() {
        assert_eq!(normalize("hello\nworld"), "hello\nworld");
    }

    #[test]
    fn strips_control_chars() {
        assert_eq!(normalize("hello\x00world"), "helloworld");
    }

    #[test]
    fn trims_edges() {
        assert_eq!(normalize("  hello  "), "hello");
    }

    #[test]
    fn nfc_normalization() {
        // e + combining acute accent -> single é character
        let decomposed = "e\u{0301}";
        let result = normalize(decomposed);
        assert_eq!(result, "\u{00e9}");
    }

    #[test]
    fn tabs_collapse_to_space() {
        assert_eq!(normalize("hello\t\tworld"), "hello world");
    }

    #[test]
    fn empty_string() {
        assert_eq!(normalize(""), "");
    }

    // --- strip_artifacts() backward compat ---

    #[test]
    fn strip_report_issue_lines() {
        let input = "Some content\nReport issue for preceding element\nMore content";
        assert_eq!(strip_artifacts(input), "Some content\nMore content");
    }

    #[test]
    fn strip_html_conversion_warning() {
        let input = "# Title\nHTML conversions sometimes display errors due to...\nReal text";
        assert_eq!(strip_artifacts(input), "# Title\nReal text");
    }

    #[test]
    fn strip_authors_best_practices() {
        let input = "Authors: achieve the best HTML results from your LaTeX\nActual content";
        assert_eq!(strip_artifacts(input), "Actual content");
    }

    #[test]
    fn strip_preserves_real_content() {
        let input = "Knowledge graphs are important.\nThey enable reasoning.";
        assert_eq!(strip_artifacts(input), input);
    }

    #[test]
    fn strip_case_insensitive() {
        let input = "REPORT ISSUE FOR PRECEDING ELEMENT\nContent";
        assert_eq!(strip_artifacts(input), "Content");
    }

    #[test]
    fn strip_inline_artifact_concatenated() {
        let input =
            "higher capability in identifying refactorings.Report issue for preceding element";
        assert_eq!(
            strip_artifacts(input),
            "higher capability in identifying refactorings."
        );
    }

    #[test]
    fn strip_inline_artifact_mid_text() {
        let input = "Some text.Report issue for preceding element\nMore content here.";
        assert_eq!(strip_artifacts(input), "Some text.\nMore content here.");
    }

    #[test]
    fn strip_mathjax_postsubscript() {
        let input = "e start_POSTSUBSCRIPT i end_POSTSUBSCRIPT in E";
        assert_eq!(strip_artifacts(input), "e i in E");
    }

    #[test]
    fn strip_mathjax_postsuperscript() {
        let input = "x start_POSTSUPERSCRIPT 2 end_POSTSUPERSCRIPT";
        assert_eq!(strip_artifacts(input), "x 2");
    }

    #[test]
    fn strip_mathjax_italic_prefix() {
        let input = "italic_e italic_i caligraphic_E";
        assert_eq!(strip_artifacts(input), "e i E");
    }

    #[test]
    fn strip_mathjax_mixed() {
        let input = "The entity italic_e start_POSTSUBSCRIPT italic_i end_POSTSUBSCRIPT belongs to caligraphic_E";
        assert_eq!(strip_artifacts(input), "The entity e i belongs to E");
    }

    // --- NormalizeChain tests ---

    #[test]
    fn chain_empty_is_identity() {
        let chain = NormalizeChain::new();
        assert_eq!(chain.run("hello world"), "hello world");
    }

    #[test]
    fn chain_single_pass() {
        let chain = NormalizeChain::new().push(TrimPass);
        assert_eq!(chain.run("  hello  "), "hello");
    }

    #[test]
    fn chain_compose_order_matters() {
        // Trim then whitespace-collapse is different from reverse.
        let input = "  hello   world  ";
        let chain = NormalizeChain::new().push(TrimPass).push(WhitespacePass);
        assert_eq!(chain.run(input), "hello world");
    }

    #[test]
    fn default_document_chain() {
        let chain = NormalizeChain::default_document();
        let input = "  hello\x00   world  ";
        let result = chain.run(input);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn default_document_strips_artifacts() {
        let chain = NormalizeChain::default_document();
        let input = "Content.\nReport issue for preceding element\nMore.";
        assert_eq!(chain.run(input), "Content.\nMore.");
    }

    #[test]
    fn default_document_strips_mathjax() {
        let chain = NormalizeChain::default_document();
        let input = "The entity italic_e belongs to caligraphic_E";
        assert_eq!(chain.run(input), "The entity e belongs to E");
    }

    #[test]
    fn code_chain_preserves_whitespace_runs() {
        let chain = NormalizeChain::code();
        // Code chain doesn't collapse whitespace (indentation matters).
        let input = "    fn main() {}";
        let result = chain.run(input);
        assert_eq!(result, "fn main() {}"); // trimmed but spaces preserved internally
    }

    #[test]
    fn code_chain_skips_artifact_stripping() {
        let chain = NormalizeChain::code();
        // Code chain doesn't strip artifacts (they could be legit content).
        let input = "// Report issue for preceding element";
        let result = chain.run(input);
        assert_eq!(result, "// Report issue for preceding element");
    }

    // --- Individual pass tests ---

    #[test]
    fn blank_line_collapse_preserves_double() {
        let pass = BlankLineCollapsePass;
        let input = "a\n\nb";
        assert_eq!(pass.apply(input), "a\n\nb");
    }

    #[test]
    fn blank_line_collapse_trims_triple() {
        let pass = BlankLineCollapsePass;
        let input = "a\n\n\nb";
        assert_eq!(pass.apply(input), "a\n\nb");
    }

    #[test]
    fn blank_line_collapse_trims_many() {
        let pass = BlankLineCollapsePass;
        let input = "a\n\n\n\n\nb";
        assert_eq!(pass.apply(input), "a\n\nb");
    }

    #[test]
    fn pass_names_are_unique() {
        let passes: Vec<Box<dyn NormalizePass>> = vec![
            Box::new(UnicodeNfcPass),
            Box::new(ControlCharPass),
            Box::new(WhitespacePass),
            Box::new(TrimPass),
            Box::new(ArtifactLinePass),
            Box::new(InlineArtifactPass),
            Box::new(MathJaxPass),
            Box::new(BlankLineCollapsePass),
        ];
        let names: Vec<&str> = passes.iter().map(|p| p.name()).collect();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "pass names must be unique");
    }

    #[test]
    fn mathjax_no_duplicate_markers() {
        // Verify the list has no duplicates.
        let unique: std::collections::HashSet<&str> = MATHJAX_MARKERS.iter().copied().collect();
        assert_eq!(
            MATHJAX_MARKERS.len(),
            unique.len(),
            "MATHJAX_MARKERS should have no duplicates"
        );
    }

    #[test]
    fn strip_mathjax_escaped_underscores() {
        let pass = MathJaxPass;
        // Markdown-escaped variant from HTML→MD conversion.
        let input = r"italic\_p-value=0.05";
        assert_eq!(pass.apply(input), "p-value=0.05");
    }

    #[test]
    fn strip_mathjax_escaped_postsubscript() {
        let pass = MathJaxPass;
        let input = r"start\_POSTSUBSCRIPT i end\_POSTSUBSCRIPT";
        assert_eq!(pass.apply(input), " i ");
    }
}
