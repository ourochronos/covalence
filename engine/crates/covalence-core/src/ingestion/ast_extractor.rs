//! Deterministic AST-based entity extraction for source code.
//!
//! Walks a tree-sitter AST to extract structured entities and
//! relationships from Rust and Python source code. Unlike the LLM
//! extractor, all extractions are deterministic with confidence 1.0.
//!
//! Design principle: struct/class fields become metadata properties
//! on their parent entity, NOT separate graph nodes.

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::ingestion::code_chunker::CodeLanguage;
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionContext, ExtractionResult, Extractor,
};

/// Compute a SHA-256 hash of a tree-sitter node's source text.
///
/// Used to fingerprint individual AST items (functions, structs, etc.)
/// so that incremental re-ingestion can skip unchanged code entities.
/// The hash is stored in `ExtractedEntity.metadata.ast_hash` and
/// persisted on graph nodes in `properties.ast_hash`.
fn compute_ast_hash(source: &str, node: &tree_sitter::Node) -> String {
    let text = &source[node.start_byte()..node.end_byte()];
    let hash = Sha256::digest(text.as_bytes());
    format!("{hash:x}")
}

/// Build the metadata JSON carrying an AST hash for a code entity.
fn ast_metadata(source: &str, node: &tree_sitter::Node) -> Option<serde_json::Value> {
    Some(serde_json::json!({ "ast_hash": compute_ast_hash(source, node) }))
}

/// Deterministic extractor that walks tree-sitter ASTs to extract
/// code entities and relationships.
///
/// Produces entities for structs, enums, traits, functions, impl
/// blocks, modules, constants, macros (Rust) and classes, functions
/// (Python). Relationships include `implements`, `extends`,
/// `imports`, `calls`, and `contains`.
///
/// All extractions have confidence 1.0 since they are derived from
/// deterministic AST parsing rather than probabilistic LLM output.
pub struct AstExtractor;

impl AstExtractor {
    /// Create a new AST extractor.
    pub fn new() -> Self {
        Self
    }

    /// Extract entities and relationships from source code.
    ///
    /// Detects the language from the extraction context (source URI
    /// or source type) and delegates to the appropriate
    /// language-specific extractor.
    fn extract_code(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        let lang = self.detect_language(context);
        let lang = match lang {
            Some(l) => l,
            None => return Ok(ExtractionResult::default()),
        };

        let mut parser = tree_sitter::Parser::new();
        let ts_language = match lang {
            CodeLanguage::Rust => tree_sitter_rust::LANGUAGE,
            CodeLanguage::Python => tree_sitter_python::LANGUAGE,
            CodeLanguage::Go => tree_sitter_go::LANGUAGE,
        };
        parser
            .set_language(&ts_language.into())
            .map_err(|e| Error::Ingestion(format!("tree-sitter language error: {e}")))?;

        // The input may be Markdown-wrapped code from the code
        // chunker. Extract raw code from fenced blocks before
        // parsing.
        let raw_code = unwrap_markdown_code(text);

        let tree = parser
            .parse(raw_code.as_bytes(), None)
            .ok_or_else(|| Error::Ingestion("tree-sitter parse failed".into()))?;

        match lang {
            CodeLanguage::Rust => extract_rust(&raw_code, &tree),
            CodeLanguage::Python => extract_python(&raw_code, &tree),
            CodeLanguage::Go => extract_go(&raw_code, &tree),
        }
    }

    /// Detect the code language from extraction context.
    fn detect_language(&self, context: &ExtractionContext) -> Option<CodeLanguage> {
        // Try URI-based detection first.
        if let Some(ref uri) = context.source_uri {
            if let Some(lang) = CodeLanguage::from_uri(uri) {
                return Some(lang);
            }
        }
        // Try source_type as a MIME type.
        if let Some(ref st) = context.source_type {
            if let Some(lang) = CodeLanguage::from_mime(st) {
                return Some(lang);
            }
        }
        None
    }
}

impl Default for AstExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Extractor for AstExtractor {
    async fn extract(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        self.extract_code(text, context)
    }
}

/// Extract raw code from Markdown-fenced code blocks.
///
/// The code chunker wraps source in Markdown sections with fenced
/// blocks. This function strips those wrappers to recover the
/// original source for AST parsing.
fn unwrap_markdown_code(text: &str) -> String {
    let mut code_parts: Vec<&str> = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            code_parts.push(line);
        }
    }

    if code_parts.is_empty() {
        // No fenced blocks found — treat the whole input as code.
        text.to_string()
    } else {
        code_parts.join("\n")
    }
}

// ── Rust extraction ─────────────────────────────────────────────

/// Extract entities and relationships from a Rust AST.
fn extract_rust(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
    let root = tree.root_node();
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut relationships: Vec<ExtractedRelationship> = Vec::new();

    let child_count = root.child_count() as u32;
    for i in 0..child_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        extract_rust_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level Rust AST node.
fn extract_rust_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "struct_item" => extract_rust_struct(source, node, entities),
        "enum_item" => extract_rust_enum(source, node, entities),
        "trait_item" => extract_rust_trait(source, node, entities),
        "function_item" => {
            extract_rust_function(source, node, entities);
        }
        "impl_item" => {
            extract_rust_impl(source, node, entities, relationships);
        }
        "mod_item" => extract_rust_mod(source, node, entities),
        "const_item" | "static_item" => {
            extract_rust_constant(source, node, entities);
        }
        "macro_definition" => {
            extract_rust_macro(source, node, entities);
        }
        "use_declaration" => {
            extract_rust_use(source, node, relationships);
        }
        _ => {}
    }
}

/// Extract a Rust struct entity with fields as metadata.
fn extract_rust_struct(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let visibility = detect_visibility(source, node);
    let fields = extract_rust_fields(source, node);

    let description = if !fields.is_empty() {
        let field_summary: Vec<String> = fields
            .iter()
            .filter_map(|f| {
                let n = f.get("name")?.as_str()?;
                let t = f.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                Some(format!("{n}: {t}"))
            })
            .collect();
        Some(format!(
            "{visibility} Rust struct with fields: {}",
            field_summary.join(", ")
        ))
    } else {
        Some(format!("{visibility} Rust unit or tuple struct"))
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "struct".to_string(),
        description,
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Rust enum entity with variants as metadata.
fn extract_rust_enum(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let visibility = detect_visibility(source, node);
    let variants = extract_rust_enum_variants(source, node);

    let description = if !variants.is_empty() {
        Some(format!(
            "{visibility} Rust enum with variants: {}",
            variants.join(", ")
        ))
    } else {
        Some(format!("{visibility} Rust empty enum"))
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "enum".to_string(),
        description,
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Rust trait entity.
fn extract_rust_trait(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let visibility = detect_visibility(source, node);
    let methods = extract_rust_trait_methods(source, node);

    let description = if !methods.is_empty() {
        Some(format!(
            "{visibility} Rust trait with methods: {}",
            methods.join(", ")
        ))
    } else {
        Some(format!("{visibility} Rust marker trait"))
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "trait".to_string(),
        description,
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Rust function entity with signature in description.
fn extract_rust_function(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node);
    let signature = extract_signature_before_brace(text);

    entities.push(ExtractedEntity {
        name,
        entity_type: "function".to_string(),
        description: Some(signature),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Rust impl block entity and its relationships.
fn extract_rust_impl(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let text = node_text(source, node);
    let header = extract_signature_before_brace(text);

    // Parse the impl header to find type and optional trait.
    let (impl_type, impl_trait) = parse_impl_header(&header);

    let impl_name = if let Some(ref trait_name) = impl_trait {
        format!("impl {trait_name} for {impl_type}")
    } else {
        format!("impl {impl_type}")
    };

    entities.push(ExtractedEntity {
        name: impl_name.clone(),
        entity_type: "impl_block".to_string(),
        description: Some(header),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // Relationship: impl with trait → `implements`
    // Relationship: impl without trait → `extends`
    if let Some(trait_name) = impl_trait {
        relationships.push(ExtractedRelationship {
            source_name: impl_type.clone(),
            target_name: trait_name,
            rel_type: "implements".to_string(),
            description: None,
            confidence: 1.0,
        });
    } else {
        relationships.push(ExtractedRelationship {
            source_name: impl_name.clone(),
            target_name: impl_type.clone(),
            rel_type: "extends".to_string(),
            description: None,
            confidence: 1.0,
        });
    }

    // Extract methods inside the impl block as individual function
    // entities with `contains` edges from the impl block. Each method
    // gets its own entity so it can receive a semantic summary and
    // embedding in the same vector space as prose concepts.
    let body = node.child_by_field_name("body");
    if let Some(body_node) = body {
        for i in 0..body_node.child_count() as u32 {
            let Some(child) = body_node.child(i) else {
                continue;
            };
            if child.kind() == "function_item" {
                if let Some(fn_name) = child_text_by_field(source, &child, "name") {
                    // Create a function entity for the method.
                    let method_text = node_text(source, &child);
                    let method_sig = extract_signature_before_brace(method_text);
                    entities.push(ExtractedEntity {
                        name: fn_name.clone(),
                        entity_type: "function".to_string(),
                        description: Some(method_sig),
                        confidence: 1.0,
                        metadata: ast_metadata(source, &child),
                    });

                    // Relationship: impl → method
                    relationships.push(ExtractedRelationship {
                        source_name: impl_name.clone(),
                        target_name: fn_name,
                        rel_type: "contains".to_string(),
                        description: None,
                        confidence: 1.0,
                    });
                }
            }
        }
    }
}

/// Extract a Rust module entity.
fn extract_rust_mod(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "module".to_string(),
        description: Some("Module declaration".to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Rust const or static entity.
fn extract_rust_constant(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let kind = node.kind();
    let keyword = kind.strip_suffix("_item").unwrap_or(kind);

    entities.push(ExtractedEntity {
        name,
        entity_type: "constant".to_string(),
        description: Some(format!("{keyword} declaration")),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Rust macro_rules! entity.
fn extract_rust_macro(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "macro".to_string(),
        description: Some("Macro definition".to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract use declarations as `imports` relationships.
fn extract_rust_use(
    source: &str,
    node: &tree_sitter::Node,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let text = node_text(source, node).trim().to_string();

    // Extract the imported path. Strip `use ` prefix and trailing
    // `;`.
    let path = text
        .strip_prefix("pub use ")
        .or_else(|| text.strip_prefix("pub(crate) use "))
        .or_else(|| text.strip_prefix("use "))
        .unwrap_or(&text);
    let path = path.strip_suffix(';').unwrap_or(path).trim();

    if path.is_empty() {
        return;
    }

    // The "source" of the import is the current module (implicit).
    // We use a placeholder that downstream consumers can map.
    relationships.push(ExtractedRelationship {
        source_name: "<module>".to_string(),
        target_name: path.to_string(),
        rel_type: "imports".to_string(),
        description: None,
        confidence: 1.0,
    });
}

// ── Python extraction ───────────────────────────────────────────

/// Extract entities and relationships from a Python AST.
fn extract_python(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
    let root = tree.root_node();
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut relationships: Vec<ExtractedRelationship> = Vec::new();

    let child_count = root.child_count() as u32;
    for i in 0..child_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        extract_python_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level Python AST node.
fn extract_python_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "class_definition" => {
            extract_python_class(source, node, entities, relationships);
        }
        "function_definition" => {
            extract_python_function(source, node, entities);
        }
        "decorated_definition" => {
            // Unwrap decorated definitions to their inner node.
            for i in 0..node.child_count() as u32 {
                let Some(child) = node.child(i) else {
                    continue;
                };
                match child.kind() {
                    "function_definition" => {
                        extract_python_function(source, &child, entities);
                    }
                    "class_definition" => {
                        extract_python_class(source, &child, entities, relationships);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Extract a Python class entity with methods as metadata.
fn extract_python_class(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let methods = extract_python_methods(source, node);
    let method_count = methods.len();

    // Extract base classes for inheritance relationships.
    let bases = extract_python_bases(source, node);

    let description = if method_count > 0 {
        Some(format!("Class with {method_count} methods"))
    } else {
        Some("Empty class".to_string())
    };

    entities.push(ExtractedEntity {
        name: name.clone(),
        entity_type: "class".to_string(),
        description,
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // Inheritance → `extends` relationships.
    for base in &bases {
        relationships.push(ExtractedRelationship {
            source_name: name.clone(),
            target_name: base.clone(),
            rel_type: "extends".to_string(),
            description: None,
            confidence: 1.0,
        });
    }

    // Extract methods inside the class as individual function entities
    // with `contains` edges from the class.
    if let Some(body_node) = node.child_by_field_name("body") {
        for i in 0..body_node.child_count() as u32 {
            let Some(child) = body_node.child(i) else {
                continue;
            };
            if child.kind() == "function_definition" {
                if let Some(fn_name) = child_text_by_field(source, &child, "name") {
                    let method_text = node_text(source, &child);
                    let method_sig = extract_signature_before_brace(method_text);
                    entities.push(ExtractedEntity {
                        name: fn_name.clone(),
                        entity_type: "function".to_string(),
                        description: Some(method_sig),
                        confidence: 1.0,
                        metadata: ast_metadata(source, &child),
                    });

                    relationships.push(ExtractedRelationship {
                        source_name: name.clone(),
                        target_name: fn_name,
                        rel_type: "contains".to_string(),
                        description: None,
                        confidence: 1.0,
                    });
                }
            }
        }
    }
}

/// Extract a Python function entity.
fn extract_python_function(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node);
    let first_line = text.lines().next().unwrap_or("").trim();
    let signature = first_line
        .strip_suffix(':')
        .unwrap_or(first_line)
        .trim()
        .to_string();

    entities.push(ExtractedEntity {
        name,
        entity_type: "function".to_string(),
        description: Some(signature),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

// ── Go extraction ───────────────────────────────────────────────

/// Extract entities and relationships from a Go AST.
fn extract_go(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
    let root = tree.root_node();
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut relationships: Vec<ExtractedRelationship> = Vec::new();

    let child_count = root.child_count() as u32;
    for i in 0..child_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        extract_go_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level Go AST node.
fn extract_go_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "function_declaration" => {
            extract_go_function(source, node, entities);
        }
        "method_declaration" => {
            extract_go_method(source, node, entities, relationships);
        }
        "type_declaration" => {
            extract_go_type_decl(source, node, entities, relationships);
        }
        "const_declaration" | "var_declaration" => {
            extract_go_const_var(source, node, entities);
        }
        _ => {}
    }
}

/// Extract a Go function entity.
fn extract_go_function(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node);
    let signature = extract_signature_before_brace(text);

    entities.push(ExtractedEntity {
        name,
        entity_type: "function".to_string(),
        description: Some(signature),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Go method entity and its receiver relationship.
fn extract_go_method(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node);
    let signature = extract_signature_before_brace(text);

    // Extract receiver type for the relationship.
    let receiver_type = node
        .child_by_field_name("receiver")
        .and_then(|r| extract_go_receiver_type(source, &r));

    let method_name = if let Some(ref recv) = receiver_type {
        format!("{recv}.{name}")
    } else {
        name.clone()
    };

    entities.push(ExtractedEntity {
        name: method_name.clone(),
        entity_type: "function".to_string(),
        description: Some(signature),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // Relationship: receiver type `contains` this method.
    if let Some(recv) = receiver_type {
        relationships.push(ExtractedRelationship {
            source_name: recv,
            target_name: method_name,
            rel_type: "contains".to_string(),
            description: None,
            confidence: 1.0,
        });
    }
}

/// Extract the receiver type name from a Go method receiver parameter list.
fn extract_go_receiver_type(source: &str, receiver: &tree_sitter::Node) -> Option<String> {
    // receiver is a parameter_list: `(s *Server)` or `(s Server)`
    for i in 0..receiver.child_count() as u32 {
        let Some(child) = receiver.child(i) else {
            continue;
        };
        if child.kind() == "parameter_declaration" {
            let type_node = child.child_by_field_name("type")?;
            let type_text = node_text(source, &type_node).trim();
            // Strip pointer prefix.
            let clean = type_text.strip_prefix('*').unwrap_or(type_text);
            return Some(clean.to_string());
        }
    }
    None
}

/// Extract a Go type declaration (struct, interface, type alias).
fn extract_go_type_decl(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    // type_declaration may contain one or more type_spec children.
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "type_spec" {
            extract_go_type_spec(source, &child, entities, relationships);
        }
    }
}

/// Extract a single Go type spec (struct, interface, or alias).
fn extract_go_type_spec(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let type_node = node.child_by_field_name("type");
    let type_kind = type_node.as_ref().map(|t| t.kind());

    let (entity_type, description) = match type_kind {
        Some("struct_type") => {
            let fields = count_go_struct_fields(type_node.as_ref().unwrap());
            ("struct", format!("Go struct with {fields} fields"))
        }
        Some("interface_type") => {
            let methods = count_go_interface_methods(type_node.as_ref().unwrap());
            ("trait", format!("Go interface with {methods} methods"))
        }
        _ => ("type_alias", "Go type alias".to_string()),
    };

    entities.push(ExtractedEntity {
        name: name.clone(),
        entity_type: entity_type.to_string(),
        description: Some(description),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // Extract embedded types as `extends` relationships.
    if let Some(ref tn) = type_node {
        if tn.kind() == "struct_type" {
            for embed in extract_go_embedded_types(source, tn) {
                relationships.push(ExtractedRelationship {
                    source_name: name.clone(),
                    target_name: embed,
                    rel_type: "extends".to_string(),
                    description: None,
                    confidence: 1.0,
                });
            }
        }
    }
}

/// Extract Go package-level const or var declarations.
fn extract_go_const_var(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let kind = node.kind(); // "const_declaration" or "var_declaration"
    let entity_type = if kind == "const_declaration" {
        "constant"
    } else {
        "variable"
    };

    // Each const/var declaration may have multiple specs.
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "const_spec" || child.kind() == "var_spec" {
            if let Some(name) = child_text_by_field(source, &child, "name") {
                let type_hint = child_text_by_field(source, &child, "type")
                    .unwrap_or_else(|| "(inferred)".to_string());
                entities.push(ExtractedEntity {
                    name,
                    entity_type: entity_type.to_string(),
                    description: Some(format!("Go {kind}: {type_hint}")),
                    confidence: 1.0,
                    metadata: ast_metadata(source, &child),
                });
            }
        }
    }
}

/// Count fields in a Go struct type node.
fn count_go_struct_fields(node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            if child.kind() == "field_declaration_list" {
                for j in 0..child.child_count() as u32 {
                    if let Some(field) = child.child(j) {
                        if field.kind() == "field_declaration" {
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    count
}

/// Count methods in a Go interface type node.
fn count_go_interface_methods(node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            if child.kind() == "method_spec" {
                count += 1;
            }
        }
    }
    count
}

/// Extract embedded (anonymous) types from a Go struct.
fn extract_go_embedded_types(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut embeds = Vec::new();
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else { continue };
        if child.kind() == "field_declaration_list" {
            for j in 0..child.child_count() as u32 {
                let Some(field) = child.child(j) else {
                    continue;
                };
                if field.kind() == "field_declaration" {
                    // Embedded fields have no name, just a type.
                    let has_name = field.child_by_field_name("name").is_some();
                    if !has_name {
                        if let Some(type_node) = field.child_by_field_name("type") {
                            let type_text = node_text(source, &type_node).trim();
                            let clean = type_text.strip_prefix('*').unwrap_or(type_text);
                            embeds.push(clean.to_string());
                        }
                    }
                }
            }
        }
    }
    embeds
}

// ── Helpers ─────────────────────────────────────────────────────

/// Get text of a named field child.
fn child_text_by_field(source: &str, node: &tree_sitter::Node, field: &str) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    Some(source[child.start_byte()..child.end_byte()].to_string())
}

/// Get the full text of a node.
fn node_text<'a>(source: &'a str, node: &tree_sitter::Node) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

/// Detect visibility (pub, pub(crate), private) for a Rust node.
fn detect_visibility(source: &str, node: &tree_sitter::Node) -> &'static str {
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "visibility_modifier" {
            let text = node_text(source, &child);
            if text.contains("crate") {
                return "pub(crate)";
            }
            return "pub";
        }
    }
    "private"
}

/// Extract field information from a Rust struct.
fn extract_rust_fields(source: &str, node: &tree_sitter::Node) -> Vec<serde_json::Value> {
    let mut fields = Vec::new();

    // Look for field_declaration_list body.
    let body = node.child_by_field_name("body");
    let body_node = match body {
        Some(b) => b,
        None => return fields,
    };

    for i in 0..body_node.child_count() as u32 {
        let Some(child) = body_node.child(i) else {
            continue;
        };
        if child.kind() == "field_declaration" {
            let name = child_text_by_field(source, &child, "name");
            let type_node = child.child_by_field_name("type");
            let type_text = type_node.map(|t| node_text(source, &t).to_string());

            if let Some(name) = name {
                let mut field = serde_json::json!({"name": name});
                if let Some(ref t) = type_text {
                    field["type"] = serde_json::json!(t);
                }
                fields.push(field);
            }
        }
    }

    fields
}

/// Extract variant names from a Rust enum.
fn extract_rust_enum_variants(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut variants = Vec::new();

    let body = node.child_by_field_name("body");
    let body_node = match body {
        Some(b) => b,
        None => return variants,
    };

    for i in 0..body_node.child_count() as u32 {
        let Some(child) = body_node.child(i) else {
            continue;
        };
        if child.kind() == "enum_variant" {
            if let Some(name) = child_text_by_field(source, &child, "name") {
                variants.push(name);
            }
        }
    }

    variants
}

/// Extract method signatures from a Rust trait.
fn extract_rust_trait_methods(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut methods = Vec::new();

    let body = node.child_by_field_name("body");
    let body_node = match body {
        Some(b) => b,
        None => return methods,
    };

    for i in 0..body_node.child_count() as u32 {
        let Some(child) = body_node.child(i) else {
            continue;
        };
        if child.kind() == "function_item" || child.kind() == "function_signature_item" {
            if let Some(name) = child_text_by_field(source, &child, "name") {
                methods.push(name);
            }
        }
    }

    methods
}

/// Extract the signature part of a Rust item (before the `{`).
fn extract_signature_before_brace(text: &str) -> String {
    if let Some(pos) = text.find('{') {
        let sig = text[..pos].trim();
        if sig.len() > 120 {
            // Snap to char boundary to avoid panics on non-ASCII.
            let mut end = 117;
            while end > 0 && !sig.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &sig[..end])
        } else {
            sig.to_string()
        }
    } else {
        text.lines().next().unwrap_or("").trim().to_string()
    }
}

/// Parse an impl header to extract the type and optional trait.
///
/// Handles `impl Type`, `impl Trait for Type`,
/// `impl<T> Trait for Type<T>`.
fn parse_impl_header(header: &str) -> (String, Option<String>) {
    // Strip leading `impl` keyword.
    let stripped = header.strip_prefix("impl").unwrap_or(header).trim();

    // Skip leading generic parameters like `<T: Clone>`.
    let after_generics = skip_leading_generics(stripped);

    // Check for ` for ` to detect trait impl.
    if let Some(for_pos) = after_generics.find(" for ") {
        let trait_part = after_generics[..for_pos].trim();
        let type_part = after_generics[for_pos + 5..].trim();

        // Strip generic parameters from trait/type names for
        // cleaner entity names.
        let trait_name = strip_generics(trait_part);
        let type_name = strip_generics(type_part);

        (type_name, Some(trait_name))
    } else {
        let type_name = strip_generics(after_generics);
        (type_name, None)
    }
}

/// Skip a leading generic parameter list (`<...>`) from a string.
///
/// Handles nested angle brackets. Returns the remainder after the
/// closing `>`, or the original string if it doesn't start with `<`.
fn skip_leading_generics(s: &str) -> &str {
    if !s.starts_with('<') {
        return s;
    }
    let mut depth: u32 = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return s[i + 1..].trim();
                }
            }
            _ => {}
        }
    }
    // Unbalanced brackets — return original.
    s
}

/// Strip generic parameters from a type name.
///
/// `Foo<T, U>` → `Foo`, `Bar` → `Bar`.
fn strip_generics(s: &str) -> String {
    if let Some(pos) = s.find('<') {
        s[..pos].trim().to_string()
    } else {
        s.trim().to_string()
    }
}

/// Extract method names from a Python class body.
fn extract_python_methods(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut methods = Vec::new();

    let body = node.child_by_field_name("body");
    let body_node = match body {
        Some(b) => b,
        None => return methods,
    };

    for i in 0..body_node.child_count() as u32 {
        let Some(child) = body_node.child(i) else {
            continue;
        };
        match child.kind() {
            "function_definition" => {
                if let Some(name) = child_text_by_field(source, &child, "name") {
                    methods.push(name);
                }
            }
            "decorated_definition" => {
                // Unwrap decorated methods.
                for j in 0..child.child_count() as u32 {
                    let Some(inner) = child.child(j) else {
                        continue;
                    };
                    if inner.kind() == "function_definition" {
                        if let Some(name) = child_text_by_field(source, &inner, "name") {
                            methods.push(name);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    methods
}

/// Extract base class names from a Python class definition.
fn extract_python_bases(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut bases = Vec::new();

    let superclasses = node.child_by_field_name("superclasses");
    let arg_list = match superclasses {
        Some(al) => al,
        None => return bases,
    };

    for i in 0..arg_list.child_count() as u32 {
        let Some(child) = arg_list.child(i) else {
            continue;
        };
        // Skip punctuation tokens like `(`, `)`, `,`.
        if child.kind() == "identifier" || child.kind() == "attribute" {
            let text = node_text(source, &child).trim().to_string();
            if !text.is_empty() {
                bases.push(text);
            }
        }
    }

    bases
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(uri: &str) -> ExtractionContext {
        ExtractionContext {
            source_type: Some("code".to_string()),
            source_uri: Some(uri.to_string()),
            source_title: None,
        }
    }

    #[tokio::test]
    async fn rust_struct_extraction() {
        let source = r#"
pub struct Config {
    pub chunk_size: usize,
    pub embed_dim: usize,
    name: String,
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("config.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0];
        assert_eq!(entity.name, "Config");
        assert_eq!(entity.entity_type, "struct");
        assert_eq!(entity.confidence, 1.0);
        let desc = entity.description.as_deref().unwrap();
        assert!(desc.contains("Rust struct"), "desc: {desc}");
        assert!(desc.contains("chunk_size"), "desc: {desc}");
        assert!(desc.contains("embed_dim"), "desc: {desc}");
        assert!(desc.contains("name"), "desc: {desc}");
    }

    #[tokio::test]
    async fn rust_struct_fields_not_separate_entities() {
        let source = r#"
pub struct Config {
    pub chunk_size: usize,
    pub embed_dim: usize,
    pub name: String,
    pub overlap: usize,
    pub batch: usize,
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("config.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // Fields must NOT produce separate entities. Only 1 entity
        // for the struct itself.
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "Config");
        assert_eq!(result.entities[0].entity_type, "struct");
    }

    #[tokio::test]
    async fn rust_function_extraction() {
        let source = r#"
pub fn process(input: &str, limit: usize) -> Result<Vec<String>> {
    todo!()
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("lib.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0];
        assert_eq!(entity.name, "process");
        assert_eq!(entity.entity_type, "function");
        assert_eq!(entity.confidence, 1.0);
        // Signature should be in the description.
        let desc = entity.description.as_deref().unwrap();
        assert!(desc.contains("process"));
        assert!(desc.contains("input"));
    }

    #[tokio::test]
    async fn rust_impl_with_trait() {
        let source = r#"
trait Drawable {
    fn draw(&self);
}

struct Circle {
    radius: f64,
}

impl Drawable for Circle {
    fn draw(&self) {
        println!("drawing circle");
    }
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("shapes.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // Should have: trait Drawable, struct Circle, impl block
        let types: Vec<&str> = result
            .entities
            .iter()
            .map(|e| e.entity_type.as_str())
            .collect();
        assert!(types.contains(&"trait"));
        assert!(types.contains(&"struct"));
        assert!(types.contains(&"impl_block"));

        // Should have an `implements` relationship.
        let implements: Vec<_> = result
            .relationships
            .iter()
            .filter(|r| r.rel_type == "implements")
            .collect();
        assert!(!implements.is_empty());
        let rel = &implements[0];
        assert_eq!(rel.source_name, "Circle");
        assert_eq!(rel.target_name, "Drawable");
        assert_eq!(rel.confidence, 1.0);
    }

    #[tokio::test]
    async fn rust_impl_without_trait() {
        let source = r#"
struct Foo {
    value: i32,
}

impl Foo {
    fn new(value: i32) -> Self {
        Self { value }
    }
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("foo.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // Should have an `extends` relationship (impl → struct).
        let extends: Vec<_> = result
            .relationships
            .iter()
            .filter(|r| r.rel_type == "extends")
            .collect();
        assert!(!extends.is_empty());
        assert_eq!(extends[0].target_name, "Foo");

        // Should have a `contains` relationship for the method.
        let contains: Vec<_> = result
            .relationships
            .iter()
            .filter(|r| r.rel_type == "contains")
            .collect();
        assert!(!contains.is_empty());
        assert_eq!(contains[0].target_name, "new");
    }

    #[tokio::test]
    async fn rust_use_declarations() {
        let source = r#"
use std::collections::HashMap;
use crate::error::Result;
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("lib.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // Use declarations produce `imports` relationships.
        let imports: Vec<_> = result
            .relationships
            .iter()
            .filter(|r| r.rel_type == "imports")
            .collect();
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].target_name, "std::collections::HashMap");
        assert_eq!(imports[1].target_name, "crate::error::Result");
    }

    #[tokio::test]
    async fn rust_enum_extraction() {
        let source = r#"
pub enum Color {
    Red,
    Green,
    Blue,
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("color.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0];
        assert_eq!(entity.name, "Color");
        assert_eq!(entity.entity_type, "enum");
        assert_eq!(entity.confidence, 1.0);
        let desc = entity.description.as_deref().unwrap();
        assert!(desc.contains("Rust enum"), "desc: {desc}");
        assert!(desc.contains("Red"), "desc: {desc}");
        assert!(desc.contains("Green"), "desc: {desc}");
        assert!(desc.contains("Blue"), "desc: {desc}");
    }

    #[tokio::test]
    async fn rust_const_and_static() {
        let source = r#"
const MAX_SIZE: usize = 1024;
static COUNTER: AtomicU64 = AtomicU64::new(0);
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("constants.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 2);
        assert!(result.entities.iter().all(|e| e.entity_type == "constant"));
        assert!(result.entities.iter().all(|e| e.confidence == 1.0));
    }

    #[tokio::test]
    async fn rust_macro_extraction() {
        let source = r#"
macro_rules! define_id {
    ($name:ident) => {
        pub struct $name(uuid::Uuid);
    };
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("macros.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "define_id");
        assert_eq!(result.entities[0].entity_type, "macro");
        assert_eq!(result.entities[0].confidence, 1.0);
    }

    #[tokio::test]
    async fn rust_mod_extraction() {
        let source = r#"
mod tests {
    fn test_something() {}
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("lib.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        let mods: Vec<_> = result
            .entities
            .iter()
            .filter(|e| e.entity_type == "module")
            .collect();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].name, "tests");
    }

    #[tokio::test]
    async fn python_class_extraction() {
        let source = r#"
class MyService:
    def __init__(self, config):
        self.config = config

    def process(self, data):
        return data

    def cleanup(self):
        pass
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("service.py");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // 1 class + 3 methods extracted individually
        assert_eq!(result.entities.len(), 4);
        let class = &result.entities[0];
        assert_eq!(class.name, "MyService");
        assert_eq!(class.entity_type, "class");
        assert_eq!(class.confidence, 1.0);
        assert!(class.description.as_deref().unwrap().contains("3 methods"));

        // Methods extracted as individual function entities
        let method_names: Vec<&str> = result.entities[1..]
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(method_names.contains(&"__init__"));
        assert!(method_names.contains(&"process"));
        assert!(method_names.contains(&"cleanup"));
    }

    #[tokio::test]
    async fn python_function_extraction() {
        let source = r#"
def process_data(items: list, limit: int = 10) -> dict:
    result = {}
    for item in items[:limit]:
        result[item] = True
    return result
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("utils.py");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 1);
        let entity = &result.entities[0];
        assert_eq!(entity.name, "process_data");
        assert_eq!(entity.entity_type, "function");
        assert_eq!(entity.confidence, 1.0);
        // Signature should be in description.
        let desc = entity.description.as_deref().unwrap();
        assert!(desc.contains("process_data"));
    }

    #[tokio::test]
    async fn python_class_inheritance() {
        let source = r#"
class Animal:
    def speak(self):
        pass

class Dog(Animal):
    def speak(self):
        return "woof"
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("animals.py");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // 2 classes + 2 methods (one speak() each)
        assert_eq!(result.entities.len(), 4);

        // Dog should have an `extends` relationship to Animal.
        let extends: Vec<_> = result
            .relationships
            .iter()
            .filter(|r| r.rel_type == "extends")
            .collect();
        assert_eq!(extends.len(), 1);
        assert_eq!(extends[0].source_name, "Dog");
        assert_eq!(extends[0].target_name, "Animal");
    }

    #[tokio::test]
    async fn python_decorated_definition() {
        let source = r#"
@staticmethod
def helper():
    pass

@classmethod
def create(cls):
    pass
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("utils.py");
        let result = extractor.extract(source, &ctx).await.unwrap();

        // Decorated definitions should be unwrapped.
        assert_eq!(result.entities.len(), 2);
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"helper"));
        assert!(names.contains(&"create"));
    }

    #[tokio::test]
    async fn empty_source_produces_no_entities() {
        let extractor = AstExtractor::new();
        let ctx = make_context("empty.rs");
        let result = extractor.extract("", &ctx).await.unwrap();

        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[tokio::test]
    async fn all_confidences_are_one() {
        let source = r#"
struct Foo { x: i32 }
enum Bar { A, B }
trait Baz { fn run(&self); }
fn helper() {}
impl Baz for Foo { fn run(&self) {} }
const X: i32 = 0;
mod inner {}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("mix.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        for entity in &result.entities {
            assert_eq!(
                entity.confidence, 1.0,
                "entity {} has confidence != 1.0",
                entity.name
            );
        }
        for rel in &result.relationships {
            assert_eq!(
                rel.confidence, 1.0,
                "relationship {} has confidence != 1.0",
                rel.rel_type
            );
        }
    }

    #[tokio::test]
    async fn unknown_language_returns_empty() {
        let extractor = AstExtractor::new();
        let ctx = ExtractionContext {
            source_type: Some("web_page".to_string()),
            source_uri: Some("index.html".to_string()),
            source_title: None,
        };
        let result = extractor.extract("some content", &ctx).await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[tokio::test]
    async fn markdown_wrapped_code_extraction() {
        // Simulate the output from code_to_markdown.
        let source = r#"# struct Config

```rust
pub struct Config {
    pub size: usize,
}
```

# fn process

```rust
fn process() {
    todo!()
}
```
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("config.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();

        assert_eq!(result.entities.len(), 2);
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Config"));
        assert!(names.contains(&"process"));
    }

    #[test]
    fn unwrap_markdown_preserves_raw_code() {
        let raw = "fn foo() {}";
        assert_eq!(unwrap_markdown_code(raw), raw);
    }

    #[test]
    fn unwrap_markdown_strips_fences() {
        let md = "# heading\n\n```rust\nfn foo() {}\n```\n";
        let result = unwrap_markdown_code(md);
        assert_eq!(result.trim(), "fn foo() {}");
    }

    #[test]
    fn parse_impl_header_trait() {
        let (ty, tr) = parse_impl_header("impl Display for Config");
        assert_eq!(ty, "Config");
        assert_eq!(tr.as_deref(), Some("Display"));
    }

    #[test]
    fn parse_impl_header_plain() {
        let (ty, tr) = parse_impl_header("impl Config");
        assert_eq!(ty, "Config");
        assert!(tr.is_none());
    }

    #[test]
    fn parse_impl_header_generic() {
        let (ty, tr) = parse_impl_header("impl<T: Clone> Iterator for Foo<T>");
        assert_eq!(ty, "Foo");
        assert_eq!(tr.as_deref(), Some("Iterator"));
    }

    #[test]
    fn strip_generics_works() {
        assert_eq!(strip_generics("Vec<T>"), "Vec");
        assert_eq!(strip_generics("Config"), "Config");
        assert_eq!(strip_generics("HashMap<K, V>"), "HashMap");
    }

    #[test]
    fn default_trait_impl() {
        let _extractor: AstExtractor = Default::default();
    }

    #[test]
    fn extract_signature_unicode_no_panic() {
        // Signature > 120 bytes with multi-byte chars must not panic
        // at the truncation boundary.
        let sig_body = "ä".repeat(65); // 65 × 2 bytes = 130 bytes
        let text = format!("fn {sig_body}() {{\n    todo!()\n}}");
        let result = extract_signature_before_brace(&text);
        assert!(
            result.ends_with("..."),
            "expected truncated sig, got: {result}"
        );
    }

    // ── Go extraction tests ─────────────────────────────────────

    fn go_context() -> ExtractionContext {
        ExtractionContext {
            source_uri: Some("file://cmd/root.go".to_string()),
            source_type: Some("code".to_string()),
            source_title: Some("root.go".to_string()),
        }
    }

    #[tokio::test]
    async fn go_function_extraction() {
        let source = r#"package main

func Hello() {
    fmt.Println("hello")
}

func Add(a, b int) int {
    return a + b
}
"#;
        let md = crate::ingestion::code_chunker::code_to_markdown(
            source.trim(),
            crate::ingestion::code_chunker::CodeLanguage::Go,
        )
        .unwrap();
        let extractor = AstExtractor::new();
        let result = extractor.extract(&md, &go_context()).await.unwrap();
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Hello"), "missing Hello in {names:?}");
        assert!(names.contains(&"Add"), "missing Add in {names:?}");
    }

    #[tokio::test]
    async fn go_struct_extraction() {
        let source = r#"package main

type Server struct {
    Host string
    Port int
}
"#;
        let md = crate::ingestion::code_chunker::code_to_markdown(
            source.trim(),
            crate::ingestion::code_chunker::CodeLanguage::Go,
        )
        .unwrap();
        let extractor = AstExtractor::new();
        let result = extractor.extract(&md, &go_context()).await.unwrap();
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Server"), "missing Server in {names:?}");
        let server = result.entities.iter().find(|e| e.name == "Server").unwrap();
        assert_eq!(server.entity_type, "struct");
    }

    #[tokio::test]
    async fn go_interface_extraction() {
        let source = r#"package main

type Reader interface {
    Read(p []byte) (n int, err error)
}
"#;
        let md = crate::ingestion::code_chunker::code_to_markdown(
            source.trim(),
            crate::ingestion::code_chunker::CodeLanguage::Go,
        )
        .unwrap();
        let extractor = AstExtractor::new();
        let result = extractor.extract(&md, &go_context()).await.unwrap();
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Reader"), "missing Reader in {names:?}");
        let reader = result.entities.iter().find(|e| e.name == "Reader").unwrap();
        assert_eq!(reader.entity_type, "trait");
    }

    #[tokio::test]
    async fn go_method_extraction() {
        let source = r#"package main

type Server struct {
    Host string
}

func (s *Server) Start() error {
    return nil
}
"#;
        let md = crate::ingestion::code_chunker::code_to_markdown(
            source.trim(),
            crate::ingestion::code_chunker::CodeLanguage::Go,
        )
        .unwrap();
        let extractor = AstExtractor::new();
        let result = extractor.extract(&md, &go_context()).await.unwrap();
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"Server.Start"),
            "missing Server.Start in {names:?}"
        );
        // Should have a `contains` relationship from Server to Server.Start.
        let contains = result
            .relationships
            .iter()
            .find(|r| r.rel_type == "contains" && r.source_name == "Server");
        assert!(contains.is_some(), "missing contains relationship");
    }

    #[tokio::test]
    async fn go_embedded_type_extraction() {
        let source = r#"package main

type Base struct {
    ID int
}

type Child struct {
    Base
    Name string
}
"#;
        let md = crate::ingestion::code_chunker::code_to_markdown(
            source.trim(),
            crate::ingestion::code_chunker::CodeLanguage::Go,
        )
        .unwrap();
        let extractor = AstExtractor::new();
        let result = extractor.extract(&md, &go_context()).await.unwrap();
        let extends = result
            .relationships
            .iter()
            .find(|r| r.rel_type == "extends" && r.source_name == "Child");
        assert!(
            extends.is_some(),
            "missing extends relationship: {:?}",
            result.relationships
        );
        assert_eq!(extends.unwrap().target_name, "Base");
    }

    #[tokio::test]
    async fn ast_hash_present_on_code_entities() {
        let source = r#"
pub fn hello() -> String {
    "hello".to_string()
}
"#
        .trim();
        let extractor = AstExtractor::new();
        let ctx = make_context("lib.rs");
        let result = extractor.extract(source, &ctx).await.unwrap();
        assert_eq!(result.entities.len(), 1);

        let meta = result.entities[0].metadata.as_ref().unwrap();
        let hash = meta.get("ast_hash").unwrap().as_str().unwrap();
        assert_eq!(hash.len(), 64, "SHA-256 hex should be 64 chars");

        // Same source → same hash (deterministic).
        let result2 = extractor.extract(source, &ctx).await.unwrap();
        let hash2 = result2.entities[0]
            .metadata
            .as_ref()
            .unwrap()
            .get("ast_hash")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(hash, hash2);
    }

    #[tokio::test]
    async fn ast_hash_changes_on_code_change() {
        let source1 = "pub fn greet() -> &'static str { \"hi\" }";
        let source2 = "pub fn greet() -> &'static str { \"hello\" }";

        let extractor = AstExtractor::new();
        let ctx = make_context("lib.rs");

        let r1 = extractor.extract(source1, &ctx).await.unwrap();
        let r2 = extractor.extract(source2, &ctx).await.unwrap();

        let h1 = r1.entities[0]
            .metadata
            .as_ref()
            .unwrap()
            .get("ast_hash")
            .unwrap()
            .as_str()
            .unwrap();
        let h2 = r2.entities[0]
            .metadata
            .as_ref()
            .unwrap()
            .get("ast_hash")
            .unwrap()
            .as_str()
            .unwrap();
        assert_ne!(h1, h2, "different code should produce different hashes");
    }
}
