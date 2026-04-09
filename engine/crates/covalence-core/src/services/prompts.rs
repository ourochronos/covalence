#![allow(dead_code)] // Templates are public API; not all wired yet.
//! Shared LLM prompt templates for code analysis and knowledge extraction.
//!
//! Prompts are loaded from `engine/prompts/*.md` at runtime. If the file
//! is missing, a compiled-in fallback is used. This allows prompt iteration
//! without recompiling or reingesting code.
//!
//! Template variables use `{{name}}` syntax and are replaced at call time.

use std::sync::OnceLock;

/// Current prompt version for semantic code summaries.
///
/// Increment this when the prompt changes to enable selective
/// reprocessing of entities summarized with older versions.
pub const SUMMARY_PROMPT_VERSION: i32 = 3;

/// Directory where prompt template files are stored.
/// Relative to the working directory (engine/).
const PROMPTS_DIR: &str = "prompts";

/// Load a prompt template from disk, falling back to compiled-in default.
fn load_prompt(filename: &str, fallback: &str) -> String {
    let path = std::path::Path::new(PROMPTS_DIR).join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            tracing::debug!(path = %path.display(), "loaded prompt from file");
            content
        }
        Err(_) => {
            // Also try from the repo root (for prod where cwd may differ).
            let alt_paths = [
                format!("engine/{PROMPTS_DIR}/{filename}"),
                format!("/home/covalence/covalence/engine/{PROMPTS_DIR}/{filename}"),
            ];
            for alt in &alt_paths {
                if let Ok(content) = std::fs::read_to_string(alt) {
                    tracing::debug!(path = alt, "loaded prompt from alternate path");
                    return content;
                }
            }
            tracing::debug!(filename, "using compiled-in fallback prompt");
            fallback.to_string()
        }
    }
}

/// Cached prompt templates loaded once at first use.
static CODE_SUMMARY_PROMPT: OnceLock<String> = OnceLock::new();
static ENTITY_EXTRACTION_PROMPT: OnceLock<String> = OnceLock::new();
static RELATIONSHIP_EXTRACTION_PROMPT: OnceLock<String> = OnceLock::new();
static SECTION_COMPILATION_PROMPT: OnceLock<String> = OnceLock::new();
static SOURCE_SUMMARY_PROMPT: OnceLock<String> = OnceLock::new();

/// Get the code summary prompt template.
pub fn code_summary_template() -> &'static str {
    CODE_SUMMARY_PROMPT.get_or_init(|| {
        load_prompt(
            "code_summary.md",
            include_str!("../../../../prompts/code_summary.md"),
        )
    })
}

/// Get the entity extraction prompt template.
pub fn entity_extraction_template() -> &'static str {
    ENTITY_EXTRACTION_PROMPT.get_or_init(|| {
        load_prompt(
            "entity_extraction.md",
            include_str!("../../../../prompts/entity_extraction.md"),
        )
    })
}

/// Get the relationship extraction prompt template.
pub fn relationship_extraction_template() -> &'static str {
    RELATIONSHIP_EXTRACTION_PROMPT.get_or_init(|| {
        load_prompt(
            "relationship_extraction.md",
            include_str!("../../../../prompts/relationship_extraction.md"),
        )
    })
}

/// Get the section compilation prompt template.
pub fn section_compilation_template() -> &'static str {
    SECTION_COMPILATION_PROMPT.get_or_init(|| {
        load_prompt(
            "section_compilation.md",
            include_str!("../../../../prompts/section_compilation.md"),
        )
    })
}

/// Get the source summary prompt template.
pub fn source_summary_template() -> &'static str {
    SOURCE_SUMMARY_PROMPT.get_or_init(|| {
        load_prompt(
            "source_summary.md",
            include_str!("../../../../prompts/source_summary.md"),
        )
    })
}

/// Wrap user-supplied content in XML-style tags to separate it from
/// prompt instructions and mitigate injection attacks.
///
/// Any instructions that appear inside the wrapped content will be
/// treated as data, not commands, because the system prompt explicitly
/// tells the LLM to ignore instructions within these tags.
pub fn wrap_input(tag: &str, content: &str) -> String {
    format!("<{tag}>\n{content}\n</{tag}>")
}

/// Maximum code bytes to include in the summary prompt.
///
/// Large AST chunks (e.g., an entire impl block with many methods)
/// can blow past a model's context window. We truncate to keep the
/// prompt within budget while preserving syntactic coherence.
const MAX_CODE_BYTES: usize = 3000;

/// Truncate a code string at a line boundary so we never cut in
/// the middle of a token or multi-byte UTF-8 character.
///
/// Returns the full string if it's already within the limit.
fn truncate_code(code: &str, max_bytes: usize) -> &str {
    if code.len() <= max_bytes {
        return code;
    }
    // First, snap to a valid char boundary so we can safely search
    // for newlines. This prevents panics on multi-byte UTF-8.
    let mut safe_end = max_bytes;
    while safe_end > 0 && !code.is_char_boundary(safe_end) {
        safe_end -= 1;
    }
    // Find the last newline before the safe limit. This keeps
    // every included line syntactically complete, which reduces
    // LLM hallucinations from broken code fragments.
    match code[..safe_end].rfind('\n') {
        Some(pos) => &code[..pos],
        // No newline found (single very-long line): return the
        // char-boundary-safe slice.
        None => &code[..safe_end],
    }
}

/// Build a code summary prompt by filling in template variables.
///
/// Returns `(system_prompt, user_prompt)`. The template's
/// instruction text ("You are a code analysis engine...") becomes
/// the system prompt, while the entity metadata and code go into
/// the user prompt. This matches the role-separation pattern used
/// by the entity extraction pipeline and improves instruction-
/// following from the LLM.
pub fn build_summary_prompt(
    entity_name: &str,
    entity_type: &str,
    file_path: &str,
    code: &str,
) -> (String, String) {
    let template = code_summary_template();
    let code = truncate_code(code, MAX_CODE_BYTES);

    // Split at the <entity> tag: everything before is the system
    // prompt (instructions), everything from <entity> onward is
    // the user prompt (data).
    let (system, user_template) = match template.find("<entity>") {
        Some(pos) => (template[..pos].trim(), &template[pos..]),
        None => (template, ""),
    };

    let user = user_template
        .replace("{{name}}", entity_name)
        .replace("{{type}}", entity_type)
        .replace("{{file}}", file_path)
        .replace("{{code}}", code);

    (system.to_string(), user)
}

/// Wrap document text for entity extraction.
pub fn wrap_document(text: &str) -> String {
    wrap_input("document", text)
}

/// Wrap statement list for section compilation.
pub fn wrap_statements(text: &str) -> String {
    wrap_input("statements", text)
}

/// Wrap section list for source summary.
pub fn wrap_sections(text: &str) -> String {
    wrap_input("sections", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_code_short_string_unchanged() {
        let code = "fn main() {}";
        assert_eq!(truncate_code(code, 3000), code);
    }

    #[test]
    fn truncate_code_at_line_boundary() {
        let code = "line1\nline2\nline3\nline4\n";
        // Limit that falls in the middle of "line3"
        let result = truncate_code(code, 14);
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn truncate_code_utf8_safe() {
        // "é" is 2 bytes — a limit of 5 falls in the middle
        let code = "abc\néfg";
        let result = truncate_code(code, 5);
        // Should cut at the newline before "é"
        assert_eq!(result, "abc");
    }

    #[test]
    fn build_summary_prompt_returns_system_user_pair() {
        let (system, user) = build_summary_prompt("Foo", "struct", "src/foo.rs", "struct Foo {}");
        assert!(
            system.contains("code analysis engine"),
            "system prompt should have instructions"
        );
        assert!(
            user.contains("<entity>"),
            "user prompt should have entity tag"
        );
        assert!(
            user.contains("struct Foo {}"),
            "user prompt should have code"
        );
        assert!(
            !system.contains("<entity>"),
            "system prompt should NOT have entity data"
        );
    }
}
