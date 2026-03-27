//! Java-specific tree-sitter AST extraction.
//!
//! Extracts classes, interfaces, enums, annotation types, methods,
//! and their relationships from Java source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{child_text_by_field, extract_signature_before_brace, node_text};

// ── Java extraction ────────────────────────────────────────────

/// Extract entities and relationships from a Java AST.
pub(crate) fn extract_java(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
    let root = tree.root_node();
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut relationships: Vec<ExtractedRelationship> = Vec::new();

    // Java files wrap everything in a `program` node. Walk its
    // children which are typically package_declaration,
    // import_declaration, and type declarations.
    let child_count = root.child_count() as u32;
    for i in 0..child_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        extract_java_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level Java AST node.
fn extract_java_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "class_declaration" => {
            extract_java_class(source, node, entities, relationships);
        }
        "interface_declaration" => {
            extract_java_interface(source, node, entities, relationships);
        }
        "enum_declaration" => {
            extract_java_enum(source, node, entities);
        }
        "annotation_type_declaration" => {
            extract_java_annotation_type(source, node, entities);
        }
        "import_declaration" => {
            extract_java_import(source, node, relationships);
        }
        "method_declaration" => {
            // Top-level method (shouldn't normally happen in Java
            // but can appear in some AST contexts).
            extract_java_method_entity(source, node, entities);
        }
        _ => {}
    }
}

/// Extract a Java class declaration with methods and heritage.
fn extract_java_class(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let visibility = detect_java_visibility(source, node);
    let is_abstract = has_modifier(source, node, "abstract");
    let methods = count_java_methods(node);

    let mut desc = format!("{visibility} Java class");
    if is_abstract {
        desc = format!("{visibility} abstract Java class");
    }
    if methods > 0 {
        desc = format!("{desc} with {methods} methods");
    }

    entities.push(ExtractedEntity {
        name: name.clone(),
        entity_type: "class".to_string(),
        description: Some(desc),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // Extract extends (superclass).
    if let Some(superclass) = node.child_by_field_name("superclass") {
        let base = node_text(source, &superclass).trim().to_string();
        if !base.is_empty() {
            relationships.push(ExtractedRelationship {
                source_name: name.clone(),
                target_name: base,
                rel_type: "extends".to_string(),
                description: None,
                confidence: 1.0,
            });
        }
    }

    // Extract implements (interfaces).
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        extract_java_type_list(source, &interfaces, &name, "implements", relationships);
    }

    // Extract methods as individual entities.
    extract_java_method_entities(source, node, &name, entities, relationships);
}

/// Extract a Java interface declaration.
fn extract_java_interface(
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
    let header = extract_signature_before_brace(text);

    entities.push(ExtractedEntity {
        name: name.clone(),
        entity_type: "interface".to_string(),
        description: Some(header),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // Extract extends for interfaces (interface extends).
    if let Some(extends_node) = node.child_by_field_name("type_parameters") {
        // Interfaces use extends_interfaces field in some grammars.
        let _ = extends_node; // Handled below via direct child walk.
    }

    // Walk children looking for extends_type_list.
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "extends_interfaces" {
            extract_java_type_list(source, &child, &name, "extends", relationships);
        }
    }

    // Extract method signatures inside the interface.
    extract_java_method_entities(source, node, &name, entities, relationships);
}

/// Extract a Java enum declaration.
fn extract_java_enum(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let constants = extract_java_enum_constants(source, node);
    let description = if !constants.is_empty() {
        format!("Enum with constants: {}", constants.join(", "))
    } else {
        "Empty enum".to_string()
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "enum".to_string(),
        description: Some(description),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a Java annotation type declaration.
fn extract_java_annotation_type(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    entities.push(ExtractedEntity {
        name,
        entity_type: "interface".to_string(),
        description: Some("Java annotation type".to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract import declarations as `imports` relationships.
fn extract_java_import(
    source: &str,
    node: &tree_sitter::Node,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let text = node_text(source, node).trim().to_string();
    // Strip `import ` prefix, optional `static `, and trailing `;`.
    let path = text
        .strip_prefix("import static ")
        .or_else(|| text.strip_prefix("import "))
        .unwrap_or(&text);
    let path = path.strip_suffix(';').unwrap_or(path).trim();

    if !path.is_empty() {
        relationships.push(ExtractedRelationship {
            source_name: "<module>".to_string(),
            target_name: path.to_string(),
            rel_type: "imports".to_string(),
            description: None,
            confidence: 1.0,
        });
    }
}

/// Extract a standalone Java method as an entity.
fn extract_java_method_entity(
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
    let visibility = detect_java_visibility(source, node);
    let is_static = has_modifier(source, node, "static");
    let is_abstract = has_modifier(source, node, "abstract");

    let mut desc = visibility.to_string();
    if is_static {
        desc = format!("{desc} static");
    }
    if is_abstract {
        desc = format!("{desc} abstract");
    }
    desc = format!("{desc} {signature}");

    entities.push(ExtractedEntity {
        name,
        entity_type: "function".to_string(),
        description: Some(desc.trim().to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

// ── Java-specific helpers ──────────────────────────────────────

/// Detect visibility modifier for a Java node.
fn detect_java_visibility(source: &str, node: &tree_sitter::Node) -> &'static str {
    if let Some(modifiers) = node.child_by_field_name("modifiers") {
        let text = node_text(source, &modifiers);
        if text.contains("public") {
            return "public";
        }
        if text.contains("protected") {
            return "protected";
        }
        if text.contains("private") {
            return "private";
        }
    }
    // Also check direct children (some grammar versions).
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "modifiers" {
            let text = node_text(source, &child);
            if text.contains("public") {
                return "public";
            }
            if text.contains("protected") {
                return "protected";
            }
            if text.contains("private") {
                return "private";
            }
        }
    }
    "package-private"
}

/// Check if a Java node has a specific modifier keyword.
fn has_modifier(source: &str, node: &tree_sitter::Node, modifier: &str) -> bool {
    if let Some(modifiers) = node.child_by_field_name("modifiers") {
        let text = node_text(source, &modifiers);
        return text.contains(modifier);
    }
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "modifiers" {
            let text = node_text(source, &child);
            if text.contains(modifier) {
                return true;
            }
        }
    }
    false
}

/// Count methods in a Java class or interface body.
fn count_java_methods(node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i) {
                if child.kind() == "method_declaration" || child.kind() == "constructor_declaration"
                {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Extract methods from a class/interface body as entities.
fn extract_java_method_entities(
    source: &str,
    node: &tree_sitter::Node,
    parent_name: &str,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };

    for i in 0..body.child_count() as u32 {
        let Some(child) = body.child(i) else {
            continue;
        };
        if child.kind() == "method_declaration" || child.kind() == "constructor_declaration" {
            if let Some(fn_name) = child_text_by_field(source, &child, "name") {
                extract_java_method_entity(source, &child, entities);

                relationships.push(ExtractedRelationship {
                    source_name: parent_name.to_string(),
                    target_name: fn_name,
                    rel_type: "contains".to_string(),
                    description: None,
                    confidence: 1.0,
                });
            }
        }
    }
}

/// Extract type names from a Java type list (implements, extends).
fn extract_java_type_list(
    source: &str,
    node: &tree_sitter::Node,
    class_name: &str,
    rel_type: &str,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "type_identifier"
            || child.kind() == "generic_type"
            || child.kind() == "type_list"
        {
            if child.kind() == "type_list" {
                // Recurse into type_list.
                extract_java_type_list(source, &child, class_name, rel_type, relationships);
            } else {
                let type_name = node_text(source, &child).trim().to_string();
                // Strip generics for cleaner entity names.
                let clean = if let Some(pos) = type_name.find('<') {
                    type_name[..pos].trim().to_string()
                } else {
                    type_name
                };
                if !clean.is_empty() {
                    relationships.push(ExtractedRelationship {
                        source_name: class_name.to_string(),
                        target_name: clean,
                        rel_type: rel_type.to_string(),
                        description: None,
                        confidence: 1.0,
                    });
                }
            }
        }
    }
}

/// Extract enum constant names from a Java enum declaration.
fn extract_java_enum_constants(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut constants = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i) {
                if child.kind() == "enum_constant" {
                    if let Some(name) = child_text_by_field(source, &child, "name") {
                        constants.push(name);
                    }
                }
            }
        }
    }
    constants
}
