//! Shared extraction utilities used across all language-specific extractors.
//!
//! Contains tree-sitter node helpers, call target extraction, type reference
//! extraction, and signature parsing functions.

// ── CALLS and USES_TYPE extraction ─────────────────────────────

/// Recursively walk an AST subtree to find call targets.
///
/// Extracts function/method names from `call_expression` nodes.
/// Returns deduplicated callee names.
pub(crate) fn extract_call_targets(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut calls = Vec::new();
    collect_calls_recursive(source, node, &mut calls);
    calls.sort();
    calls.dedup();
    calls
}

fn collect_calls_recursive(source: &str, node: &tree_sitter::Node, calls: &mut Vec<String>) {
    if node.kind() == "call_expression" {
        // Try to get the callee name.
        if let Some(func_node) = node.child_by_field_name("function") {
            let callee = match func_node.kind() {
                // Direct call: foo()
                "identifier" => Some(node_text(source, &func_node).to_string()),
                // Method call: self.foo() or obj.foo()
                "field_expression" => func_node
                    .child_by_field_name("field")
                    .map(|f| node_text(source, &f).to_string()),
                // Scoped call: Module::foo()
                "scoped_identifier" => func_node
                    .child_by_field_name("name")
                    .map(|n| node_text(source, &n).to_string()),
                _ => None,
            };
            if let Some(name) = callee {
                // Skip very common names that aren't useful as edges.
                if !matches!(
                    name.as_str(),
                    "clone"
                        | "to_string"
                        | "into"
                        | "from"
                        | "unwrap"
                        | "expect"
                        | "ok"
                        | "err"
                        | "map"
                        | "and_then"
                        | "unwrap_or"
                        | "unwrap_or_default"
                        | "unwrap_or_else"
                        | "is_some"
                        | "is_none"
                        | "is_ok"
                        | "is_err"
                        | "as_ref"
                        | "as_str"
                        | "as_deref"
                        | "len"
                        | "is_empty"
                        | "push"
                        | "insert"
                        | "get"
                        | "contains"
                        | "iter"
                        | "collect"
                        | "filter"
                        | "format"
                        | "println"
                        | "eprintln"
                        | "write"
                        | "debug"
                        | "info"
                        | "warn"
                        | "error"
                        | "bind"
                        | "execute"
                        | "fetch_one"
                        | "fetch_all"
                        | "fetch_optional"
                        | "await"
                ) {
                    calls.push(name);
                }
            }
        }
    }

    // Recurse into children.
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_calls_recursive(source, &child, calls);
        }
    }
}

/// Extract type references from a function's parameters and return type.
///
/// Walks the `parameters` and `return_type` fields of a function_item
/// to find type_identifier nodes. Returns deduplicated type names.
pub(crate) fn extract_type_references(source: &str, node: &tree_sitter::Node) -> Vec<String> {
    let mut types = Vec::new();

    // Walk parameters.
    if let Some(params) = node.child_by_field_name("parameters") {
        collect_type_identifiers(source, &params, &mut types);
    }

    // Walk return type.
    if let Some(ret) = node.child_by_field_name("return_type") {
        collect_type_identifiers(source, &ret, &mut types);
    }

    types.sort();
    types.dedup();

    // Filter out primitives and very common types.
    types.retain(|t| {
        !matches!(
            t.as_str(),
            "bool"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "f32"
                | "f64"
                | "str"
                | "String"
                | "Vec"
                | "Option"
                | "Result"
                | "Box"
                | "Arc"
                | "Rc"
                | "HashMap"
                | "HashSet"
                | "BTreeMap"
                | "BTreeSet"
                | "Cow"
                | "Pin"
                | "Future"
                | "Send"
                | "Sync"
                | "Clone"
                | "Debug"
                | "Display"
                | "Default"
                | "Iterator"
                | "IntoIterator"
                | "Serialize"
                | "Deserialize"
                | "Self"
        )
    });

    types
}

fn collect_type_identifiers(source: &str, node: &tree_sitter::Node, types: &mut Vec<String>) {
    if node.kind() == "type_identifier" {
        types.push(node_text(source, node).to_string());
    }
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_type_identifiers(source, &child, types);
        }
    }
}

/// Get text of a named field child.
pub(crate) fn child_text_by_field(
    source: &str,
    node: &tree_sitter::Node,
    field: &str,
) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    Some(source[child.start_byte()..child.end_byte()].to_string())
}

/// Get the full text of a node.
pub(crate) fn node_text<'a>(source: &'a str, node: &tree_sitter::Node) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

/// Extract the signature part of a Rust item (before the `{`).
pub(crate) fn extract_signature_before_brace(text: &str) -> String {
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
