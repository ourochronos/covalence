//! Python-specific tree-sitter AST extraction.
//!
//! Extracts classes, functions, decorated definitions, and inheritance
//! relationships from Python source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{child_text_by_field, extract_signature_before_brace, node_text};

// ── Python extraction ───────────────────────────────────────────

/// Extract entities and relationships from a Python AST.
pub(crate) fn extract_python(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
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

// ── Python-specific helpers ─────────────────────────────────────

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
