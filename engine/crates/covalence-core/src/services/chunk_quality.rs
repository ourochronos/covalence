//! Chunk quality filters — heuristics for discarding ingestion artifacts.
//!
//! These functions detect chunks that are metadata-only, boilerplate-heavy,
//! author blocks, bibliography entries, or web-scraping artifacts.  They are
//! used during the ingestion pipeline to prevent low-quality chunks from
//! entering the knowledge graph.

/// Known boilerplate lines from arxiv and academic paper pages.
pub(crate) const BOILERPLATE_LINES: &[&str] = &[
    "view pdf",
    "view a pdf",
    "html (experimental)",
    "cite as:",
    "subjects:",
    "comments:",
    "download pdf",
    "download:",
    "bibtex",
    "submission history",
    "< prev",
    "next >",
    "report issue for preceding element",
    "report github issue",
    "submit without github",
    "submit in github",
    "content selection saved",
    "back to arxiv",
    "back to abstract",
    "why html?",
];

/// Known artifact headings from web scraping that should cause the
/// entire chunk to be discarded.
pub(crate) const ARTIFACT_HEADINGS: &[&str] = &["report issue for preceding element"];

/// Check if a chunk's content is purely metadata with no substantive
/// text. Returns `true` for chunks whose lines are all bold labels
/// (`**Key:** value`), blank lines, or heading markers with no body.
///
/// These chunks are ingestion artifacts from metadata appearing before
/// the first heading in a source document.
pub(crate) fn is_metadata_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }

    // Very short chunks (< 30 non-whitespace chars) are almost
    // always heading-only or UI fragment artifacts.
    let non_ws: usize = trimmed.chars().filter(|c| !c.is_whitespace()).count();
    if non_ws < 30 {
        return true;
    }

    // Short chunks (< 80 chars) where every line is a bold label or
    // blank are considered metadata-only.
    if trimmed.len() >= 80 {
        return false;
    }
    trimmed.lines().all(|line| {
        let l = line.trim();
        l.is_empty()
            || (l.starts_with("**") && l.contains(":**"))
            || (l.starts_with('[')
                && l.len() < 20
                && l.chars().skip(1).take(4).all(|c| c.is_ascii_digit()))
    })
}

/// Check whether a chunk is dominated by web UI boilerplate
/// (navigation elements, arxiv metadata, etc.) rather than
/// substantive content.
///
/// Returns `true` when ≥60% of non-blank lines match known
/// boilerplate patterns.
pub(crate) fn is_boilerplate_heavy(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false; // handled by is_metadata_only
    }
    let lines: Vec<&str> = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return false;
    }
    let boilerplate_count = lines.iter().filter(|l| is_boilerplate_line(l)).count();
    let ratio = boilerplate_count as f64 / lines.len() as f64;
    ratio >= 0.6
}

/// Check whether a single line matches boilerplate patterns.
pub(crate) fn is_boilerplate_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    // Known boilerplate strings.
    if BOILERPLATE_LINES.iter().any(|bp| lower.contains(bp)) {
        return true;
    }
    // Bold label lines: **Key:** value
    if line.starts_with("**") && line.contains(":**") {
        return true;
    }
    // Very short lines (< 15 chars) that are just labels.
    if line.len() < 15 && (lower.ends_with(':') || lower.ends_with("...")) {
        return true;
    }
    // Navigation/ToC links: numbered items pointing to sections.
    // Pattern: "01. [Section](url)" or "[Section](url#anchor)"
    let trimmed = line.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let trimmed = trimmed.trim();
    if trimmed.starts_with('[') && trimmed.contains("](") && trimmed.contains("://") {
        return true;
    }
    false
}

/// Detect author-block chunks: sequences of names, affiliations, and
/// email addresses with no substantive content.  These appear in scraped
/// academic papers as the header block before the abstract.
///
/// Heuristic: if ≥40% of non-blank lines contain an email indicator
/// (`@` or `mailto:`) the chunk is considered an author block.
pub(crate) fn is_author_block(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return false;
    }

    // Pattern 1: starts with "Authors:" prefix — common in arxiv
    // scraped content. The first line names the authors and the
    // rest is affiliations/institutions.
    let first = lines[0];
    if first.starts_with("Authors:") || first.starts_with("**Authors:**") {
        return true;
    }

    // Pattern 2: high ratio of email/mailto lines (≥ 40%).
    if lines.len() >= 2 {
        let email_lines = lines
            .iter()
            .filter(|l| l.contains('@') || l.contains("mailto:"))
            .count();
        let ratio = email_lines as f64 / lines.len() as f64;
        if ratio >= 0.4 {
            return true;
        }
    }

    false
}

/// Detect bibliography/reference entry chunks.
///
/// Short chunks (<300 chars) that match bibliography patterns:
/// - Start with `"- Author (Year)"` or `"- Author et al. (Year)"`
/// - Contain an `arXiv preprint` or journal citation
/// - Are individual citation entries from a References section
///
/// These are per-citation fragments from academic papers that add
/// noise without substantive content.
pub(crate) fn is_bibliography_entry(text: &str) -> bool {
    let trimmed = text.trim();
    // Only applies to short fragments — a full references section
    // would be longer and should be kept.
    if trimmed.len() > 300 {
        return false;
    }
    let lines: Vec<&str> = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return false;
    }

    let first = lines[0];

    // Pattern: "- Author (Year)" or "- Author et al. (Year)"
    if first.starts_with("- ") && first.len() < 80 {
        // Check for "(YYYY)" pattern
        if first
            .chars()
            .collect::<Vec<_>>()
            .windows(6)
            .any(|w| w[0] == '(' && w[1..5].iter().all(|c| c.is_ascii_digit()) && w[5] == ')')
        {
            return true;
        }
    }

    // Pattern: single citation with "arXiv preprint" or italic journal
    if lines.len() <= 4 {
        let joined = lines.join(" ");
        let lower = joined.to_lowercase();
        if (lower.contains("arxiv preprint") || lower.contains("_arxiv_")) && trimmed.len() < 200 {
            return true;
        }
        // Pattern: short fragment with journal/proceedings italics.
        // Academic citations often appear as:
        //   "Title.\n\n_Proc. VLDB Endow._ 5, 12 (2012), 2018."
        //   "Title.\n\nMorgan & Claypool Publishers."
        if trimmed.len() < 200 {
            // Contains italic markup wrapping a journal or proceedings name
            if lower.contains("_proc.") || lower.contains("_proceedings") {
                return true;
            }
            // Contains doi.org URL (standalone citation reference)
            if lower.contains("doi.org/") && lines.len() <= 3 {
                return true;
            }
        }
        // Pattern: very short fragment ending with a publisher name
        if trimmed.len() < 150 && lines.len() <= 3 {
            let publisher_markers = [
                "publishers",
                "springer",
                "ieee",
                "acm press",
                "morgan & claypool",
                "mit press",
                "cambridge university press",
                "oxford university press",
                "report issue",
            ];
            if publisher_markers.iter().any(|m| lower.contains(m)) {
                return true;
            }
        }
    }

    false
}

/// Returns `true` if any heading in the chunk's path matches a known
/// web-scraping artifact heading.
pub(crate) fn has_artifact_heading(heading_path: &[String]) -> bool {
    heading_path.iter().any(|h| {
        let lower = h.to_lowercase();
        ARTIFACT_HEADINGS.iter().any(|a| lower.contains(a))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_only_bold_labels() {
        assert!(is_metadata_only("**Authors:** John\n**arxiv:** 123"));
    }

    #[test]
    fn metadata_only_empty() {
        assert!(is_metadata_only(""));
        assert!(is_metadata_only("  \n  "));
    }

    #[test]
    fn metadata_only_short_arxiv() {
        assert!(is_metadata_only("[2506.12345]"));
    }

    #[test]
    fn not_metadata_has_content() {
        assert!(!is_metadata_only(
            "**Authors:** John\nThis paper discusses knowledge graphs."
        ));
    }

    #[test]
    fn not_metadata_long_text() {
        let long = "**Authors:** A very long author list that exceeds eighty characters in total length and counts as real content";
        assert!(!is_metadata_only(long));
    }

    #[test]
    fn metadata_only_very_short_chunks() {
        // ArXiv UI fragments that slip through as chunks
        assert!(is_metadata_only("##### Report GitHub Issue"));
        assert!(is_metadata_only("×\n\nTitle:"));
        assert!(is_metadata_only("Description:"));
    }

    #[test]
    fn not_metadata_substantive_short() {
        // 30+ non-whitespace chars = substantive
        assert!(!is_metadata_only(
            "Knowledge graphs enable reasoning about entities."
        ));
    }

    #[test]
    fn boilerplate_arxiv_ui_fragments() {
        assert!(is_boilerplate_line("Report GitHub Issue"));
        assert!(is_boilerplate_line("Submit without GitHubSubmit in GitHub"));
        assert!(is_boilerplate_line(
            "Content selection saved. Describe the issue below:"
        ));
        assert!(is_boilerplate_line("Back to arXiv"));
        assert!(is_boilerplate_line("Why HTML?"));
    }

    #[test]
    fn bibliography_entry_with_year() {
        assert!(is_bibliography_entry(
            "- Aggarwal et al. (2001)\nCharu C Aggarwal, Alexander Hinneburg."
        ));
    }

    #[test]
    fn bibliography_entry_arxiv() {
        assert!(is_bibliography_entry(
            "GPT-4 Technical Report.\n\n_ArXiv_, abs/2303.08774, 2023."
        ));
    }

    #[test]
    fn bibliography_entry_standalone_citation() {
        assert!(is_bibliography_entry(
            "- OpenAI (2023)\nOpenAI.\n\nGPT-4 Technical Report."
        ));
    }

    #[test]
    fn not_bibliography_substantive_content() {
        let text = "Knowledge graphs are a powerful way to represent structured \
                    information. They consist of nodes representing entities and edges \
                    representing relationships between them. This enables sophisticated \
                    reasoning about the world.";
        assert!(!is_bibliography_entry(text));
    }

    #[test]
    fn not_bibliography_long_references() {
        // A full references section (>300 chars) should be kept.
        let long = "- Author (2020)\n".repeat(30);
        assert!(!is_bibliography_entry(&long));
    }

    #[test]
    fn bibliography_entry_journal_italic() {
        assert!(is_bibliography_entry(
            "Entity resolution: theory, practice & open challenges.\n\n\
             _Proc. VLDB Endow._ 5, 12 (2012), 2018\u{2013}2019."
        ));
    }

    #[test]
    fn bibliography_entry_publisher() {
        assert!(is_bibliography_entry(
            "_The four generations of entity resolution_.\n\n\
             Morgan & Claypool Publishers."
        ));
    }

    #[test]
    fn bibliography_entry_doi() {
        assert!(is_bibliography_entry(
            "ing, C. D.: RAPTOR: Recursive Abstractive Processing.\n\n\
             doi.org/10.48550/arXiv.2401.18059"
        ));
    }

    #[test]
    fn bibliography_entry_report_issue() {
        assert!(is_bibliography_entry("ing, C. D.: RAPTOR.\n\nReport Issue"));
    }

    #[test]
    fn boilerplate_heavy_arxiv_page() {
        let text = "Authors:John Doe, Jane Smith\n\
                    View a PDF of the paper\n\
                    View PDF\n\
                    HTML (experimental)\n\
                    Subjects:\n\
                    Cite as:";
        assert!(is_boilerplate_heavy(text));
    }

    #[test]
    fn boilerplate_heavy_nav_elements() {
        let text = "< prev\nnext >\nView PDF\nDownload PDF";
        assert!(is_boilerplate_heavy(text));
    }

    #[test]
    fn not_boilerplate_real_content() {
        let text = "Knowledge graphs represent entities and relationships.\n\
                    This enables multi-hop reasoning across documents.\n\
                    The system uses RRF for fusion.";
        assert!(!is_boilerplate_heavy(text));
    }

    #[test]
    fn not_boilerplate_mixed_content() {
        // Some boilerplate but majority is real content.
        let text = "**Authors:** John Doe\n\
                    Knowledge graphs are important.\n\
                    They enable structured retrieval.\n\
                    RRF fuses multiple signal dimensions.\n\
                    The system uses pgvector for embeddings.";
        assert!(!is_boilerplate_heavy(text));
    }

    #[test]
    fn boilerplate_line_detection() {
        assert!(is_boilerplate_line("View PDF"));
        assert!(is_boilerplate_line("< prev"));
        assert!(is_boilerplate_line("**Authors:** John"));
        assert!(is_boilerplate_line("Subjects:"));
        assert!(!is_boilerplate_line("Knowledge graphs enable reasoning."));
    }

    #[test]
    fn boilerplate_report_issue_arxiv() {
        assert!(is_boilerplate_line("Report issue for preceding element"));
    }

    #[test]
    fn boilerplate_toc_nav_links() {
        assert!(is_boilerplate_line(
            "01. [Abstract](https://arxiv.org/html/2506.02509#abstract \"Abstract\")"
        ));
        assert!(is_boilerplate_line(
            "[1 Introduction](https://arxiv.org/html/2506.02509v1#S1 \"Title\")"
        ));
    }

    #[test]
    fn not_boilerplate_inline_link() {
        // A sentence with a link is NOT boilerplate.
        assert!(!is_boilerplate_line("See the original paper for details."));
    }

    #[test]
    fn artifact_heading_filter() {
        let path = vec!["Report issue for preceding element".to_string()];
        assert!(has_artifact_heading(&path));
    }

    #[test]
    fn artifact_heading_nested() {
        let path = vec![
            "My Paper".to_string(),
            "Report issue for preceding element".to_string(),
        ];
        assert!(has_artifact_heading(&path));
    }

    #[test]
    fn artifact_heading_clean_path() {
        let path = vec!["Introduction".to_string(), "Methods".to_string()];
        assert!(!has_artifact_heading(&path));
    }

    #[test]
    fn artifact_heading_empty_path() {
        let path: Vec<String> = vec![];
        assert!(!has_artifact_heading(&path));
    }

    #[test]
    fn author_block_detected() {
        // Each author line typically has name + email on same line
        // (or email on a short adjacent line).
        let text = "Bo Liu Beijing Institute of Technology liubo@bit.edu.cn\n\
                    Yanjie Jiang Peking University yanjiejiang@pku.edu.cn\n\
                    Yuxia Zhang Beijing Institute of Technology yuxiazh@bit.edu.cn\n\
                    Nan Niu University of Cincinnati nan.niu@uc.edu\n\
                    Guangjie Li National Innovation Institute liguangjie@126.com";
        assert!(is_author_block(text));
    }

    #[test]
    fn author_block_mailto_links() {
        let text = "Alice Smith\n\
                    [alice@example.com](mailto:alice@example.com)\n\
                    Bob Jones\n\
                    [bob@test.org](mailto:bob@test.org)\n\
                    Carol White\n\
                    [carol@uni.edu](mailto:carol@uni.edu)";
        assert!(is_author_block(text));
    }

    #[test]
    fn not_author_block_real_content() {
        let text = "Knowledge graphs represent entities and relationships.\n\
                    This enables multi-hop reasoning across documents.\n\
                    Contact us at support@example.com for details.";
        assert!(!is_author_block(text));
    }

    #[test]
    fn not_author_block_single_email() {
        let text = "Send questions to admin@example.com";
        assert!(!is_author_block(text));
    }

    #[test]
    fn author_block_prefix_detected() {
        let text = "Authors:Hairong Zhang, Jiaheng Si, Guohang Yan, Boyuan Qi";
        assert!(is_author_block(text));
    }

    #[test]
    fn author_block_bold_prefix_detected() {
        let text = "**Authors:** Alice Smith, Bob Jones, Carol White\n\
                    University of Example, Department of CS";
        assert!(is_author_block(text));
    }

    #[test]
    fn not_author_block_empty() {
        assert!(!is_author_block(""));
    }
}
