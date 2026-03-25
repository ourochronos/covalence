//! Go-specific tree-sitter AST extraction.
//!
//! Extracts functions, methods, type declarations (structs, interfaces,
//! aliases), constants, variables, and embedded type relationships from
//! Go source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{
    child_text_by_field, extract_call_targets, extract_signature_before_brace,
    extract_type_references, node_text,
};

// ── Go extraction ───────────────────────────────────────────────

/// Extract entities and relationships from a Go AST.
pub(crate) fn extract_go(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
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
            extract_go_function(source, node, entities, relationships);
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

/// Extract a Go function entity with CALLS and USES_TYPE.
fn extract_go_function(
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

    entities.push(ExtractedEntity {
        name: name.clone(),
        entity_type: "function".to_string(),
        description: Some(signature),
        confidence: 1.0,
        metadata: ast_metadata(source, node),
    });

    // CALLS from function body.
    if let Some(body) = node.child_by_field_name("body") {
        for callee in &extract_call_targets(source, &body) {
            relationships.push(ExtractedRelationship {
                source_name: name.clone(),
                target_name: callee.clone(),
                rel_type: "calls".to_string(),
                description: None,
                confidence: 1.0,
            });
        }
    }

    // USES_TYPE from parameters and return type.
    for type_name in &extract_type_references(source, node) {
        relationships.push(ExtractedRelationship {
            source_name: name.clone(),
            target_name: type_name.clone(),
            rel_type: "uses_type".to_string(),
            description: None,
            confidence: 1.0,
        });
    }
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
            target_name: method_name.clone(),
            rel_type: "contains".to_string(),
            description: None,
            confidence: 1.0,
        });
    }

    // CALLS from method body.
    if let Some(body) = node.child_by_field_name("body") {
        for callee in &extract_call_targets(source, &body) {
            relationships.push(ExtractedRelationship {
                source_name: method_name.clone(),
                target_name: callee.clone(),
                rel_type: "calls".to_string(),
                description: None,
                confidence: 1.0,
            });
        }
    }

    // USES_TYPE from parameters and return type.
    for type_name in &extract_type_references(source, node) {
        relationships.push(ExtractedRelationship {
            source_name: method_name.clone(),
            target_name: type_name.clone(),
            rel_type: "uses_type".to_string(),
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

// ── Go-specific helpers ─────────────────────────────────────────

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
