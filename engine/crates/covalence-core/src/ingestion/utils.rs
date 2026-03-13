//! Shared utility functions for the ingestion pipeline.

/// Compute cosine similarity between two embedding vectors.
///
/// Returns 0.0 for mismatched lengths, empty vectors, or zero-norm
/// vectors (avoids division by zero).
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a < 1e-12 || norm_b < 1e-12 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Escape invalid LaTeX backslash sequences inside JSON string
/// values so that `serde_json` can parse LLM output.
///
/// LLMs sometimes emit raw LaTeX (`\omega`, `\ddot{u}`) inside JSON
/// strings. These are not valid JSON escapes. This function doubles
/// any backslash not followed by a valid JSON escape character
/// (`"`, `\`, `/`, `b`, `f`, `n`, `r`, `t`, `u`).
///
/// The function is safe against already-escaped input: `\\omega`
/// is processed as `\` (valid escape `\\`) followed by `omega`
/// (literal chars), producing `\\omega` unchanged.
pub fn sanitize_latex_in_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 32);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if matches!(next, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') {
                    // Valid JSON escape — pass through both characters.
                    out.push('\\');
                    out.push(chars.next().unwrap());
                } else {
                    // Invalid escape — double the backslash.
                    out.push('\\');
                    out.push('\\');
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-10);
    }

    #[test]
    fn orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-10);
    }

    #[test]
    fn opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-10);
    }

    #[test]
    fn mismatched_lengths() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn empty_vectors() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn zero_vector() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn sanitize_latex_basic() {
        let input = r#"{"title": "\omega_{x|y} values"}"#;
        let sanitized = sanitize_latex_in_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
        assert!(parsed["title"].as_str().unwrap().contains("omega"));
    }

    #[test]
    fn sanitize_latex_preserves_valid_escapes() {
        let input = r#"{"title": "line1\nline2", "tab": "\t"}"#;
        let sanitized = sanitize_latex_in_json(input);
        assert_eq!(sanitized, input, "valid escapes must be unchanged");
    }

    #[test]
    fn sanitize_latex_ddot_and_overline() {
        let input = r#"{"summary": "The value \ddot{u} for \overline{y}"}"#;
        let sanitized = sanitize_latex_in_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
        let summary = parsed["summary"].as_str().unwrap();
        assert!(summary.contains("ddot"));
        assert!(summary.contains("overline"));
    }

    #[test]
    fn sanitize_latex_trailing_backslash() {
        let input = r#"{"x": "end\"#;
        let sanitized = sanitize_latex_in_json(input);
        assert!(sanitized.ends_with('\\'));
    }

    #[test]
    fn sanitize_latex_already_escaped() {
        let input = r#"{"x": "\\omega stays \\alpha"}"#;
        let sanitized = sanitize_latex_in_json(input);
        assert_eq!(sanitized, input, "already-escaped backslashes stay");
    }

    #[test]
    fn sanitize_latex_empty_string() {
        assert_eq!(sanitize_latex_in_json(""), "");
    }

    #[test]
    fn sanitize_latex_no_backslashes() {
        let input = r#"{"key": "plain text"}"#;
        assert_eq!(sanitize_latex_in_json(input), input);
    }
}
