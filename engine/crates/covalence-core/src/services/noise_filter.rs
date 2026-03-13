//! Entity noise filtering — prevents extraction artifacts from
//! polluting the knowledge graph.
//!
//! The [`is_noise_entity`] function applies heuristic rules to reject
//! entities that are clearly not meaningful domain knowledge: paper
//! titles, code syntax, math symbols, generic English words, HTML
//! artifacts, prices, statistics, and document structure references.
//!
//! Used in both the extraction pipeline (ingestion-time) and the
//! admin cleanup endpoint (retroactive).

/// Reject extracted entities that are clearly noise.
///
/// Catches paper titles, generic single words, code syntax, and math
/// symbols that the LLM extractor sometimes produces despite prompt
/// instructions.
pub(crate) fn is_noise_entity(name: &str, entity_type: &str) -> bool {
    let trimmed = name.trim();
    let lower = trimmed.to_lowercase();

    // Embedded newlines: entity names should be single-line.
    if trimmed.contains('\n') {
        return true;
    }

    // Named HTML entities (&amp;, &lt;, etc.) anywhere in the name.
    if trimmed.contains("&amp;") || trimmed.contains("&lt;") || trimmed.contains("&gt;") {
        return true;
    }

    // ALL_CAPS_SNAKE test constants (e.g., COVALENCE_TEST_CLAMP_12345).
    if trimmed.contains('_')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && trimmed.chars().filter(|c| *c == '_').count() >= 2
    {
        return true;
    }

    // Very short names (1-2 chars by char count) are noise unless
    // they're well-known abbreviations. Single letters like "C", "D",
    // "E" are mathematical variables from papers, not real entities.
    let char_count = trimmed.chars().count();
    if char_count <= 2 {
        const SHORT_ALLOWLIST: &[&str] = &[
            "AI", "DB", "GI", "IE", "IR", "IT", "KG", "ML", "NE", "NL", "NLP", "QA", "RL", "UI",
            "UX",
        ];
        if !SHORT_ALLOWLIST.contains(&trimmed) {
            return true;
        }
    }

    // Paper titles: concept entities with >55 chars are almost always
    // paper titles (real concepts are shorter).
    if entity_type == "concept" && trimmed.len() > 55 {
        return true;
    }

    // Code syntax: angle brackets, double colons, parens, dots with
    // uppercase (Go/Rust method calls), backtick-wrapped, URL paths,
    // snake_case identifiers, function calls with ().
    if trimmed.contains('<') && trimmed.contains('>') {
        return true;
    }
    if trimmed.contains("::") {
        return true;
    }
    if entity_type == "concept"
        && trimmed.contains('.')
        && trimmed
            .split('.')
            .any(|p| p.starts_with(|c: char| c.is_uppercase()))
    {
        return true;
    }
    // Backtick-wrapped names (e.g. `apply_event` method).
    if trimmed.starts_with('`') || trimmed.ends_with('`') {
        return true;
    }
    // Function-call syntax (e.g. "foo()", "self.bar()").
    if trimmed.contains("()") {
        return true;
    }
    // URL paths (e.g. "/admin/publish/:source_id").
    if trimmed.starts_with('/') {
        return true;
    }
    // Snake_case identifiers that look like code (at least two underscored
    // segments, e.g. "batch_futures", "active_resolver"). We exempt
    // well-known multi-word technical terms by requiring no spaces.
    if entity_type == "concept"
        && !trimmed.contains(' ')
        && trimmed.contains('_')
        && trimmed.matches('_').count() >= 1
        && trimmed.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return true;
    }
    // Wildcard/glob suffixes (e.g. "aeval*", "an*").
    if trimmed.ends_with('*') {
        return true;
    }

    // Math/LaTeX: braces, subscripts, Greek letters.
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return true;
    }
    if trimmed.contains('_') && trimmed.contains('^') {
        return true;
    }
    // Short math expressions: "P(x)", "f(x)", "P(A|B)", etc.
    if trimmed.len() < 10
        && trimmed.contains('(')
        && trimmed.contains(')')
        && trimmed.chars().filter(|c| c.is_alphabetic()).count() <= 4
    {
        return true;
    }
    // Unicode math symbols (excluding common ASCII).
    if trimmed
        .chars()
        .any(|c| ('\u{0370}'..='\u{03FF}').contains(&c) || ('\u{2200}'..='\u{22FF}').contains(&c))
        && trimmed.len() < 15
    {
        return true;
    }

    // Generic English words that shouldn't be entities.
    const GENERIC_WORDS: &[&str] = &[
        "alias",
        "article",
        "association",
        "auditable",
        "biology",
        "brand",
        "charge",
        "checklist",
        "children",
        "clicking",
        "collaboration",
        "compounds",
        "consequences",
        "court",
        "covariates",
        "debate",
        "diversify",
        "drugs",
        "edges",
        "generation",
        "hobbies",
        "infrastructure",
        "likes",
        "minima",
        "misaligned",
        "monotonicity",
        "nodes",
        "numeric",
        "ownership",
        "popularity",
        "possession",
        "predicate",
        "prepay",
        "purity",
        "reactants",
        "regret",
        "response",
        "reversible",
        "reward",
        "sources",
        "spiciness",
        "structural",
        "timeliness",
        "warnings",
    ];
    if entity_type == "concept" && !lower.contains(' ') && GENERIC_WORDS.contains(&lower.as_str()) {
        return true;
    }

    // Multi-word generic phrases that shouldn't be entities.
    const GENERIC_PHRASES: &[&str] = &[
        "ai use",
        "embedding operations",
        "entropy term",
        "node record",
        "node records",
        "predictive model",
        "predictive models",
        "vector space",
    ];
    if entity_type == "concept" && GENERIC_PHRASES.contains(&lower.as_str()) {
        return true;
    }

    // Document structure artifacts: "Table 8", "Figure 12", "Section 3.1",
    // "Appendix D.1", "Claim_New: ...".
    if is_document_artifact(trimmed) {
        return true;
    }

    // "X insight" pattern: LLM extracts "RAPTOR insight" or "STAR-RAG insight"
    // from spec text. The real entity is X, not "X insight".
    if lower.ends_with(" insight") || lower.ends_with(" insights") {
        return true;
    }

    // HTML/markdown artifacts: comment markers, table syntax, tildes.
    if trimmed.starts_with("<!--")
        || trimmed.starts_with("-->")
        || trimmed.starts_with("~~~")
        || trimmed.starts_with("| ")
        || trimmed.starts_with("|:")
        || trimmed.starts_with("|--")
        || trimmed == "@"
        || trimmed == "|"
    {
        return true;
    }
    // HTML entities (&#39; etc.)
    if trimmed.starts_with("&#") && trimmed.ends_with(';') {
        return true;
    }

    // Markdown links: [text](url)
    if trimmed.starts_with('[') && trimmed.contains("](") {
        return true;
    }

    // Bare prices/currency: $18, $1.80, $0.02 per million tokens
    if trimmed.starts_with('$') && trimmed[1..].starts_with(|c: char| c.is_ascii_digit()) {
        return true;
    }

    // Bare statistics/percentages: "94.8%", "55.7% F-1 Match".
    if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) && trimmed.contains('%') {
        return true;
    }

    // Bare numbers or numeric expressions: "7 * 86400", pure digits.
    if trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == ' ' || c == '*' || c == '.')
    {
        return true;
    }

    // Citation/reference fragments: "486(3-5):75–174".
    if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit())
        && (trimmed.contains('–') || trimmed.contains(':'))
        && trimmed.chars().filter(|c| c.is_ascii_digit()).count() > trimmed.len() / 2
    {
        return true;
    }

    false
}

/// Returns true if the name is a document structure reference.
fn is_document_artifact(name: &str) -> bool {
    use std::sync::LazyLock;

    static DOC_ARTIFACT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^(Table|Figure|Fig\.|Section|Appendix|Algorithm|Listing|Equation|Theorem|Lemma|Corollary|Definition|Proposition|Example|Remark)\s+(?-i)[\dA-Z]",
        )
        .unwrap()
    });

    static CLAIM_PREFIX_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)^Claim_").unwrap());

    DOC_ARTIFACT_RE.is_match(name) || CLAIM_PREFIX_RE.is_match(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_entity_paper_title() {
        assert!(is_noise_entity(
            "Retrieval-augmented generation for knowledge-intensive NLP tasks",
            "concept"
        ));
    }

    #[test]
    fn noise_entity_generic_word() {
        assert!(is_noise_entity("children", "concept"));
        assert!(is_noise_entity("clicking", "concept"));
        assert!(is_noise_entity("generation", "concept"));
        assert!(is_noise_entity("Response", "concept"));
        assert!(is_noise_entity("Sources", "concept"));
    }

    #[test]
    fn noise_entity_generic_phrase_extended() {
        assert!(is_noise_entity("predictive model", "concept"));
        assert!(is_noise_entity("predictive models", "concept"));
        assert!(is_noise_entity("node record", "concept"));
        assert!(is_noise_entity("Node Records", "concept"));
        assert!(is_noise_entity("embedding operations", "concept"));
        assert!(is_noise_entity("entropy term", "concept"));
        assert!(is_noise_entity("Article", "concept"));
        // "Article" is fine as a non-concept entity type.
        assert!(!is_noise_entity("Article", "technology"));
    }

    #[test]
    fn noise_entity_code_syntax() {
        assert!(is_noise_entity("DateTime<Utc>", "concept"));
        assert!(is_noise_entity("os.ReadFile", "concept"));
    }

    #[test]
    fn noise_entity_math_symbol() {
        assert!(is_noise_entity("{eij,dij}", "concept"));
        assert!(is_noise_entity("ω_◇^(i)", "concept"));
    }

    #[test]
    fn not_noise_real_entity() {
        assert!(!is_noise_entity("PageRank", "concept"));
        assert!(!is_noise_entity("Subjective Logic", "concept"));
        assert!(!is_noise_entity("FAISS", "technology"));
        assert!(!is_noise_entity("John Smith", "person"));
        assert!(!is_noise_entity("Google", "organization"));
    }

    #[test]
    fn not_noise_short_acronym() {
        assert!(!is_noise_entity("RRF", "concept"));
        assert!(!is_noise_entity("NLP", "concept"));
    }

    #[test]
    fn noise_entity_math_expression() {
        assert!(is_noise_entity("P(x)", "concept"));
        assert!(is_noise_entity("f(x)", "concept"));
        assert!(is_noise_entity("P(A|B)", "concept"));
    }

    #[test]
    fn noise_entity_graph_terms() {
        assert!(is_noise_entity("Nodes", "concept"));
        assert!(is_noise_entity("edges", "concept"));
        assert!(is_noise_entity("Structural", "concept"));
        assert!(is_noise_entity("biology", "concept"));
    }

    #[test]
    fn noise_entity_generic_phrase() {
        assert!(is_noise_entity("AI use", "concept"));
        assert!(is_noise_entity("vector space", "concept"));
    }

    #[test]
    fn not_noise_qualified_math() {
        // Real entities that happen to contain parens.
        assert!(!is_noise_entity("PageRank algorithm", "concept"));
        assert!(!is_noise_entity("Precision@5 (search metric)", "concept"));
    }

    #[test]
    fn noise_entity_document_artifacts() {
        assert!(is_noise_entity("Table 8", "concept"));
        assert!(is_noise_entity("Figure 12", "concept"));
        assert!(is_noise_entity("Section 3.1.6", "concept"));
        assert!(is_noise_entity("Appendix D.1", "concept"));
        assert!(is_noise_entity("Algorithm 2", "concept"));
        assert!(is_noise_entity("Theorem 1", "concept"));
        assert!(is_noise_entity(
            "Claim_New: The API uses GraphQL",
            "concept"
        ));
    }

    #[test]
    fn noise_entity_insight_pattern() {
        assert!(is_noise_entity("RAPTOR insight", "concept"));
        assert!(is_noise_entity("STAR-RAG insight", "concept"));
        assert!(is_noise_entity("Graph traversal insights", "concept"));
    }

    #[test]
    fn not_noise_real_table_or_figure() {
        // Entities that start with these words but aren't doc artifacts.
        assert!(!is_noise_entity("Table storage", "concept"));
        assert!(!is_noise_entity("Figure-ground segregation", "concept"));
    }

    #[test]
    fn noise_entity_backtick_wrapped() {
        assert!(is_noise_entity("`apply_event` method", "concept"));
        assert!(is_noise_entity("`assemble_empty` test case", "concept"));
    }

    #[test]
    fn noise_entity_function_call() {
        assert!(is_noise_entity(
            "abstention_check.should_abstain()",
            "concept"
        ));
        assert!(is_noise_entity("ast_ext.as_ref()", "concept"));
    }

    #[test]
    fn noise_entity_url_path() {
        assert!(is_noise_entity("/admin/publish/:source_id", "concept"));
    }

    #[test]
    fn noise_entity_snake_case() {
        assert!(is_noise_entity("batch_futures", "concept"));
        assert!(is_noise_entity("active_resolver", "concept"));
        assert!(is_noise_entity("alias_dim", "concept"));
    }

    #[test]
    fn noise_entity_wildcard() {
        assert!(is_noise_entity("aeval*", "concept"));
        assert!(is_noise_entity("an*", "concept"));
    }

    #[test]
    fn not_noise_snake_case_tech() {
        // Technology entities can have underscores (e.g. crate names).
        assert!(!is_noise_entity("pg_trgm", "technology"));
    }

    #[test]
    fn noise_entity_short_allowlist() {
        // Single letters are noise (math variables).
        assert!(is_noise_entity("C", "concept"));
        assert!(is_noise_entity("D", "concept"));
        assert!(is_noise_entity("E", "concept"));
        assert!(is_noise_entity("N", "concept"));
        // Two-letter non-standard abbreviations.
        assert!(is_noise_entity("CA", "concept"));
        assert!(is_noise_entity("FF", "concept"));
        assert!(is_noise_entity("GF", "concept"));
        // Allowlisted abbreviations pass through.
        assert!(!is_noise_entity("AI", "concept"));
        assert!(!is_noise_entity("ML", "concept"));
        assert!(!is_noise_entity("KG", "concept"));
        assert!(!is_noise_entity("QA", "concept"));
        assert!(!is_noise_entity("DB", "concept"));
        assert!(!is_noise_entity("IR", "concept"));
    }

    #[test]
    fn noise_entity_html_markdown_artifacts() {
        assert!(is_noise_entity("<!--", "concept"));
        assert!(is_noise_entity("-->", "concept"));
        assert!(is_noise_entity("~~~", "concept"));
        assert!(is_noise_entity("| --- | --- |", "concept"));
        assert!(is_noise_entity("|---|---|", "concept"));
        assert!(is_noise_entity("@", "concept"));
        assert!(is_noise_entity("|", "concept"));
        assert!(is_noise_entity("&#39;", "concept"));
    }

    #[test]
    fn noise_entity_markdown_links() {
        assert!(is_noise_entity(
            "[1 Introduction](https://arxiv.org/html/2506.02509v1#S1)",
            "concept"
        ));
    }

    #[test]
    fn noise_entity_prices() {
        assert!(is_noise_entity("$18", "concept"));
        assert!(is_noise_entity("$1.80", "concept"));
        assert!(is_noise_entity("$0.02 per million tokens", "concept"));
    }

    #[test]
    fn noise_entity_percentages() {
        assert!(is_noise_entity("94.8%", "concept"));
        assert!(is_noise_entity("55.7% F-1 Match", "concept"));
        assert!(is_noise_entity("57.67% EM", "concept"));
        assert!(is_noise_entity("20% memory overhead", "concept"));
    }

    #[test]
    fn noise_entity_bare_numbers() {
        assert!(is_noise_entity("7 * 86400", "concept"));
        assert!(is_noise_entity("42", "concept"));
        assert!(is_noise_entity("3.14", "concept"));
    }

    #[test]
    fn noise_entity_citation_fragment() {
        assert!(is_noise_entity("486(3-5):75\u{2013}174", "concept"));
    }

    #[test]
    fn noise_entity_embedded_newline() {
        assert!(is_noise_entity(
            "Just plain markdown\nwith no tables.",
            "other"
        ));
    }

    #[test]
    fn noise_entity_html_named_entities() {
        assert!(is_noise_entity(
            "&amp; &lt; &gt; &quot; &nbsp; &#39;",
            "concept"
        ));
    }

    #[test]
    fn noise_entity_test_constants() {
        assert!(is_noise_entity("COVALENCE_TEST_CLAMP_12345", "other"));
        assert!(is_noise_entity("MY_TEST_CONSTANT", "technology"));
        // Single underscore in all-caps with non-concept type is OK.
        assert!(!is_noise_entity("API_KEY", "technology"));
    }
}
