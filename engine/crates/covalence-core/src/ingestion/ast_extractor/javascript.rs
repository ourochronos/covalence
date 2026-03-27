//! JavaScript-specific tree-sitter AST extraction.
//!
//! Extracts functions, arrow functions, classes, variable
//! declarations, and their relationships from JavaScript source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{child_text_by_field, extract_signature_before_brace, node_text};

// ── JavaScript extraction ──────────────────────────────────────

/// Extract entities and relationships from a JavaScript AST.
pub(crate) fn extract_javascript(
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
        extract_js_node(source, &node, &mut entities, &mut relationships);
    }

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

/// Process a single top-level JavaScript AST node.
fn extract_js_node(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let kind = node.kind();
    match kind {
        "function_declaration" => {
            extract_js_function(source, node, entities);
        }
        "class_declaration" => {
            extract_js_class(source, node, entities, relationships);
        }
        "lexical_declaration" | "variable_declaration" => {
            extract_js_variable_decl(source, node, entities);
        }
        "import_statement" => {
            extract_js_import(source, node, relationships);
        }
        "export_statement" => {
            // Unwrap the export to extract the inner declaration.
            for i in 0..node.child_count() as u32 {
                let Some(child) = node.child(i) else {
                    continue;
                };
                extract_js_node(source, &child, entities, relationships);
            }
        }
        _ => {}
    }
}

/// Extract a JavaScript function declaration.
fn extract_js_function(
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
    let is_generator = text.contains("function*");

    let mut desc = signature;
    if is_async {
        desc = format!("async {desc}");
    }
    if is_generator {
        desc = format!("generator {desc}");
    }

    entities.push(ExtractedEntity {
        name,
        entity_type: "function".to_string(),
        description: Some(desc),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });
}

/// Extract a JavaScript class with methods and heritage.
fn extract_js_class(
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

    // Extract extends heritage.
    extract_js_extends(source, node, &name, relationships);

    // Extract methods as individual entities.
    extract_js_method_entities(source, node, &name, entities, relationships);
}

/// Extract variable declarations — arrow functions or plain variables.
fn extract_js_variable_decl(
    source: &str,
    node: &tree_sitter::Node,
    entities: &mut Vec<ExtractedEntity>,
) {
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else {
            continue;
        };
        if child.kind() == "variable_declarator" {
            extract_js_variable_declarator(source, &child, entities);
        }
    }
}

/// Extract a variable declarator — arrow functions become
/// `arrow_function` entities, others become `variable`.
fn extract_js_variable_declarator(
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
            description: Some("JavaScript variable".to_string()),
            confidence: 1.0,
            metadata: ast_metadata(source, node),
        });
    }
}

/// Extract import relationships from an import statement.
fn extract_js_import(
    source: &str,
    node: &tree_sitter::Node,
    relationships: &mut Vec<ExtractedRelationship>,
) {
    let text = node_text(source, node).trim().to_string();
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

// ── JavaScript-specific helpers ────────────────────────────────

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

/// Extract `extends` heritage from a JavaScript class.
fn extract_js_extends(
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
            // The heritage text is like "extends Base".
            let text = node_text(source, &child).trim();
            let base = text
                .strip_prefix("extends ")
                .and_then(|s| s.split_whitespace().next());
            if let Some(base) = base {
                relationships.push(ExtractedRelationship {
                    source_name: class_name.to_string(),
                    target_name: base.to_string(),
                    rel_type: "extends".to_string(),
                    description: None,
                    confidence: 1.0,
                });
            }
        }
    }
}

/// Extract methods from a class body as individual entities.
fn extract_js_method_entities(
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
