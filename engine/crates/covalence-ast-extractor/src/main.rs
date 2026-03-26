//! Standalone AST extractor — STDIO service contract.
//!
//! Reads JSON from stdin, extracts entities and relationships from
//! source code using tree-sitter, writes JSON to stdout.
//! Stateless — each invocation is independent.
//!
//! ## Input
//!
//! ```json
//! {
//!   "source_code": "fn main() { }",
//!   "language": "rust",
//!   "file_path": "src/main.rs"
//! }
//! ```
//!
//! ## Output
//!
//! ```json
//! {
//!   "entities": [...],
//!   "relationships": [...],
//!   "language": "rust",
//!   "file_hash": "abc123..."
//! }
//! ```
//!
//! ## Health check
//!
//! ```json
//! {"ping": true}
//! ```

use std::io::{self, Read};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use covalence_core::ingestion::AstExtractor;
use covalence_core::ingestion::extractor::{ExtractionContext, Extractor};

/// STDIO request envelope.
#[derive(Deserialize)]
struct Request {
    /// If true, respond with a pong health check instead of extracting.
    #[serde(default)]
    ping: bool,
    /// Source code to extract entities from.
    source_code: Option<String>,
    /// Language hint (e.g. "rust", "python", "go").
    language: Option<String>,
    /// File path used for language detection when `language` is absent.
    file_path: Option<String>,
}

/// STDIO response envelope for extraction results.
#[derive(Serialize)]
struct Response {
    /// Extracted entities.
    entities: Vec<EntityOut>,
    /// Extracted relationships between entities.
    relationships: Vec<RelationOut>,
    /// Detected or specified language.
    language: String,
    /// SHA-256 hash of the input source code.
    file_hash: String,
}

/// Serialized entity in the STDIO output.
#[derive(Serialize)]
struct EntityOut {
    /// Entity name as extracted from the AST.
    name: String,
    /// Entity type (e.g. "function", "struct", "class").
    entity_type: String,
    /// Human-readable description (signature, fields, etc.).
    description: Option<String>,
    /// Extraction confidence (always 1.0 for AST extraction).
    confidence: f64,
    /// Optional metadata (e.g. `ast_hash` for incremental detection).
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

/// Serialized relationship in the STDIO output.
#[derive(Serialize)]
struct RelationOut {
    /// Name of the source entity.
    source: String,
    /// Name of the target entity.
    target: String,
    /// Relationship type (e.g. "implements", "extends", "calls").
    rel_type: String,
    /// Optional relationship description.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Extraction confidence (always 1.0 for AST extraction).
    confidence: f64,
}

/// Health check response.
#[derive(Serialize)]
struct PongResponse {
    /// Always true.
    pong: bool,
    /// Service name.
    name: String,
    /// Service version (from Cargo.toml).
    version: String,
    /// Supported languages.
    languages: Vec<String>,
}

/// Map a language name to a file extension for `ExtractionContext`.
fn language_to_extension(lang: &str) -> &str {
    match lang {
        "rust" => "rs",
        "python" => "py",
        "go" => "go",
        other => other,
    }
}

/// Build an `ExtractionContext` from the request fields.
///
/// Language detection in `AstExtractor` works via the source URI
/// extension, so we construct a synthetic URI when only a language
/// hint is provided.
fn build_context(language: Option<&str>, file_path: Option<&str>) -> ExtractionContext {
    let source_uri = match (file_path, language) {
        // Prefer an explicit file path.
        (Some(path), _) => Some(path.to_string()),
        // Fall back to a synthetic path from the language hint.
        (None, Some(lang)) => {
            let ext = language_to_extension(lang);
            Some(format!("stdin.{ext}"))
        }
        (None, None) => None,
    };

    ExtractionContext {
        source_type: Some("code".to_string()),
        source_uri,
        source_title: file_path.map(|p| p.to_string()),
    }
}

/// Detect the effective language name from the request.
fn detect_language_name(language: Option<&str>, file_path: Option<&str>) -> String {
    if let Some(lang) = language {
        return lang.to_string();
    }
    if let Some(path) = file_path {
        if let Some(ext) = path.rsplit('.').next() {
            return match ext {
                "rs" => "rust",
                "py" => "python",
                "go" => "go",
                other => other,
            }
            .to_string();
        }
    }
    "unknown".to_string()
}

/// Compute a SHA-256 hash of the full source code.
fn compute_file_hash(source: &str) -> String {
    let hash = Sha256::digest(source.as_bytes());
    format!("{hash:x}")
}

fn main() {
    // Read all of stdin.
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("failed to read stdin");

    // Parse request JSON.
    let req: Request = match serde_json::from_str(&input) {
        Ok(r) => r,
        Err(e) => {
            let err = serde_json::json!({
                "error": format!("invalid request JSON: {e}")
            });
            println!("{}", serde_json::to_string(&err).unwrap());
            std::process::exit(1);
        }
    };

    // Handle ping.
    if req.ping {
        let pong = PongResponse {
            pong: true,
            name: "covalence-ast-extractor".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "go".to_string()],
        };
        println!("{}", serde_json::to_string(&pong).unwrap());
        return;
    }

    // Validate required fields.
    let source_code = match req.source_code {
        Some(ref s) => s.as_str(),
        None => {
            let err = serde_json::json!({
                "error": "source_code is required for extraction"
            });
            println!("{}", serde_json::to_string(&err).unwrap());
            std::process::exit(1);
        }
    };

    let context = build_context(req.language.as_deref(), req.file_path.as_deref());
    let language_name = detect_language_name(req.language.as_deref(), req.file_path.as_deref());
    let file_hash = compute_file_hash(source_code);

    // Run the AST extractor. It is async via the Extractor trait but
    // performs no actual I/O, so a current-thread runtime suffices.
    let extractor = AstExtractor::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("failed to create tokio runtime");
    let result = rt.block_on(extractor.extract(source_code, &context));

    match result {
        Ok(extraction) => {
            let response = Response {
                entities: extraction
                    .entities
                    .into_iter()
                    .map(|e| EntityOut {
                        name: e.name,
                        entity_type: e.entity_type,
                        description: e.description,
                        confidence: e.confidence,
                        metadata: e.metadata,
                    })
                    .collect(),
                relationships: extraction
                    .relationships
                    .into_iter()
                    .map(|r| RelationOut {
                        source: r.source_name,
                        target: r.target_name,
                        rel_type: r.rel_type,
                        description: r.description,
                        confidence: r.confidence,
                    })
                    .collect(),
                language: language_name,
                file_hash,
            };
            println!("{}", serde_json::to_string(&response).unwrap());
        }
        Err(e) => {
            let err = serde_json::json!({
                "error": format!("extraction failed: {e}")
            });
            println!("{}", serde_json::to_string(&err).unwrap());
            std::process::exit(1);
        }
    }
}
