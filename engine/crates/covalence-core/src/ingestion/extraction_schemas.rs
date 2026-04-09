//! JSON schemas for structured LLM outputs.
//!
//! When the chat backend supports native `json_schema` response
//! format (OpenAI API), these schemas are sent alongside the
//! request instead of being embedded in the prompt text. This
//! saves ~40% of input tokens on extraction calls and eliminates
//! output formatting errors.
//!
//! The schemas here are the single source of truth — they match
//! the `ExtractionResult` struct in [`super::extractor`].

use std::sync::LazyLock;

/// JSON Schema for entity and relationship extraction.
///
/// Matches the expected output of `entity_extraction.md` and is
/// parsed into [`super::extractor::ExtractionResult`].
pub static ENTITY_EXTRACTION_SCHEMA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    serde_json::json!({
        "type": "object",
        "properties": {
            "entities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "entity_type": { "type": "string" },
                        "description": { "type": ["string", "null"] },
                        "confidence": { "type": "number" }
                    },
                    "required": ["name", "entity_type", "confidence"],
                    "additionalProperties": false
                }
            },
            "relationships": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "source_name": { "type": "string" },
                        "target_name": { "type": "string" },
                        "rel_type": { "type": "string" },
                        "description": { "type": ["string", "null"] },
                        "confidence": { "type": "number" }
                    },
                    "required": ["source_name", "target_name", "rel_type", "confidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["entities", "relationships"],
        "additionalProperties": false
    })
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json() {
        // Verify the schema is well-formed and has the expected
        // top-level structure.
        let schema = &*ENTITY_EXTRACTION_SCHEMA;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["entities"].is_object());
        assert!(schema["properties"]["relationships"].is_object());
    }

    #[test]
    fn schema_entities_require_name_and_type() {
        let items = &ENTITY_EXTRACTION_SCHEMA["properties"]["entities"]["items"];
        let required: Vec<&str> = items["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"name"));
        assert!(required.contains(&"entity_type"));
        assert!(required.contains(&"confidence"));
    }

    #[test]
    fn schema_relationships_require_source_target_type() {
        let items = &ENTITY_EXTRACTION_SCHEMA["properties"]["relationships"]["items"];
        let required: Vec<&str> = items["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"source_name"));
        assert!(required.contains(&"target_name"));
        assert!(required.contains(&"rel_type"));
    }
}
