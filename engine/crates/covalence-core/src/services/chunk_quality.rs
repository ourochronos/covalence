//! Chunk quality filters — heuristics for discarding ingestion artifacts.
//!
//! These functions detect chunks that are metadata-only, boilerplate-heavy,
//! author blocks, bibliography entries, or web-scraping artifacts.  They are
//! used during the ingestion pipeline to prevent low-quality chunks from
//! entering the knowledge graph.

/// Known boilerplate lines from arxiv and academic paper pages.
pub const BOILERPLATE_LINES: &[&str] = &[
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
    "tex source",
    "view license",
    "browse context",
    "current browse context",
    "change to browse by",
];

/// Known artifact headings from web scraping that should cause the
/// entire chunk to be discarded.
pub const ARTIFACT_HEADINGS: &[&str] = &["report issue for preceding element"];

/// Check if a chunk's content is purely metadata with no substantive
/// text. Returns `true` for chunks whose lines are all bold labels
/// (`**Key:** value`), blank lines, or heading markers with no body.
///
/// These chunks are ingestion artifacts from metadata appearing before
/// the first heading in a source document.
pub fn is_metadata_only(text: &str) -> bool {
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
pub fn is_boilerplate_heavy(text: &str) -> bool {
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
pub fn is_boilerplate_line(line: &str) -> bool {
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
    // ArXiv navigation fragments: pure punctuation or very short
    // tokens like "|", "new", "recent", date patterns "| 2025-06".
    if line.len() <= 3 && !line.chars().any(|c| c.is_alphabetic()) {
        return true;
    }
    // ArXiv date nav: "| YYYY-MM" or "YYYY-MM"
    if line.len() < 15 {
        let stripped = line.trim_start_matches('|').trim();
        if stripped.len() >= 7
            && stripped.len() <= 10
            && stripped.as_bytes()[4] == b'-'
            && stripped[..4].chars().all(|c| c.is_ascii_digit())
            && stripped[5..7].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// Detect author-block chunks: sequences of names, affiliations, and
/// email addresses with no substantive content.  These appear in scraped
/// academic papers as the header block before the abstract.
///
/// Uses two heuristics:
/// - Global: if ≥40% of non-blank lines contain an email indicator.
/// - Prefix: if any of the first 6 non-blank lines contain an email
///   address and a heading marker (######) appears later. This catches
///   chunks where the author block is merged with the abstract.
pub fn is_author_block(text: &str) -> bool {
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

    // Pattern 3: email in the first 6 lines + heading later.
    // Catches chunks where the author header is merged with the
    // abstract (e.g., "Name1 Name2\n...\n@email\n######Abstract\n...").
    let prefix = &lines[..lines.len().min(6)];
    let has_prefix_email = prefix
        .iter()
        .any(|l| l.contains('@') || l.contains("mailto:"));
    let has_later_heading = lines.iter().skip(1).any(|l| l.starts_with('#'));
    if has_prefix_email && has_later_heading {
        return true;
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
pub fn is_bibliography_entry(text: &str) -> bool {
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

    // Pattern: "- \[N\]↑" — ArXiv HTML citation entry with escaped
    // bracket and arrow marker. These are unambiguously bibliography.
    if first.starts_with("- \\[") && first.contains('\u{2191}') {
        return true;
    }
    // Also catch without dash prefix: "\[N\]↑"
    if first.starts_with("\\[") && first.contains('\u{2191}') {
        return true;
    }

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
        // Pattern: "Title.\n\nJournal, vol(issue):pages."
        // Short citation where a non-first line contains journal
        // volume/issue references like "486(3-5):75–174".
        if lines.len() >= 2 && lines.len() <= 3 && trimmed.len() < 200 {
            let rest = lines[1..].join(" ");
            // Check "digit(" pattern (volume/issue) in non-first lines.
            let has_vol_issue = rest
                .as_bytes()
                .windows(2)
                .any(|w| w[0].is_ascii_digit() && w[1] == b'(');
            // Check "digit:digit" pattern (volume:page) in non-first lines.
            let has_vol_page = rest
                .as_bytes()
                .windows(3)
                .any(|w| w[0].is_ascii_digit() && w[1] == b':' && w[2].is_ascii_digit());
            if first.ends_with('.') && (has_vol_issue || has_vol_page) {
                return true;
            }
        }
    }

    false
}

/// Detect large reference/bibliography section chunks.
///
/// Unlike [`is_bibliography_entry`] (which catches individual short
/// citations), this catches full reference sections — long chunks
/// where the majority of lines are citation-like or citation-adjacent.
///
/// A line is "citation-like" if it matches any of:
/// - List item (`- `, `* `, `[N]`) with a `(YYYY)` year pattern
/// - "Retrieved" URL line
/// - Standalone arrow/citation marker like `(1)↑`
/// - Contains "arXiv preprint" or "arXiv:" (preprint reference)
/// - Is a standalone year line like `2023.` or `(2024)`
/// - Contains a DOI URL
/// - Ends with a bare year (`YYYY.`) — author lines in multi-line
///   citations
/// - Starts with italic markup (`_`) — journal/conference names
/// - Starts with `In _` — conference proceedings
/// - Contains "et al." — author list continuation
/// - Escaped bracket list item (`\[N\]`) — markdown-escaped
///   citation numbers
pub fn is_reference_section(text: &str) -> bool {
    let trimmed = text.trim();
    // Only applies to substantial chunks (the small ones are handled
    // by `is_bibliography_entry`).
    if trimmed.len() < 300 {
        return false;
    }

    let lines: Vec<&str> = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 5 {
        return false;
    }

    let citation_count = lines
        .iter()
        .filter(|line| {
            let is_list_item =
                line.starts_with("- ") || line.starts_with("* ") || line.starts_with('[');
            let has_year = line.as_bytes().windows(6).any(|w| {
                w[0] == b'(' && w[1..5].iter().all(|b| b.is_ascii_digit()) && w[5] == b')'
            });
            // Bare year at end of line: "...Name. 2004." or "2023."
            let ends_with_bare_year = {
                let bytes = line.as_bytes();
                bytes.len() >= 5
                    && bytes[bytes.len() - 1] == b'.'
                    && bytes[bytes.len() - 5..bytes.len() - 1]
                        .iter()
                        .all(|b| b.is_ascii_digit())
            };
            let lower = line.to_lowercase();

            // Primary: list item with year, or "Retrieved" line,
            // or standalone arrow marker.
            (is_list_item && has_year)
                || line.starts_with("Retrieved ")
                || (line.len() < 15 && line.contains('\u{2191}'))
                // arXiv preprint reference lines
                || lower.contains("arxiv preprint arxiv:")
                || lower.contains("arxiv preprint,")
                || lower.contains("_arxiv preprint")
                // DOI reference lines
                || lower.contains("doi.org/")
                // Standalone year lines like "2023." or "(2024)."
                || (line.len() < 10 && has_year)
                // Arrow markers with year (multi-line citations)
                || (line.contains('\u{2191}') && has_year)
                // List item that is a citation header (has ↑ marker)
                || (is_list_item && line.contains('\u{2191}'))
                // Bare year at end of line — author lines in
                // multi-line citations end with "Name. YYYY."
                || ends_with_bare_year
                // Italic journal/conference names: "_Journal_, vol..."
                || line.starts_with('_')
                // Conference proceedings: "In _Proceedings of..._"
                || line.starts_with("In _")
                // "et al." is extremely citation-specific
                || lower.contains("et al.")
                // Escaped bracket list items: "- \[1\]↑" or "\[1\]↑"
                || line.starts_with("\\[")
                || line.starts_with("- \\[")
        })
        .count();

    let ratio = citation_count as f64 / lines.len() as f64;
    // If >30% of lines are citation-like, it's a reference section.
    // (lowered from 40% because multi-line citations have many
    // continuation lines that are author names/paper titles.)
    ratio > 0.3
}

/// Detect "title-only" chunks: chunks where the content is just a
/// heading or title with no body text. These occur when scraped pages
/// emit a title element followed by empty whitespace or HTML fragments.
///
/// A chunk is title-only when it has ≤2 meaningful lines (non-blank,
/// non-HTML-comment, non-whitespace-only) after cleanup.
pub fn is_title_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false; // handled by is_metadata_only
    }

    let meaningful_lines: Vec<&str> = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("-->")
                && !l.starts_with("<!--")
                && l.chars().any(|c| c.is_alphanumeric())
        })
        .collect();

    // 0 or 1 meaningful lines = title-only
    if meaningful_lines.len() <= 1 {
        return true;
    }

    // 2 meaningful lines: if the second is also very short (< 30 chars)
    // and looks like a subtitle or fragment, still title-only.
    if meaningful_lines.len() == 2 {
        let second = meaningful_lines[1];
        if second.len() < 30 {
            return true;
        }
    }

    false
}

/// Returns `true` if any heading in the chunk's path matches a known
/// web-scraping artifact heading.
pub fn has_artifact_heading(heading_path: &[String]) -> bool {
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
    fn bibliography_entry_escaped_bracket_arrow() {
        // ArXiv HTML citation entry with escaped brackets and ↑ marker.
        assert!(is_bibliography_entry(
            "- \\[9\\]\u{2191}\nLu Dai, Yijie Xu, Jinhui Ye, Hao Liu, and Hui Xiong.\n\n\
             Seper: Measure retrieval utility through the lens of semantic perplexity reduction."
        ));
        // Without dash prefix
        assert!(is_bibliography_entry(
            "\\[42\\]\u{2191}\nSome Author et al.\n\nSome paper title."
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
    fn bibliography_entry_journal_vol_pages() {
        // Title + journal with volume(issue):pages format
        assert!(is_bibliography_entry(
            "Community detection in graphs.\n\nPhysics reports, 486(3-5):75\u{2013}174."
        ));
    }

    #[test]
    fn bibliography_entry_volume_colon() {
        assert!(is_bibliography_entry(
            "Some paper title.\n\nJournal of ML Research, 15:1929\u{2013}1958."
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
    fn boilerplate_heavy_arxiv_abstract_page() {
        // Real ArXiv abstract page chunk: title + nav + browse context
        let text = "View a PDF of the paper titled Some Paper\n\
                    - View PDF\n\
                    - HTML (experimental)\n\
                    - TeX Source\n\
                    view license\n\
                    Current browse context: cs.IR\n\
                    < prev\n\
                    |\n\
                    next >\n\
                    new\n\
                    |\n\
                    recent\n\
                    | 2025-06\n\
                    Change to browse by:\n\
                    cs\n\
                    cs.CL";
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

    #[test]
    fn author_block_merged_with_abstract() {
        // Real-world case: arxiv paper header merged with abstract.
        // Email is in the first 6 lines, heading appears later.
        let text = "Zhuowan Li1 Cheng Li1 Mingyang Zhang1\n\
                    Qiaozhu Mei2 Michael Bendersky1\n\
                    1 Google DeepMind 2 University of Michigan\n\
                    1 {zhuowan,chgli}@google.com 2 qmei@umich.edu\n\
                    ###### Abstract\n\
                    Retrieval Augmented Generation (RAG) has been a powerful tool.";
        assert!(is_author_block(text));
    }

    #[test]
    fn not_author_block_email_in_body_no_heading() {
        // Email in first 6 lines but no later heading — could be
        // legitimate content with a contact reference.
        let text = "Contact the team at admin@example.com\n\
                    for more information about the project.\n\
                    We are based in San Francisco.";
        assert!(!is_author_block(text));
    }

    // --- Reference section tests ---

    #[test]
    fn reference_section_detected() {
        // Simulate a typical academic reference list chunk.
        let mut lines = Vec::new();
        for i in 0..20 {
            lines.push(format!(
                "- Author{i} et al. (20{:02}). Some paper title. _Journal_ {i}.",
                i % 25
            ));
        }
        let text = lines.join("\n");
        assert!(is_reference_section(&text));
    }

    #[test]
    fn reference_section_with_arrows() {
        // ArXiv-style references with ↑ markers.
        let text = "- (1)\u{2191}\n\
            - api (2024)\u{2191}\n\
            2024.\n\
            API Pricing of Open-AI.\n\
            Retrieved December 16, 2024 from https://openai.com/api/pricing/\n\
            - Ahmed and Abulaish (2012)\u{2191}\n\
            Faraz Ahmed and Muhammad Abulaish. 2012.\n\
            An MCL-based approach for spam profile detection.\n\
            - Amer-Yahia et al. (2023)\u{2191}\n\
            Sihem Amer-Yahia et al. 2023.\n\
            Some other paper about data management.\n\
            - Bennett (2021)\u{2191}\n\
            J. Bennett. 2021.\n\
            Yet another reference entry.";
        assert!(is_reference_section(text));
    }

    #[test]
    fn reference_section_multiline_arxiv() {
        // Multi-line citation format from GraphRAG paper (arXiv style).
        // Each citation spans 3-4 lines: header with ↑, author line, title, arXiv ref.
        let text = "- Achiam et al., (2023)\u{2191}\n\
            Achiam, J., Adler, S., Agarwal, S., Ahmad, L.\n\
            Gpt-4 technical report.\n\
            arXiv preprint arXiv:2303.08774.\n\
            - Anil et al., (2023)\u{2191}\n\
            Anil, R., Borgeaud, S., Wu, Y., Alayrac, J.-B.\n\
            Gemini: a family of highly capable multimodal models.\n\
            arXiv preprint arXiv:2312.11805.\n\
            - Baek et al., (2023)\u{2191}\n\
            Baek, J., Aji, A. F., and Saffari, A.\n\
            Knowledge-augmented language model prompting.\n\
            arXiv preprint arXiv:2305.18846.\n\
            - Brown et al., (2020)\u{2191}\n\
            Brown, T., Mann, B., Ryder, N., Subbiah, M.\n\
            Language models are few-shot learners.\n\
            Advances in neural information processing systems.";
        assert!(is_reference_section(text));
    }

    #[test]
    fn reference_section_escaped_brackets_arxiv_html() {
        // ArXiv HTML bibliography format: escaped brackets, bare years,
        // italic journal names. Each citation spans ~7 non-blank lines
        // but only the header has ↑ — the continuation lines (author,
        // year, title, journal) must also be detected.
        let text = "- \\[1\\]\u{2191}\n\
            Nasreen Abdul-Jaleel, James Allan, W Bruce Croft, Fernando Diaz, Leah Larkey,\n\
            Xiaoyan Li, Mark D Smucker, and Courtney Wade. 2004.\n\
            \n\
            Umass at trec 2004: Novelty and hard.\n\
            \n\
            _Computer Science Department Faculty Publication Series_, page\n\
            189.\n\
            \n\
            - \\[2\\]\u{2191}\n\
            Josh Achiam, Steven Adler, Sandhini Agarwal, Lama Ahmad, Ilge Akkaya,\n\
            Florencia Leoni Aleman, Diogo Almeida, et al. 2023.\n\
            \n\
            Gpt-4 technical report.\n\
            \n\
            _arXiv preprint arXiv:2303.08774_.\n\
            \n\
            - \\[3\\]\u{2191}\n\
            Payal Bajaj, Daniel Campos, Nick Craswell, Li Deng, et al. 2016.\n\
            \n\
            Ms marco: A human generated machine reading comprehension dataset.\n\
            \n\
            _arXiv preprint arXiv:1611.09268_.\n\
            \n\
            - \\[4\\]\u{2191}\n\
            Nicholas J. Belkin, Paul Kantor, Edward A. Fox, and Joseph A Shaw. 1995.\n\
            \n\
            Combining the evidence of multiple query representations for\n\
            information retrieval.\n\
            \n\
            _Information Processing & Management_, 31(3):431\u{2013}448.\n\
            \n\
            - \\[5\\]\u{2191}\n\
            Tao Chen, Mingyang Zhang, Jing Lu, Michael Bendersky, and Marc Najork. 2022.\n\
            \n\
            Out-of-domain semantics to the rescue! zero-shot hybrid retrieval\n\
            models.\n\
            \n\
            In _European Conference on Information Retrieval_, pages\n\
            95\u{2013}110. Springer.";
        assert!(is_reference_section(text));
    }

    #[test]
    fn not_reference_section_short() {
        let text = "- Author (2024). A paper.";
        assert!(!is_reference_section(text));
    }

    #[test]
    fn not_reference_section_real_content() {
        let text = "Knowledge graphs enable structured representation of facts.\n\
            They combine entity recognition with relationship extraction.\n\
            Graph neural networks can propagate information across edges.\n\
            Retrieval-augmented generation improves factual grounding.\n\
            Multi-hop reasoning requires traversal of intermediate nodes.\n\
            Community detection identifies densely connected subgraphs.\n\
            Temporal annotations track when facts become valid or expire.\n\
            Confidence scores quantify epistemic uncertainty.\n\
            Provenance chains link assertions to their source material.\n\
            Fusion algorithms combine evidence from multiple dimensions.";
        assert!(!is_reference_section(text));
    }

    #[test]
    fn not_reference_section_italic_content() {
        // Legitimate content with italic terms should not false-positive.
        let text = "The _Subjective Logic_ framework represents epistemic states\n\
            as opinion tuples. Each opinion has four components:\n\
            _belief_, _disbelief_, _uncertainty_, and _base rate_.\n\
            These components satisfy the constraint b + d + u = 1.\n\
            The _projected probability_ is computed as P = b + a * u.\n\
            This allows reasoning under genuine uncertainty.\n\
            Fusion operators combine opinions from multiple sources.\n\
            The _cumulative fusion_ operator is commutative.\n\
            _Averaging fusion_ handles dependent evidence.\n\
            Trust propagation uses _discount_ and _consensus_ operators.";
        assert!(!is_reference_section(text));
    }

    // --- Title-only tests ---

    #[test]
    fn title_only_arxiv_scraped() {
        // Exact pattern from production: title + whitespace padding + HTML comment
        let text = "[2309.11798] A Comprehensive Review of Community Detection in Graphs\n \
            \n \n \n \n \n \n \n \n \n \n \n \n \n \n \n \n\n \n \n \n \n \n\n \n \n \n \n\n \n \n\n \n \n-->";
        assert!(is_title_only(text));
    }

    #[test]
    fn title_only_single_heading() {
        assert!(is_title_only("# Introduction"));
        assert!(is_title_only("## Methods and Evaluation"));
    }

    #[test]
    fn title_only_with_short_subtitle() {
        let text = "Graph Neural Networks\nA brief overview";
        assert!(is_title_only(text));
    }

    #[test]
    fn not_title_only_has_body() {
        let text = "# Introduction\n\nThis paper presents a novel approach to entity resolution \
            using knowledge graph embeddings. We show that combining vector similarity with \
            graph neighborhood analysis improves F1 by 12%.";
        assert!(!is_title_only(text));
    }

    #[test]
    fn not_title_only_multi_paragraph() {
        let text = "Graph algorithms\n\n\
            Community detection partitions graphs into densely connected subgroups.\n\
            Several approaches exist, including modularity optimization and spectral methods.";
        assert!(!is_title_only(text));
    }

    #[test]
    fn title_only_empty_handled() {
        // Empty text is handled by is_metadata_only, not is_title_only
        assert!(!is_title_only(""));
    }
}
