//! TypeScript-specific tree-sitter AST extraction.
//!
//! Extracts functions, arrow functions, classes, interfaces,
//! type aliases, enums, methods, and their relationships from
//! TypeScript source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{child_text_by_field, extract_signature_before_brace, node_text};

// ── TypeScript extraction ──────────────────────────────────────

/// Extract entities and relationships from a TypeScript AST.
pub(crate) fn extract_typescript(
    source: &str,
    tree: &tree_sitter::Tree,
) -> Result<ExtractionResult> {
    let root = tree.root_node();
    let mut entities: Vec<ExtractedEntity> = Vec::new();
    let mut relationships: Vec<ExtractedRelationship> = Vec::new();

    let child_count = root.child_count() as u32;
    for i in 0..child_count {
        let Some(node) = root.child(i) else {
            continue;
        };
        extract_ts_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level TypeScript AST node.
fn extract_ts_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "function_declaration" => {
            extract_ts_function(source, node, entities);
        }
        "class_declaration" => {
            extract_ts_class(source, node, entities, relationships);
        }
        "interface_declaration" => {
            extract_ts_interface(source, node, entities);
        }
        "type_alias_declaration" => {
            extract_ts_type_alias(source, node, entities);
        }
        "enum_declaration" => {
            extract_ts_enum(source, node, entities);
        }
        "lexical_declaration" => {
            extract_ts_lexical(source, node, entities);
        }
        "import_statement" => {
            extract_ts_import(source, node, relationships);
        }
        "export_statement" => {
            // Unwrap the export to extract the inner declaration.
            for i in 0..node.child_count() as u32 {
                let Some(child) = node.child(i) else {
                    continue;
                };
                extract_ts_node(source, &child, entities, relationships);
            }
        }
        _ => {}
    }
}

/// Extract a TypeScript function declaration.
fn extract_ts_function(
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
    let is_async = text.trim_start().starts_with("async ");

    let description = if is_async {
        format!("async {signature}")
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

/// Extract a TypeScript class with methods and heritage.
fn extract_ts_class(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let methods = count_class_methods(node);
    let description = if methods > 0 {
        Some(format!("Class with {methods} methods"))
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

    // Extract heritage: extends and implements.
    extract_class_heritage(source, node, &name, relationships);

    // Extract methods as individual entities.
    extract_class_method_entities(source, node, &name, entities, relationships);
}

/// Extract a TypeScript interface declaration.
fn extract_ts_interface(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let text = node_text(source, node);
    let header = extract_signature_before_brace(text);

    entities.push(ExtractedEntity {
        name,
        entity_type: "interface".to_string(),
        description: Some(header),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a TypeScript type alias declaration.
fn extract_ts_type_alias(
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
        entity_type: "type_alias".to_string(),
        description: Some("TypeScript type alias".to_string()),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a TypeScript enum declaration.
fn extract_ts_enum(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let variants = extract_enum_members(source, node);
    let description = if !variants.is_empty() {
        format!("Enum with members: {}", variants.join(", "))
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

/// Extract a lexical declaration (const/let with arrow functions or values).
fn extract_ts_lexical(source: &str, node: &tree_sitter::Node, entities: &mut Vec<ExtractedEntity>) {
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "variable_declarator" {
            extract_ts_variable_declarator(source, &child, entities);
        }
    }
}

/// Extract a variable declarator — if initialized with an arrow function,
/// emit an `arrow_function` entity; otherwise emit a `variable`.
fn extract_ts_variable_declarator(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    let name = match child_text_by_field(source, node, "name") {
        Some(n) => n,
        None => return,
    };

    let value_node = node.child_by_field_name("value");
    let is_arrow = value_node
        .as_ref()
        .is_some_and(|v| v.kind() == "arrow_function");

    if is_arrow {
        let text = node_text(source, node);
        entities.push(ExtractedEntity {
            name,
            entity_type: "arrow_function".to_string(),
            description: Some(text.to_string()),
            confidence: 1.0,
            metadata: ast_metadata(source, node),
        });
    } else {
        entities.push(ExtractedEntity {
            name,
            entity_type: "variable".to_string(),
            description: Some("TypeScript variable".to_string()),
            confidence: 1.0,
            metadata: ast_metadata(source, node),
        });
    }
}

/// Extract import relationships from an import statement.
fn extract_ts_import(
    source: &str,
    node: &tree_sitter::Node,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let text = node_text(source, node).trim().to_string();
    // Extract the source module from `from 'module'`.
    if let Some(src) = node.child_by_field_name("source") {
        let module = node_text(source, &src)
            .trim()
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
        if !module.is_empty() {
            relationships.push(ExtractedRelationship {
                source_name: "<module>".to_string(),
                target_name: module,
                rel_type: "imports".to_string(),
                description: Some(text),
                confidence: 1.0,
            });
        }
    }
}

// ── TypeScript-specific helpers ────────────────────────────────

/// Count method definitions in a class body.
fn count_class_methods(node: &tree_sitter::Node) -> usize {
    let mut count = 0;
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i) {
                if child.kind() == "method_definition" {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Extract extends/implements from class heritage.
fn extract_class_heritage(
    source: &str,
    node: &tree_sitter::Node,
    class_name: &str,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "class_heritage" {
            extract_heritage_clause(source, &child, class_name, relationships);
        }
    }
}

/// Parse a class_heritage node for extends/implements clauses.
fn extract_heritage_clause(
    source: &str,
    node: &tree_sitter::Node,
    class_name: &str,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let text = node_text(source, node);
    // Split on `extends` and `implements` keywords.
    // Heritage text is like "extends Base implements Foo, Bar".
    let text = text.trim();

    // Look for `extends Foo` pattern.
    if let Some(after_extends) = text.strip_prefix("extends ") {
        let base = after_extends
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(',');
        if !base.is_empty() {
            relationships.push(ExtractedRelationship {
                source_name: class_name.to_string(),
                target_name: base.to_string(),
                rel_type: "extends".to_string(),
                description: None,
                confidence: 1.0,
            });
        }
    }

    // Look for `implements Foo, Bar` pattern.
    if let Some(pos) = text.find("implements ") {
        let after = &text[pos + 11..];
        for iface in after.split(',') {
            let iface = iface.trim();
            let iface = iface.split_whitespace().next().unwrap_or("");
            if !iface.is_empty() {
                relationships.push(ExtractedRelationship {
                    source_name: class_name.to_string(),
                    target_name: iface.to_string(),
                    rel_type: "implements".to_string(),
                    description: None,
                    confidence: 1.0,
                });
            }
        }
    }
}

/// Extract methods from a class body as individual entities.
fn extract_class_method_entities(
    source: &str,
    node: &tree_sitter::Node,
    class_name: &str,
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
        if child.kind() == "method_definition" {
            if let Some(fn_name) = child_text_by_field(source, &child, "name") {
                let text = node_text(source, &child);
                let sig = extract_signature_before_brace(text);

                entities.push(ExtractedEntity {
                    name: fn_name.clone(),
                    entity_type: "function".to_string(),
                    description: Some(sig),
                    confidence: 1.0,
                    metadata: ast_metadata(source, &child),
                });

                relationships.push(ExtractedRelationship {
                    source_name: class_name.to_string(),
                    target_name: fn_name,
                    rel_type: "contains".to_string(),
                    description: None,
                    confidence: 1.0,
                });
            }
        }
    }
}

/// Extract enum member names.
fn extract_enum_members(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut members = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() as u32 {
            if let Some(child) = body.child(i) {
                // tree-sitter-typescript uses "property_identifier"
                // inside enum bodies as member names.
                if child.kind() == "enum_assignment" {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        members.push(node_text(source, &name_node).trim().to_string());
                    }
                } else if child.kind() == "property_identifier" {
                    members.push(node_text(source, &child).trim().to_string());
                }
            }
        }
    }
    members
}
