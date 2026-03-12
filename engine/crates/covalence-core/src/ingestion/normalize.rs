//! Stage 3: Normalize parsed output to extended Markdown.
//!
//! All formats convert to Markdown as the canonical intermediate representation.
//! Applies Unicode NFC normalization, collapses whitespace, strips control
//! characters (preserving newlines), and trims.

use unicode_normalization::UnicodeNormalization;

/// Normalize text for consistent processing.
///
/// Steps:
/// 1. Unicode NFC normalization
/// 2. Strip control characters (keep `\n`)
/// 3. Collapse multiple whitespace to single space (preserving `\n`)
/// 4. Trim leading/trailing whitespace
pub fn normalize(text: &str) -> String {
    let nfc: String = text.nfc().collect();

    let cleaned: String = nfc
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();

    let mut result = String::with_capacity(cleaned.len());
    let mut prev_space = false;

    for ch in cleaned.chars() {
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

    result.trim().to_string()
}

/// Lines (or line prefixes) that are ArXiv/web-scraping artifacts.
/// If a line starts with any of these (case-insensitive), it is
/// removed entirely.
const ARTIFACT_LINE_PREFIXES: &[&str] = &[
    "report issue for preceding element",
    "html conversions sometimes display errors",
    "authors: achieve the best html results",
];

/// Remove known web-scraping artifact lines from normalized
/// markdown. Applied after Unicode normalization.
pub fn strip_artifacts(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let input =
            "Authors: achieve the best HTML results from your LaTeX\nActual content";
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
}
