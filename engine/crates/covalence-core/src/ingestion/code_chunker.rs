//! Tree-sitter based code chunking for source code files.
//!
//! Parses Rust and Python source files into AST nodes and produces
//! Markdown-structured output where each top-level item (function,
//! struct, impl block, class, etc.) becomes a section. The existing
//! heading-based chunker then naturally splits at these boundaries.

use crate::error::{Error, Result};

/// Supported programming languages for tree-sitter parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeLanguage {
    /// Rust source code.
    Rust,
    /// Python source code.
    Python,
    /// Go source code.
    Go,
    /// TypeScript source code (including TSX).
    TypeScript,
    /// JavaScript source code (including JSX).
    JavaScript,
    /// Java source code.
    Java,
    /// C source code (including headers).
    C,
}

impl CodeLanguage {
    /// Detect language from a MIME type string.
    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "text/x-rust" => Some(Self::Rust),
            "text/x-python" | "text/x-script.python" => Some(Self::Python),
            "text/x-go" => Some(Self::Go),
            "text/typescript" | "application/typescript" => Some(Self::TypeScript),
            "text/javascript" | "application/javascript" => Some(Self::JavaScript),
            "text/x-java" | "text/x-java-source" => Some(Self::Java),
            "text/x-c" | "text/x-csrc" | "text/x-chdr" => Some(Self::C),
            _ => None,
        }
    }

    /// Detect language from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "go" => Some(Self::Go),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" => Some(Self::JavaScript),
            "java" => Some(Self::Java),
            "c" | "h" => Some(Self::C),
            _ => None,
        }
    }

    /// Detect language from a URI or file path.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let path = uri.split('?').next().unwrap_or(uri);
        let ext = path.rsplit('.').next()?;
        Self::from_extension(ext)
    }

    /// Fence language tag for Markdown code blocks.
    fn fence_tag(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Java => "java",
            Self::C => "c",
        }
    }

    /// Tree-sitter node kinds that represent top-level items.
    fn top_level_kinds(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &[
                "function_item",
                "struct_item",
                "enum_item",
                "impl_item",
                "trait_item",
                "type_item",
                "const_item",
                "static_item",
                "mod_item",
                "macro_definition",
                "use_declaration",
            ],
            Self::Python => &[
                "function_definition",
                "class_definition",
                "decorated_definition",
            ],
            Self::Go => &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
                "const_declaration",
                "var_declaration",
            ],
            Self::TypeScript => &[
                "function_declaration",
                "class_declaration",
                "interface_declaration",
                "type_alias_declaration",
                "enum_declaration",
                "lexical_declaration",
                "export_statement",
            ],
            Self::JavaScript => &[
                "function_declaration",
                "class_declaration",
                "lexical_declaration",
                "variable_declaration",
                "export_statement",
            ],
            Self::Java => &[
                "class_declaration",
                "interface_declaration",
                "enum_declaration",
                "annotation_type_declaration",
                "method_declaration",
            ],
            Self::C => &[
                "function_definition",
                "struct_specifier",
                "enum_specifier",
                "type_definition",
                "declaration",
                "preproc_def",
                "preproc_function_def",
            ],
        }
    }
}

/// A code item extracted from the AST.
#[derive(Debug, Clone)]
struct CodeItem {
    /// Human-readable label (e.g. "fn foo", "struct Bar", "class Baz").
    label: String,
    /// The full source text of this item.
    text: String,
    /// Methods inside compound items (impl blocks, classes).
    /// When non-empty, each method gets its own sub-section chunk.
    methods: Vec<CodeItem>,
}

/// Convert source code to Markdown structured by AST items.
///
/// Each top-level item becomes a `# ` section with the item's
/// signature as the heading and the full source text in a fenced
/// code block. Items that don't match top-level kinds (e.g. loose
/// comments, imports in Rust) are collected into a preamble section.
///
/// Returns Markdown text suitable for the standard chunking pipeline.
pub fn code_to_markdown(source: &str, lang: CodeLanguage) -> Result<String> {
    let mut parser = tree_sitter::Parser::new();

    let ts_language = match lang {
        CodeLanguage::Rust => tree_sitter_rust::LANGUAGE,
        CodeLanguage::Python => tree_sitter_python::LANGUAGE,
        CodeLanguage::Go => tree_sitter_go::LANGUAGE,
        CodeLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        CodeLanguage::JavaScript => tree_sitter_javascript::LANGUAGE,
        CodeLanguage::Java => tree_sitter_java::LANGUAGE,
        CodeLanguage::C => tree_sitter_c::LANGUAGE,
    };

    parser
        .set_language(&ts_language.into())
        .map_err(|e| Error::Ingestion(format!("tree-sitter language error: {e}")))?;

    let tree = parser
        .parse(source.as_bytes(), None)
        .ok_or_else(|| Error::Ingestion("tree-sitter parse failed".to_string()))?;

    let root = tree.root_node();
    let top_level_kinds = lang.top_level_kinds();
    let fence = lang.fence_tag();

    let mut items: Vec<CodeItem> = Vec::new();
    let mut preamble_ranges: Vec<(usize, usize)> = Vec::new();

    let cursor_node_count = root.child_count() as u32;
    for i in 0..cursor_node_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        let kind = node.kind();
        let text = &source[node.start_byte()..node.end_byte()];

        if top_level_kinds.contains(&kind) {
            let label = extract_label(source, &node, lang);
            let methods = extract_methods(source, &node, lang);
            items.push(CodeItem {
                label,
                text: text.to_string(),
                methods,
            });
        } else if kind != "line_comment"
            && kind != "block_comment"
            && kind != "comment"
            && !text.trim().is_empty()
        {
            preamble_ranges.push((node.start_byte(), node.end_byte()));
        }
    }

    let mut md = String::new();

    // Preamble section: use declarations, module-level comments, etc.
    if !preamble_ranges.is_empty() {
        let preamble_text: String = preamble_ranges
            .iter()
            .map(|(s, e)| source[*s..*e].trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        if !preamble_text.is_empty() {
            md.push_str("# Preamble\n\n");
            md.push_str(&format!("```{fence}\n{preamble_text}\n```\n\n"));
        }
    }

    // One section per top-level item. For compound items (impl blocks,
    // classes), also emit sub-sections for each method so they get
    // their own chunks and semantic summaries.
    for item in &items {
        md.push_str(&format!("# {}\n\n", item.label));
        if item.methods.is_empty() {
            md.push_str(&format!("```{fence}\n{}\n```\n\n", item.text));
        } else {
            // Emit the impl/class header as a brief code block, then
            // each method as a sub-section.
            md.push_str(&format!("```{fence}\n{}\n```\n\n", item.label));
            for method in &item.methods {
                md.push_str(&format!("## {}\n\n", method.label));
                md.push_str(&format!("```{fence}\n{}\n```\n\n", method.text));
            }
        }
    }

    if md.is_empty() {
        // Fallback: if tree-sitter found no items, wrap the whole
        // file as a single section.
        md.push_str("# Source\n\n");
        md.push_str(&format!("```{fence}\n{source}\n```\n"));
    }

    Ok(md)
}

/// Extract methods from compound AST nodes (impl blocks, classes).
///
/// For Rust impl_item nodes and Python class_definition nodes,
/// walks the body to find function definitions and extracts each
/// as a CodeItem with its own label and source text.
fn extract_methods(source: &str, node: &tree_sitter::Node, lang: CodeLanguage) -> Vec<CodeItem> {
    let (container_kind, method_kind) = match lang {
        CodeLanguage::Rust => ("impl_item", "function_item"),
        CodeLanguage::Python => ("class_definition", "function_definition"),
        // Go methods are top-level, not nested.
        CodeLanguage::Go => return Vec::new(),
        CodeLanguage::TypeScript | CodeLanguage::JavaScript => {
            ("class_declaration", "method_definition")
        }
        CodeLanguage::Java => ("class_declaration", "method_declaration"),
        // C does not have class-like containers.
        CodeLanguage::C => return Vec::new(),
    };

    if node.kind() != container_kind {
        return Vec::new();
    }

    let body_node = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return Vec::new(),
    };

    let mut methods = Vec::new();
    for i in 0..body_node.child_count() as u32 {
        let Some(child) = body_node.child(i) else {
            continue;
        };
        if child.kind() == method_kind {
            let label = extract_label(source, &child, lang);
            let text = source[child.start_byte()..child.end_byte()].to_string();
            methods.push(CodeItem {
                label,
                text,
                methods: Vec::new(),
            });
        }
    }
    methods
}

/// Extract a human-readable label for a top-level AST node.
fn extract_label(source: &str, node: &tree_sitter::Node, lang: CodeLanguage) -> String {
    match lang {
        CodeLanguage::Rust => extract_rust_label(source, node),
        CodeLanguage::Python => extract_python_label(source, node),
        CodeLanguage::Go => extract_go_label(source, node),
        CodeLanguage::TypeScript | CodeLanguage::JavaScript => extract_js_ts_label(source, node),
        CodeLanguage::Java => extract_java_label(source, node),
        CodeLanguage::C => extract_c_label(source, node),
    }
}

/// Extract a label for a Rust AST node.
///
/// For functions: `fn name(...)` or `pub fn name(...)`
/// For structs/enums/traits: `struct Name` etc.
/// For impl blocks: `impl Trait for Type` or `impl Type`
fn extract_rust_label(source: &str, node: &tree_sitter::Node) -> String {
    let kind = node.kind();
    let text = &source[node.start_byte()..node.end_byte()];

    match kind {
        "function_item" => {
            // Extract up to the opening brace.
            if let Some(pos) = text.find('{') {
                let sig = text[..pos].trim();
                // Truncate very long signatures (snap to char
                // boundary to avoid panics on non-ASCII).
                if sig.len() > 120 {
                    let mut end = 117;
                    while end > 0 && !sig.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}...", &sig[..end])
                } else {
                    sig.to_string()
                }
            } else {
                // No brace found — use first line.
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "struct_item" | "enum_item" | "trait_item" | "type_item" => {
            // Find the name child.
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = &source[name_node.start_byte()..name_node.end_byte()];
                let keyword = kind.strip_suffix("_item").unwrap_or(kind);
                format!("{keyword} {name}")
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "impl_item" => {
            // Extract the impl header up to the opening brace.
            if let Some(pos) = text.find('{') {
                let header = text[..pos].trim();
                if header.len() > 120 {
                    let mut end = 117;
                    while end > 0 && !header.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}...", &header[..end])
                } else {
                    header.to_string()
                }
            } else {
                text.lines().next().unwrap_or("impl").to_string()
            }
        }
        "mod_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = &source[name_node.start_byte()..name_node.end_byte()];
                format!("mod {name}")
            } else {
                "mod".to_string()
            }
        }
        "const_item" | "static_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = &source[name_node.start_byte()..name_node.end_byte()];
                let keyword = kind.strip_suffix("_item").unwrap_or(kind);
                format!("{keyword} {name}")
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "macro_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = &source[name_node.start_byte()..name_node.end_byte()];
                format!("macro {name}")
            } else {
                "macro".to_string()
            }
        }
        "use_declaration" => {
            // use declarations are short — use the full text.
            text.trim().to_string()
        }
        _ => text.lines().next().unwrap_or(kind).to_string(),
    }
}

/// Extract a label for a Python AST node.
fn extract_python_label(source: &str, node: &tree_sitter::Node) -> String {
    let kind = node.kind();
    let text = &source[node.start_byte()..node.end_byte()];

    match kind {
        "function_definition" | "class_definition" => {
            // First line is the signature: "def name(params):" or
            // "class Name(Base):". Strip the trailing colon.
            let first_line = text.lines().next().unwrap_or(kind).trim();
            let sig = first_line.strip_suffix(':').unwrap_or(first_line).trim();
            if sig.len() > 120 {
                let mut end = 117;
                while end > 0 && !sig.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...", &sig[..end])
            } else {
                sig.to_string()
            }
        }
        "decorated_definition" => {
            // Walk to the inner function/class definition.
            for i in 0..node.child_count() as u32 {
                let Some(child) = node.child(i) else {
                    continue;
                };
                let child_kind = child.kind();
                if child_kind == "function_definition" || child_kind == "class_definition" {
                    let inner = extract_python_label(source, &child);
                    // Prepend the decorator.
                    let decorator_text = &source[node.start_byte()..child.start_byte()];
                    let first_decorator = decorator_text
                        .lines()
                        .find(|l| l.trim().starts_with('@'))
                        .map(|l| l.trim())
                        .unwrap_or("");
                    if first_decorator.is_empty() {
                        return inner;
                    }
                    return format!("{first_decorator} {inner}");
                }
            }
            text.lines().next().unwrap_or("decorated").to_string()
        }
        _ => text.lines().next().unwrap_or(kind).to_string(),
    }
}

/// Extract a label for a Go AST node.
fn extract_go_label(source: &str, node: &tree_sitter::Node) -> String {
    let kind = node.kind();
    let text = &source[node.start_byte()..node.end_byte()];

    match kind {
        "function_declaration" | "method_declaration" => {
            // Extract up to the opening brace.
            if let Some(pos) = text.find('{') {
                let sig = text[..pos].trim();
                if sig.len() > 120 {
                    let mut end = 117;
                    while end > 0 && !sig.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!("{}...", &sig[..end])
                } else {
                    sig.to_string()
                }
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "type_declaration" => {
            // "type Foo struct { ... }" or "type Bar interface { ... }"
            // Extract the first line as the label.
            let first_line = text.lines().next().unwrap_or(kind).trim();
            if let Some(pos) = first_line.find('{') {
                first_line[..pos].trim().to_string()
            } else {
                first_line.to_string()
            }
        }
        "const_declaration" | "var_declaration" => {
            // First line: "const Foo = ..." or "var bar int"
            text.lines().next().unwrap_or(kind).trim().to_string()
        }
        _ => text.lines().next().unwrap_or(kind).to_string(),
    }
}

/// Extract a label for a TypeScript or JavaScript AST node.
fn extract_js_ts_label(source: &str, node: &tree_sitter::Node) -> String {
    let kind = node.kind();
    let text = &source[node.start_byte()..node.end_byte()];

    match kind {
        "function_declaration" => {
            if let Some(pos) = text.find('{') {
                let sig = text[..pos].trim();
                truncate_label(sig)
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "class_declaration" => {
            if let Some(pos) = text.find('{') {
                let header = text[..pos].trim();
                truncate_label(header)
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "interface_declaration" | "type_alias_declaration" | "enum_declaration" => {
            if let Some(pos) = text.find('{') {
                let header = text[..pos].trim();
                truncate_label(header)
            } else {
                text.lines().next().unwrap_or(kind).trim().to_string()
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            text.lines().next().unwrap_or(kind).trim().to_string()
        }
        "export_statement" => {
            // Unwrap the export to show the inner declaration.
            let first_line = text.lines().next().unwrap_or(kind).trim();
            if let Some(pos) = first_line.find('{') {
                truncate_label(first_line[..pos].trim())
            } else {
                truncate_label(first_line)
            }
        }
        _ => text.lines().next().unwrap_or(kind).to_string(),
    }
}

/// Extract a label for a Java AST node.
fn extract_java_label(source: &str, node: &tree_sitter::Node) -> String {
    let kind = node.kind();
    let text = &source[node.start_byte()..node.end_byte()];

    match kind {
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "annotation_type_declaration" => {
            if let Some(pos) = text.find('{') {
                let header = text[..pos].trim();
                truncate_label(header)
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "method_declaration" => {
            if let Some(pos) = text.find('{') {
                let sig = text[..pos].trim();
                truncate_label(sig)
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        _ => text.lines().next().unwrap_or(kind).to_string(),
    }
}

/// Extract a label for a C AST node.
fn extract_c_label(source: &str, node: &tree_sitter::Node) -> String {
    let kind = node.kind();
    let text = &source[node.start_byte()..node.end_byte()];

    match kind {
        "function_definition" => {
            if let Some(pos) = text.find('{') {
                let sig = text[..pos].trim();
                truncate_label(sig)
            } else {
                text.lines().next().unwrap_or(kind).to_string()
            }
        }
        "struct_specifier" | "enum_specifier" => {
            if let Some(pos) = text.find('{') {
                let header = text[..pos].trim();
                truncate_label(header)
            } else {
                text.lines().next().unwrap_or(kind).trim().to_string()
            }
        }
        "type_definition" | "declaration" | "preproc_def" | "preproc_function_def" => {
            text.lines().next().unwrap_or(kind).trim().to_string()
        }
        _ => text.lines().next().unwrap_or(kind).to_string(),
    }
}

/// Truncate a label to at most 120 characters, snapping to a char boundary.
fn truncate_label(s: &str) -> String {
    if s.len() > 120 {
        let mut end = 117;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

/// Detect a code language from a MIME type or URI.
///
/// Tries MIME first, then falls back to URI extension detection.
pub fn detect_code_language(mime: &str, uri: Option<&str>) -> Option<CodeLanguage> {
    CodeLanguage::from_mime(mime).or_else(|| uri.and_then(CodeLanguage::from_uri))
}

/// MIME types handled by the code chunker.
pub const CODE_MIME_TYPES: &[&str] = &[
    "text/x-rust",
    "text/x-python",
    "text/x-script.python",
    "text/x-go",
    "text/typescript",
    "application/typescript",
    "text/javascript",
    "application/javascript",
    "text/x-java",
    "text/x-java-source",
    "text/x-c",
    "text/x-csrc",
    "text/x-chdr",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_function_chunking() {
        let source = r#"
fn hello() {
    println!("hello");
}

fn world(x: i32) -> bool {
    x > 0
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        assert!(md.contains("# fn hello()"));
        assert!(md.contains("# fn world(x: i32) -> bool"));
        assert!(md.contains("```rust"));
    }

    #[test]
    fn rust_struct_and_impl() {
        let source = r#"
struct Foo {
    bar: i32,
}

impl Foo {
    fn new(bar: i32) -> Self {
        Self { bar }
    }
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        assert!(md.contains("# struct Foo"));
        assert!(md.contains("# impl Foo"));
    }

    #[test]
    fn rust_enum_and_trait() {
        let source = r#"
enum Color {
    Red,
    Blue,
}

trait Drawable {
    fn draw(&self);
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        assert!(md.contains("# enum Color"));
        assert!(md.contains("# trait Drawable"));
    }

    #[test]
    fn rust_use_declarations_in_preamble() {
        let source = r#"
use std::io;
use std::collections::HashMap;

fn main() {
    println!("hello");
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        // use declarations are top-level items, each gets a section
        assert!(md.contains("# use std::io;"));
        assert!(md.contains("# fn main()"));
    }

    #[test]
    fn rust_pub_function() {
        let source = r#"
pub fn public_fn(x: &str) -> String {
    x.to_string()
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        assert!(md.contains("# pub fn public_fn"));
    }

    #[test]
    fn python_function_chunking() {
        let source = r#"
def hello():
    print("hello")

def world(x: int) -> bool:
    return x > 0
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Python).unwrap();
        assert!(md.contains("# def hello()"));
        assert!(md.contains("# def world(x: int) -> bool"));
        assert!(md.contains("```python"));
    }

    #[test]
    fn python_class_definition() {
        let source = r#"
class MyClass:
    def __init__(self, value):
        self.value = value

    def get_value(self):
        return self.value
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Python).unwrap();
        assert!(md.contains("# class MyClass"));
    }

    #[test]
    fn python_decorated_function() {
        let source = r#"
@staticmethod
def helper():
    pass
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Python).unwrap();
        assert!(md.contains("@staticmethod"));
        assert!(md.contains("def helper()"));
    }

    #[test]
    fn empty_source_produces_fallback() {
        let md = code_to_markdown("", CodeLanguage::Rust).unwrap();
        assert!(md.contains("# Source"));
    }

    #[test]
    fn language_from_mime() {
        assert_eq!(
            CodeLanguage::from_mime("text/x-rust"),
            Some(CodeLanguage::Rust)
        );
        assert_eq!(
            CodeLanguage::from_mime("text/x-python"),
            Some(CodeLanguage::Python)
        );
        assert_eq!(CodeLanguage::from_mime("text/html"), None);
    }

    #[test]
    fn language_from_extension() {
        assert_eq!(CodeLanguage::from_extension("rs"), Some(CodeLanguage::Rust));
        assert_eq!(
            CodeLanguage::from_extension("py"),
            Some(CodeLanguage::Python)
        );
        assert_eq!(
            CodeLanguage::from_extension("js"),
            Some(CodeLanguage::JavaScript)
        );
        assert_eq!(CodeLanguage::from_extension("xml"), None);
    }

    #[test]
    fn language_from_uri() {
        assert_eq!(
            CodeLanguage::from_uri("file:///path/to/main.rs"),
            Some(CodeLanguage::Rust)
        );
        assert_eq!(
            CodeLanguage::from_uri("/home/user/script.py"),
            Some(CodeLanguage::Python)
        );
        assert_eq!(
            CodeLanguage::from_uri("https://example.com/page.html"),
            None
        );
    }

    #[test]
    fn detect_from_mime_or_uri() {
        // MIME takes priority
        assert_eq!(
            detect_code_language("text/x-rust", Some("file.py")),
            Some(CodeLanguage::Rust)
        );
        // Falls back to URI
        assert_eq!(
            detect_code_language("application/octet-stream", Some("lib.rs")),
            Some(CodeLanguage::Rust)
        );
        // Neither matches
        assert_eq!(detect_code_language("text/html", Some("index.html")), None);
    }

    #[test]
    fn rust_macro_definition() {
        let source = r#"
macro_rules! my_macro {
    ($x:expr) => {
        println!("{}", $x);
    };
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        assert!(md.contains("# macro my_macro"));
    }

    #[test]
    fn rust_impl_trait_for_type() {
        let source = r#"
impl Display for Foo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Foo")
    }
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Rust).unwrap();
        assert!(md.contains("# impl Display for Foo"));
    }

    #[test]
    fn extract_rust_label_long_unicode_signature_no_panic() {
        // A function signature > 120 bytes with multi-byte chars at
        // the truncation boundary must not panic.
        let sig_body = "ü".repeat(60); // 60 × 2 bytes = 120 bytes
        let source = format!("fn {sig_body}() {{\n}}\n");

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&source, None).unwrap();
        let root = tree.root_node();
        let func_node = root.child(0).unwrap();

        // Should not panic on multi-byte boundary.
        let label = extract_rust_label(&source, &func_node);
        assert!(label.ends_with("..."), "got: {label}");
    }

    #[test]
    fn extract_python_label_long_unicode_no_panic() {
        let name = "é".repeat(70); // 70 × 2 bytes = 140 bytes
        let source = format!("def {name}():\n    pass\n");

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&source, None).unwrap();
        let root = tree.root_node();
        let func_node = root.child(0).unwrap();

        let label = extract_python_label(&source, &func_node);
        assert!(label.ends_with("..."), "got: {label}");
    }

    #[test]
    fn go_function_chunking() {
        let source = r#"package main

func Hello() {
    fmt.Println("hello")
}

func Add(a, b int) int {
    return a + b
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Go).unwrap();
        assert!(md.contains("# func Hello()"), "got: {md}");
        assert!(md.contains("# func Add(a, b int) int"), "got: {md}");
        assert!(md.contains("```go"));
    }

    #[test]
    fn go_struct_chunking() {
        let source = r#"package main

type Server struct {
    Host string
    Port int
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Go).unwrap();
        assert!(md.contains("# type Server struct"), "got: {md}");
    }

    #[test]
    fn go_interface_chunking() {
        let source = r#"package main

type Reader interface {
    Read(p []byte) (n int, err error)
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Go).unwrap();
        assert!(md.contains("# type Reader interface"), "got: {md}");
    }

    #[test]
    fn go_method_chunking() {
        let source = r#"package main

func (s *Server) Start() error {
    return nil
}
"#;
        let md = code_to_markdown(source.trim(), CodeLanguage::Go).unwrap();
        assert!(md.contains("# func (s *Server) Start() error"), "got: {md}");
    }

    #[test]
    fn go_language_detection() {
        assert_eq!(CodeLanguage::from_extension("go"), Some(CodeLanguage::Go));
        assert_eq!(CodeLanguage::from_mime("text/x-go"), Some(CodeLanguage::Go));
        assert_eq!(
            CodeLanguage::from_uri("cli/cmd/root.go"),
            Some(CodeLanguage::Go)
        );
    }
}
