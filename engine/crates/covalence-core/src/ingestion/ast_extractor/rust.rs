//! Rust-specific tree-sitter AST extraction.
//!
//! Extracts structs, enums, traits, functions, impl blocks, modules,
//! constants, macros, and use declarations from Rust source code.

use crate::error::Result;
use crate::ingestion::extractor::{ExtractedEntity, ExtractedRelationship, ExtractionResult};

use super::ast_metadata;
use super::common::{
    child_text_by_field, extract_call_targets, extract_signature_before_brace,
    extract_type_references, node_text,
};

// ── Rust extraction ─────────────────────────────────────────────

/// Extract entities and relationships from a Rust AST.
pub(crate) fn extract_rust(source: &str, tree: &tree_sitter::Tree) -> Result<ExtractionResult> {
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
            extract_rust_function_full(source, node, entities, relationships);
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

/// Extract a Rust function with relationships (CALLS, USES_TYPE, entity).
fn extract_rust_function_full(
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

    // Extract CALLS relationships from function body.
    if let Some(body) = node.child_by_field_name("body") {
        let calls = extract_call_targets(source, &body);
        for callee in &calls {
            relationships.push(ExtractedRelationship {
                source_name: name.clone(),
                target_name: callee.clone(),
                rel_type: "calls".to_string(),
                description: None,
                confidence: 1.0,
            });
        }
    }

    // Extract USES_TYPE relationships from parameters and return type.
    let types = extract_type_references(source, node);
    for type_name in &types {
        relationships.push(ExtractedRelationship {
            source_name: name.clone(),
            target_name: type_name.clone(),
            rel_type: "uses_type".to_string(),
            description: None,
            confidence: 1.0,
        });
    }
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
                    // Extract method as full function entity with
                    // CALLS + USES_TYPE relationships.
                    extract_rust_function_full(source, &child, entities, relationships);

                    // Relationship: impl → method (contains)
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

// ── Rust-specific helpers ───────────────────────────────────────

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

/// Parse an impl header to extract the type and optional trait.
///
/// Handles `impl Type`, `impl Trait for Type`,
/// `impl<T> Trait for Type<T>`.
pub(crate) fn parse_impl_header(header: &str) -> (String, Option<String>) {
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
pub(crate) fn strip_generics(s: &str) -> String {
    if let Some(pos) = s.find('<') {
        s[..pos].trim().to_string()
    } else {
        s.trim().to_string()
    }
}
