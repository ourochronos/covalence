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
}
