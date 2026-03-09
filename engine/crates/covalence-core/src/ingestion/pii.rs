//! PII detection for ingestion pipeline content.
//!
//! Provides pattern-based detection of personally identifiable
//! information (PII) in text before it enters the knowledge graph.

use regex::Regex;

/// A detected PII match within a text span.
#[derive(Debug, Clone, PartialEq)]
pub struct PiiMatch {
    /// Byte offset of the match start.
    pub start: usize,
    /// Byte offset of the match end (exclusive).
    pub end: usize,
    /// The type of PII detected (e.g. "email", "phone", "ssn",
    /// "credit_card").
    pub pii_type: String,
    /// Detection confidence (0.0 to 1.0).
    pub confidence: f64,
}

/// Trait for detecting PII in text.
pub trait PiiDetector: Send + Sync {
    /// Scan text and return all detected PII matches.
    fn detect(&self, text: &str) -> Vec<PiiMatch>;
}

/// Regex-based PII detector that identifies common PII patterns.
///
/// Detects email addresses, US phone numbers, SSN patterns, and
/// credit card numbers using regular expressions.
pub struct RegexPiiDetector {
    email_re: Regex,
    phone_re: Regex,
    ssn_re: Regex,
    credit_card_re: Regex,
}

impl RegexPiiDetector {
    /// Create a new regex-based PII detector with default patterns.
    pub fn new() -> Self {
        Self {
            email_re: Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
                .unwrap_or_else(|_| Regex::new("(?:)").unwrap()),
            phone_re: Regex::new(r"(?:\+?1[-.\s]?)?\(?[2-9]\d{2}\)?[-.\s]?\d{3}[-.\s]?\d{4}")
                .unwrap_or_else(|_| Regex::new("(?:)").unwrap()),
            ssn_re: Regex::new(r"\b\d{3}[-]\d{2}[-]\d{4}\b")
                .unwrap_or_else(|_| Regex::new("(?:)").unwrap()),
            credit_card_re: Regex::new(r"\b(?:\d{4}[-\s]?){3}\d{4}\b")
                .unwrap_or_else(|_| Regex::new("(?:)").unwrap()),
        }
    }
}

impl Default for RegexPiiDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl PiiDetector for RegexPiiDetector {
    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let mut matches = Vec::new();

        for m in self.email_re.find_iter(text) {
            matches.push(PiiMatch {
                start: m.start(),
                end: m.end(),
                pii_type: "email".to_string(),
                confidence: 0.95,
            });
        }

        for m in self.phone_re.find_iter(text) {
            matches.push(PiiMatch {
                start: m.start(),
                end: m.end(),
                pii_type: "phone".to_string(),
                confidence: 0.85,
            });
        }

        for m in self.ssn_re.find_iter(text) {
            matches.push(PiiMatch {
                start: m.start(),
                end: m.end(),
                pii_type: "ssn".to_string(),
                confidence: 0.90,
            });
        }

        for m in self.credit_card_re.find_iter(text) {
            matches.push(PiiMatch {
                start: m.start(),
                end: m.end(),
                pii_type: "credit_card".to_string(),
                confidence: 0.80,
            });
        }

        // Sort by position for consistent ordering.
        matches.sort_by_key(|m| m.start);
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_email() {
        let detector = RegexPiiDetector::new();
        let text = "Contact me at user@example.com for info.";
        let matches = detector.detect(text);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, "email");
        assert_eq!(&text[matches[0].start..matches[0].end], "user@example.com");
        assert!(matches[0].confidence > 0.9);
    }

    #[test]
    fn detect_phone() {
        let detector = RegexPiiDetector::new();
        let text = "Call me at (555) 123-4567 or 555.987.6543.";
        let matches = detector.detect(text);
        assert!(matches.len() >= 2);
        assert!(matches.iter().all(|m| m.pii_type == "phone"));
    }

    #[test]
    fn detect_ssn() {
        let detector = RegexPiiDetector::new();
        let text = "SSN: 123-45-6789 is sensitive.";
        let matches = detector.detect(text);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, "ssn");
        assert_eq!(&text[matches[0].start..matches[0].end], "123-45-6789");
    }

    #[test]
    fn detect_credit_card() {
        let detector = RegexPiiDetector::new();
        let text = "Card: 4111-1111-1111-1111 on file.";
        let matches = detector.detect(text);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, "credit_card");
    }

    #[test]
    fn detect_multiple_types() {
        let detector = RegexPiiDetector::new();
        let text = "Email: test@example.com, SSN: 987-65-4321, \
             Phone: (415) 555-0100";
        let matches = detector.detect(text);
        assert!(matches.len() >= 3);
        let types: Vec<_> = matches.iter().map(|m| m.pii_type.as_str()).collect();
        assert!(types.contains(&"email"));
        assert!(types.contains(&"ssn"));
        assert!(types.contains(&"phone"));
    }

    #[test]
    fn no_pii_returns_empty() {
        let detector = RegexPiiDetector::new();
        let text = "This is a perfectly clean text with no PII.";
        let matches = detector.detect(text);
        assert!(matches.is_empty());
    }

    #[test]
    fn matches_sorted_by_position() {
        let detector = RegexPiiDetector::new();
        let text = "SSN: 111-22-3333, email: a@b.com, phone: (555) 111-2222";
        let matches = detector.detect(text);
        for i in 1..matches.len() {
            assert!(matches[i].start >= matches[i - 1].start);
        }
    }
}
