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

/// Build a code summary prompt by filling in template variables.
pub fn build_summary_prompt(
    entity_name: &str,
    entity_type: &str,
    file_path: &str,
    code: &str,
) -> String {
    let template = code_summary_template();
    template
        .replace("{{name}}", entity_name)
        .replace("{{type}}", entity_type)
        .replace("{{file}}", file_path)
        .replace("{{code}}", &code[..code.len().min(3000)])
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
