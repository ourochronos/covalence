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

    // Strip surrounding quotes before length check — the LLM
    // extractor sometimes wraps entities in quotes: `"_"`, `'@'`.
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed);

    // Entirely non-alphanumeric names (punctuation, symbols, whitespace).
    if !unquoted.is_empty() && !unquoted.chars().any(|c| c.is_alphanumeric()) {
        return true;
    }

    // HTML tag fragments: `<h3`, `<div`, `<span` — LLM extracts
    // these from HTML content.
    if unquoted.starts_with('<') && unquoted[1..].starts_with(|c: char| c.is_ascii_alphabetic()) {
        return true;
    }

    // Very short names (1-2 chars by char count) are noise unless
    // they're well-known abbreviations. Single letters like "C", "D",
    // "E" are mathematical variables from papers, not real entities.
    let char_count = unquoted.chars().count();
    if char_count <= 2 {
        const SHORT_ALLOWLIST: &[&str] = &[
            "AI", "DB", "GI", "IE", "IR", "IT", "KG", "ML", "NE", "NL", "NLP", "QA", "RL", "UI",
            "UX",
        ];
        if !SHORT_ALLOWLIST.contains(&unquoted) {
            return true;
        }
    }

    // Paper titles: concept entities with >55 chars are almost always
    // paper titles (real concepts are shorter).
    if entity_type == "concept" && trimmed.len() > 55 {
        return true;
    }
    // Subtitle-style titles: "Topic: A Detailed Guide for YYYY".
    // Requires colon separator, total >35 chars, and a 4-digit
    // number that looks like a year (19xx or 20xx). Excludes ADR
    // references which use "ADR-NNNN:" format.
    if entity_type == "concept"
        && trimmed.contains(": ")
        && trimmed.len() > 35
        && !trimmed.starts_with("ADR-")
        && trimmed.as_bytes().windows(4).any(|w| {
            w.iter().all(|b| b.is_ascii_digit())
                && (w[0] == b'1' && w[1] == b'9' || w[0] == b'2' && w[1] == b'0')
        })
    {
        return true;
    }

    // Code syntax: angle brackets, double colons, parens, dots with
    // uppercase (Go/Rust method calls), backtick-wrapped, URL paths,
    // snake_case identifiers, function calls with ().
    // NOTE: use `unquoted` for syntactic checks so that quoted entities
    // like `"web_page"` are correctly detected after quote stripping.
    if unquoted.contains('<') && unquoted.contains('>') {
        return true;
    }
    if unquoted.contains("::") {
        return true;
    }
    if entity_type == "concept"
        && unquoted.contains('.')
        && unquoted
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
    if unquoted.contains("()") {
        return true;
    }
    // URL paths (e.g. "/admin/publish/:source_id").
    if unquoted.starts_with('/') {
        return true;
    }
    // Snake_case identifiers that look like code (at least two underscored
    // segments, e.g. "batch_futures", "active_resolver"). We exempt
    // well-known multi-word technical terms by requiring no spaces.
    if entity_type == "concept"
        && !unquoted.contains(' ')
        && unquoted.contains('_')
        && unquoted.matches('_').count() >= 1
        && unquoted.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return true;
    }
    // Wildcard/glob suffixes (e.g. "aeval*", "an*").
    if unquoted.ends_with('*') {
        return true;
    }

    // Rust/Go primitive types as entities: f64, i64, u8, &str, etc.
    // These leak from code examples in papers and specs.
    const PRIMITIVE_TYPES: &[&str] = &[
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
        "u16", "u32", "u64", "u128", "usize", "&str", "&mut",
    ];
    if PRIMITIVE_TYPES.contains(&unquoted) {
        return true;
    }

    // Markdown italic (`_text_`) or bold (`**text**`) wrapping.
    if unquoted.starts_with('_') && unquoted.ends_with('_') && unquoted.len() > 2 {
        return true;
    }
    if unquoted.starts_with("**") && unquoted.ends_with("**") && unquoted.len() > 4 {
        return true;
    }

    // Ampersand-prefixed type references (`&str`, `&[u8]`, `&self`).
    if unquoted.starts_with('&')
        && unquoted.len() > 1
        && unquoted[1..].starts_with(|c: char| c.is_ascii_lowercase())
    {
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
        "assumptions",
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
        "developers",
        "dimensions",
        "disturbances",
        "diversify",
        "drugs",
        "edges",
        "false",
        "federated",
        "generation",
        "hobbies",
        "hosting",
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
        "skipped",
        "sources",
        "spiciness",
        "structural",
        "timeliness",
        "timestamp",
        "true",
        "warnings",
    ];
    if entity_type == "concept" && !lower.contains(' ') && GENERIC_WORDS.contains(&lower.as_str()) {
        return true;
    }
    // "true" and "false" as `other` type are also noise.
    if (lower == "true" || lower == "false") && entity_type == "other" {
        return true;
    }

    // Multi-word generic phrases that shouldn't be entities.
    const GENERIC_PHRASES: &[&str] = &[
        "ai use",
        "choice success",
        "code constants",
        "color attributes",
        "embedding operations",
        "end users",
        "entropy term",
        "explanatory power",
        "full extraction",
        "functionality implementation",
        "generative quality",
        "global operations",
        "model upgrade",
        "node record",
        "node records",
        "overall quality",
        "predictive model",
        "predictive models",
        "retrieve token",
        "unordered lists",
        "vector space",
        "web processes",
    ];
    if entity_type == "concept" && GENERIC_PHRASES.contains(&lower.as_str()) {
        return true;
    }

    // Document structure artifacts: "Table 8", "Figure 12", "Section 3.1",
    // "Appendix D.1", "Claim_New: ...".
    if is_document_artifact(trimmed) {
        return true;
    }

    // Questions extracted as entities: "what currency needed in
    // scotland", "how does X work". Check concept entities starting
    // with question words.
    if entity_type == "concept" {
        const QUESTION_WORDS: &[&str] = &[
            "what ", "how ", "why ", "when ", "where ", "who ", "which ", "is ", "does ", "can ",
            "should ", "could ", "would ",
        ];
        if QUESTION_WORDS.iter().any(|q| lower.starts_with(q)) {
            return true;
        }
    }

    // "Key: value" metadata patterns: "Key: version", "Source: url".
    if entity_type == "concept" && trimmed.contains(": ") && trimmed.len() < 30 {
        let colon_pos = trimmed.find(": ").unwrap();
        let key = &trimmed[..colon_pos];
        // Only flag if the key part is a single capitalized word
        if !key.contains(' ') && key.starts_with(|c: char| c.is_uppercase()) {
            return true;
        }
    }

    // Generic reference placeholders: "Source A", "Source B",
    // "Entity X", "Node 1".
    if entity_type == "other" || entity_type == "concept" {
        const REF_PREFIXES: &[&str] = &["source ", "entity ", "node "];
        if REF_PREFIXES.iter().any(|p| lower.starts_with(p))
            && trimmed.len() < 15
            && trimmed
                .split_whitespace()
                .last()
                .is_some_and(|w| w.len() <= 2)
        {
            return true;
        }
    }

    // ArXiv category labels: "physics.data-an", "cs.CL", "math.AG".
    // Pattern: lowercase.lowercase-with-optional-dash, max ~20 chars.
    if entity_type == "concept"
        && trimmed.len() < 25
        && trimmed.contains('.')
        && !trimmed.contains(' ')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && trimmed
            .split('.')
            .all(|p| p.starts_with(|c: char| c.is_ascii_lowercase()))
    {
        return true;
    }

    // Math equations with equals sign: "Γ_2(θ_2) = X ⊗ W β".
    if trimmed.contains(" = ") && trimmed.len() < 40 {
        let alpha_count = trimmed.chars().filter(|c| c.is_alphabetic()).count();
        let total_count = trimmed.chars().filter(|c| !c.is_whitespace()).count();
        if alpha_count < total_count / 2 {
            return true;
        }
    }

    // Ordinal date fragments: "1st April", "2nd March".
    if entity_type == "event" && trimmed.len() < 20 && trimmed.split_whitespace().count() == 2 {
        let first = trimmed.split_whitespace().next().unwrap_or("");
        if first.ends_with("st")
            || first.ends_with("nd")
            || first.ends_with("rd")
            || first.ends_with("th")
        {
            let num_part = first.trim_end_matches(|c: char| c.is_alphabetic());
            if num_part.chars().all(|c| c.is_ascii_digit()) && !num_part.is_empty() {
                return true;
            }
        }
    }

    // "X insight" pattern: LLM extracts "RAPTOR insight" or "STAR-RAG insight"
    // from spec text. The real entity is X, not "X insight".
    if lower.ends_with(" insight") || lower.ends_with(" insights") {
        return true;
    }

    // HTML/markdown artifacts: comment markers, table syntax, tildes.
    if trimmed.starts_with("<!--")
        || trimmed.starts_with("<!")
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

    // File paths: `file://...`, `/path/to/file`.
    if trimmed.starts_with("file://") {
        return true;
    }

    // Array/dict indexing or template syntax:
    // `items[i]`, `json["key"]`, `[{parent.level}: ...]`.
    if trimmed.contains('[') && trimmed.contains(']') {
        return true;
    }

    // Breadcrumb navigation or trailing `>`: `Section > Subsection`,
    // `next >`, `cite as:`.
    if trimmed.contains(" > ") || trimmed.ends_with(" >") {
        return true;
    }

    // Quote-wrapped text: the LLM sometimes wraps example text
    // from the source in quotes and extracts it as an entity.
    // Catch both sentences (`"He went to the store."`) and shorter
    // quoted phrases (`"hello world"`, `"for example"`).
    if unquoted != trimmed && unquoted.contains(' ') {
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

    #[test]
    fn noise_entity_quote_wrapped_punctuation() {
        // LLM extracts quoted punctuation from source text.
        assert!(is_noise_entity("\"_\"", "concept"));
        assert!(is_noise_entity("'@'", "other"));
        assert!(is_noise_entity("'|'", "other"));
        assert!(is_noise_entity("':'", "other"));
        assert!(is_noise_entity("'-'", "other"));
        assert!(is_noise_entity("' '", "other"));
    }

    #[test]
    fn noise_entity_pure_punctuation() {
        assert!(is_noise_entity("@", "other"));
        assert!(is_noise_entity("|", "other"));
        assert!(is_noise_entity("-", "other"));
        assert!(is_noise_entity("...", "concept"));
    }

    #[test]
    fn noise_entity_html_tag_fragments() {
        assert!(is_noise_entity("<h3", "concept"));
        assert!(is_noise_entity("<div", "other"));
        assert!(is_noise_entity("<span", "concept"));
        // Paired tags are caught by the existing < + > filter.
        assert!(is_noise_entity("<h3>Title</h3>", "concept"));
    }

    #[test]
    fn noise_entity_quoted_short_names() {
        // Quote-wrapped short names: the inner content is too short.
        assert!(is_noise_entity("\"x\"", "concept"));
        assert!(is_noise_entity("'a'", "concept"));
        // But real abbreviations inside quotes should be kept.
        assert!(!is_noise_entity("\"AI\"", "concept"));
    }

    #[test]
    fn noise_entity_file_paths() {
        assert!(is_noise_entity("file://spec/01-architecture.md", "other"));
        assert!(is_noise_entity("file://cli/cmd/search.go", "other"));
    }

    #[test]
    fn noise_entity_array_indexing() {
        assert!(is_noise_entity("items[i].score", "concept"));
        assert!(is_noise_entity(
            "json[\"response_format\"][\"type\"]",
            "concept"
        ));
        assert!(is_noise_entity("[{parent.level}: {parent.ctx}]", "other"));
        // But markdown links [text](url) are caught by the markdown
        // link filter, not this one.
    }

    #[test]
    fn noise_entity_breadcrumb_nav() {
        assert!(is_noise_entity("Introduction > Methods", "other"));
        assert!(is_noise_entity("next >", "concept"));
        // Real entities with ">" in a different context should not
        // be caught.
        assert!(!is_noise_entity("greater than operator", "concept"));
    }

    #[test]
    fn noise_entity_quoted_multi_word() {
        assert!(is_noise_entity("\"He went to the store.\"", "other"));
        assert!(is_noise_entity("\"You are helpful.\"", "other"));
        // Multi-word quoted text without punctuation is also noise.
        assert!(is_noise_entity("\"hello world\"", "concept"));
        assert!(is_noise_entity("\"for example\"", "concept"));
        assert!(is_noise_entity("\"Source type: document\"", "concept"));
        // Single-word quoted text is fine (handled by other rules).
        assert!(!is_noise_entity("\"knowledge_graph\"", "technology"));
    }

    #[test]
    fn noise_entity_doctype() {
        assert!(is_noise_entity("<!doctype", "concept"));
        assert!(is_noise_entity("<!DOCTYPE html>", "concept"));
    }

    #[test]
    fn noise_entity_rust_primitives() {
        assert!(is_noise_entity("f64", "concept"));
        assert!(is_noise_entity("i64", "concept"));
        assert!(is_noise_entity("u8", "concept"));
        assert!(is_noise_entity("usize", "concept"));
        assert!(is_noise_entity("bool", "concept"));
        assert!(is_noise_entity("&str", "concept"));
        assert!(is_noise_entity("&mut", "concept"));
        // But legitimate entities with these as substrings are fine.
        assert!(!is_noise_entity("f64 precision arithmetic", "concept"));
    }

    #[test]
    fn noise_entity_markdown_italic_bold() {
        assert!(is_noise_entity("_The Book of Why_", "concept"));
        assert!(is_noise_entity("_emphasis text_", "concept"));
        assert!(is_noise_entity("**bold text**", "concept"));
        // Single underscore is already caught by short-name filter.
        // Real entities with underscores in the middle are fine.
        assert!(!is_noise_entity("pg_trgm", "technology"));
    }

    #[test]
    fn noise_entity_ampersand_refs() {
        assert!(is_noise_entity("&self", "concept"));
        assert!(is_noise_entity("&str", "concept"));
        // But &amp; is caught by HTML entity filter.
        assert!(is_noise_entity("&amp;", "concept"));
    }

    #[test]
    fn noise_entity_quoted_snake_case() {
        // Quoted snake_case identifiers — the unquoting + snake_case
        // filter now catches these.
        assert!(is_noise_entity("\"web_page\"", "concept"));
        assert!(is_noise_entity("\"tool_output\"", "concept"));
    }

    #[test]
    fn noise_entity_expanded_generic_words() {
        assert!(is_noise_entity("dimensions", "concept"));
        assert!(is_noise_entity("developers", "concept"));
        assert!(is_noise_entity("timestamp", "concept"));
        assert!(is_noise_entity("disturbances", "concept"));
        assert!(is_noise_entity("skipped", "concept"));
        assert!(is_noise_entity("federated", "concept"));
        assert!(is_noise_entity("hosting", "concept"));
        assert!(is_noise_entity("true", "concept"));
        assert!(is_noise_entity("false", "concept"));
        assert!(is_noise_entity("true", "other"));
        // "federated learning" is NOT generic (multi-word, specific).
        assert!(!is_noise_entity("federated learning", "concept"));
    }

    #[test]
    fn noise_entity_expanded_generic_phrases() {
        assert!(is_noise_entity("color attributes", "concept"));
        assert!(is_noise_entity("global operations", "concept"));
        assert!(is_noise_entity("Overall quality", "concept"));
        assert!(is_noise_entity("generative quality", "concept"));
        assert!(is_noise_entity("full extraction", "concept"));
        assert!(is_noise_entity("end users", "concept"));
        assert!(is_noise_entity("code constants", "concept"));
        assert!(is_noise_entity("web processes", "concept"));
        assert!(is_noise_entity("retrieve token", "concept"));
        assert!(is_noise_entity("model upgrade", "concept"));
        assert!(is_noise_entity("choice success", "concept"));
        assert!(is_noise_entity("functionality implementation", "concept"));
        assert!(is_noise_entity("unordered lists", "concept"));
        assert!(is_noise_entity("explanatory power", "concept"));
    }

    #[test]
    fn noise_entity_questions() {
        assert!(is_noise_entity(
            "what currency needed in scotland",
            "concept"
        ));
        assert!(is_noise_entity("how does chunking work", "concept"));
        assert!(is_noise_entity("is this relevant", "concept"));
        // But real entities starting with these words should not match.
        assert!(!is_noise_entity("WHO", "organization"));
    }

    #[test]
    fn noise_entity_key_value_metadata() {
        assert!(is_noise_entity("Key: version", "concept"));
        assert!(is_noise_entity("Source: url", "concept"));
        // But real concepts with colons are fine.
        assert!(!is_noise_entity(
            "Matryoshka Representation Learning",
            "concept"
        ));
    }

    #[test]
    fn noise_entity_generic_references() {
        assert!(is_noise_entity("Source A", "other"));
        assert!(is_noise_entity("Source B", "concept"));
        assert!(is_noise_entity("Entity X", "concept"));
        // But specific sources are fine.
        assert!(!is_noise_entity("Source Code Analysis", "concept"));
    }

    #[test]
    fn noise_entity_arxiv_categories() {
        assert!(is_noise_entity("physics.data-an", "concept"));
        assert!(is_noise_entity("cs.cl", "concept"));
        assert!(is_noise_entity("math.ag", "concept"));
        // But real entities with dots are fine.
        assert!(!is_noise_entity("Node2Vec", "technology"));
    }

    #[test]
    fn noise_entity_math_equations() {
        assert!(is_noise_entity(
            "\u{0393}_2(\u{03B8}_2) = X \u{2297} W \u{03B2}",
            "concept"
        ));
        // Short enough and has equals sign with mostly non-alpha chars.
    }

    #[test]
    fn noise_entity_ordinal_dates() {
        assert!(is_noise_entity("1st April", "event"));
        assert!(is_noise_entity("2nd March", "event"));
        assert!(is_noise_entity("25th December", "event"));
        // But real events should not match.
        assert!(!is_noise_entity("NeurIPS 2024", "event"));
    }

    #[test]
    fn noise_entity_subtitle_titles() {
        // Caught by subtitle-year: colon + >35 chars + 19xx/20xx year.
        assert!(is_noise_entity(
            "Roi Align: A Comprehensive Guide for 2025",
            "concept"
        ));
        assert!(is_noise_entity(
            "Health Service Research Methodology: Trends Since 2019",
            "concept"
        ));
        // Short or without year is fine.
        assert!(!is_noise_entity("Rate Limiting", "concept"));
        // ADR references with colon + year are allowed.
        assert!(!is_noise_entity(
            "ADR-0009: Three-Timescale Consolidation Pipeline",
            "concept"
        ));
    }

    #[test]
    fn noise_entity_long_concept_titles() {
        // >55 chars — almost always paper titles, not real concepts.
        assert!(is_noise_entity(
            "A Survey of Graph Neural Networks for Knowledge Graph Completion Tasks",
            "concept"
        ));
        // Real concepts under 55 chars are fine.
        assert!(!is_noise_entity(
            "emergent ontology with embedding-based normalization",
            "concept"
        ));
        assert!(!is_noise_entity("Reciprocal Rank Fusion", "concept"));
    }
}
