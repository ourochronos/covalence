//! C-specific tree-sitter AST extraction.
//!
//! Extracts functions, structs, enums, typedefs, macros, and
//! `#include` relationships from C source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{child_text_by_field, extract_signature_before_brace, node_text};

// ── C extraction ───────────────────────────────────────────────

/// Extract entities and relationships from a C AST.
pub(crate) fn extract_c(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
    let root = tree.root_node();
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut relationships: Vec<ExtractedRelationship> = Vec::new();

    let child_count = root.child_count() as u32;
    for i in 0..child_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        extract_c_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level C AST node.
fn extract_c_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "function_definition" => {
            extract_c_function(source, node, entities);
        }
        "declaration" => {
            // Top-level declarations can contain struct/enum
            // specifiers or function prototypes.
            extract_c_declaration(source, node, entities);
        }
        "struct_specifier" => {
            extract_c_struct(source, node, entities);
        }
        "enum_specifier" => {
            extract_c_enum(source, node, entities);
        }
        "type_definition" => {
            extract_c_typedef(source, node, entities);
        }
        "preproc_def" => {
            extract_c_macro(source, node, entities);
        }
        "preproc_function_def" => {
            extract_c_function_macro(source, node, entities);
        }
        "preproc_include" => {
            extract_c_include(source, node, relationships);
        }
        _ => {}
    }
}

/// Extract a C function definition.
fn extract_c_function(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match extract_c_function_name(source, node) {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node);
    let signature = extract_signature_before_brace(text);
    let is_static = text.trim_start().starts_with("static ");

    let description = if is_static {
        format!("static {signature}")
    } else {
        signature
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "function".to_string(),
        description: Some(description),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract the function name from a C function_definition.
///
/// C function names live inside the `declarator` field which can
/// be a `function_declarator` containing an `identifier` or
/// `pointer_declarator` wrapping a `function_declarator`.
fn extract_c_function_name(source: &str, node: &tree_sitter::Node) -> Option<String> {
    let declarator = node.child_by_field_name("declarator")?;
    extract_name_from_declarator(source, &declarator)
}

/// Recursively extract the identifier name from a C declarator.
///
/// In typedef contexts the leaf is a `type_identifier` rather than
/// a plain `identifier`, so we handle both.
fn extract_name_from_declarator(source: &str, node: &tree_sitter::Node) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" | "primitive_type" => {
            Some(node_text(source, node).trim().to_string())
        }
        "function_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            extract_name_from_declarator(source, &inner)
        }
        "pointer_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            extract_name_from_declarator(source, &inner)
        }
        "parenthesized_declarator" => {
            // Walk children to find the inner declarator.
            for i in 0..node.child_count() as u32 {
                if let Some(child) = node.child(i) {
                    if let Some(name) = extract_name_from_declarator(source, &child) {
                        return Some(name);
                    }
                }
            }
            None
        }
        _ => {
            // Try the declarator field if present.
            if let Some(inner) = node.child_by_field_name("declarator") {
                return extract_name_from_declarator(source, &inner);
            }
            None
        }
    }
}

/// Extract entities from a top-level C declaration.
///
/// Handles inline struct/enum specifiers and plain variable
/// declarations.
fn extract_c_declaration(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    // Check if this declaration contains struct or enum specifiers.
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        match child.kind() {
            "struct_specifier" => {
                extract_c_struct(source, &child, entities);
                return;
            }
            "enum_specifier" => {
                extract_c_enum(source, &child, entities);
                return;
            }
            _ => {}
        }
    }

    // Not a struct/enum — check for a named variable or function
    // prototype.
    if let Some(declarator) = node.child_by_field_name("declarator") {
        if let Some(name) = extract_name_from_declarator(source, &declarator) {
            let text = node_text(source, node).trim().to_string();
            // Skip if it looks like a function prototype (has parens
            // in the declarator) — those are forward declarations,
            // not definitions.
            if declarator.kind() == "function_declarator" {
                return;
            }
            entities.push(ExtractedEntity {
                name,
                entity_type: "variable".to_string(),
                description: Some(text),
                confidence: 1.0,
                metadata: ast_metadata(source, node),
            });
        }
    }
}

/// Extract a C struct specifier.
fn extract_c_struct(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return, // Anonymous struct — skip.
    };

    let fields = count_c_struct_fields(node);
    let description = if fields > 0 {
        format!("C struct with {fields} fields")
    } else {
        "C struct (forward declaration or empty)".to_string()
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "struct".to_string(),
        description: Some(description),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a C enum specifier.
fn extract_c_enum(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return, // Anonymous enum — skip.
    };

    let variants = extract_c_enum_values(source, node);
    let description = if !variants.is_empty() {
        format!("C enum with values: {}", variants.join(", "))
    } else {
        "C enum (forward declaration or empty)".to_string()
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "enum".to_string(),
        description: Some(description),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a C typedef declaration.
fn extract_c_typedef(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    // The typedef name is in the declarator field.
    let name = match node.child_by_field_name("declarator") {
        Some(decl) => match extract_name_from_declarator(source, &decl) {
            Some(n) => n,
            None => return,
        },
        None => return,
    };

    // Check if this typedef wraps a struct or enum.
    let mut inner_kind = "type_alias";
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        match child.kind() {
            "struct_specifier" => {
                // Also extract the struct if it has a name.
                extract_c_struct(source, &child, entities);
                inner_kind = "type_alias"; // typedef is still an alias
            }
            "enum_specifier" => {
                extract_c_enum(source, &child, entities);
                inner_kind = "type_alias";
            }
            _ => {}
        }
    }

    entities.push(ExtractedEntity {
        name,
        entity_type: inner_kind.to_string(),
        description: Some("C typedef".to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a C `#define` macro.
fn extract_c_macro(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "macro".to_string(),
        description: Some("C preprocessor macro".to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a C function-like `#define` macro.
fn extract_c_function_macro(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node).trim().to_string();
    entities.push(ExtractedEntity {
        name,
        entity_type: "macro".to_string(),
        description: Some(text),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract `#include` as an imports relationship.
fn extract_c_include(
    source: &str,
    node: &tree_sitter::Node,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let path_node = node.child_by_field_name("path");
    let path = match path_node {
        Some(ref p) => {
            let text = node_text(source, p).trim();
            // Strip angle brackets or quotes.
            text.trim_start_matches('<')
                .trim_end_matches('>')
                .trim_start_matches('"')
                .trim_end_matches('"')
                .to_string()
        }
        None => return,
    };

    if !path.is_empty() {
        relationships.push(ExtractedRelationship {
            source_name: "<module>".to_string(),
            target_name: path,
            rel_type: "imports".to_string(),
            description: None,
            confidence: 1.0,
        });
    }
}

// ── C-specific helpers ─────────────────────────────────────────

/// Count fields in a C struct.
fn count_c_struct_fields(node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i) {
                if child.kind() == "field_declaration" {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Extract enum value names from a C enum.
fn extract_c_enum_values(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i) {
                if child.kind() == "enumerator" {
                    if let Some(name) = child_text_by_field(source, &child, "name") {
                        values.push(name);
                    }
                }
            }
        }
    }
    values
}
