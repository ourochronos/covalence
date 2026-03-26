//! Integration tests for the STDIO service contract.
//!
//! These tests invoke the `covalence-ast-extractor` binary via
//! `std::process::Command` and verify the JSON-in/JSON-out contract.

use std::process::Command;

/// Helper: run the binary with the given JSON input and return stdout.
fn run_extractor(input: &str) -> (String, bool) {
    let bin = env!("CARGO_BIN_EXE_covalence-ast-extractor");
    let output = Command::new(bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .expect("failed to run covalence-ast-extractor");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (stdout, output.status.success())
}

#[test]
fn ping_returns_pong() {
    let (stdout, success) = run_extractor(r#"{"ping": true}"#);
    assert!(success, "ping should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));
    assert_eq!(v["pong"], true);
    assert_eq!(v["name"], "covalence-ast-extractor");
    assert!(v["version"].is_string());
    let langs = v["languages"].as_array().unwrap();
    let lang_strs: Vec<&str> = langs.iter().map(|l| l.as_str().unwrap()).collect();
    assert!(lang_strs.contains(&"rust"));
    assert!(lang_strs.contains(&"python"));
    assert!(lang_strs.contains(&"go"));
}

#[test]
fn extract_rust_function() {
    let input = serde_json::json!({
        "source_code": "pub fn hello(name: &str) -> String {\n    format!(\"hello {name}\")\n}",
        "language": "rust",
        "file_path": "src/lib.rs"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    assert_eq!(v["language"], "rust");
    assert!(v["file_hash"].is_string());

    let entities = v["entities"].as_array().unwrap();
    assert!(!entities.is_empty(), "should extract at least one entity");
    let names: Vec<&str> = entities
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"hello"), "missing hello in {names:?}");

    let hello = entities.iter().find(|e| e["name"] == "hello").unwrap();
    assert_eq!(hello["entity_type"], "function");
    assert_eq!(hello["confidence"], 1.0);
}

#[test]
fn extract_rust_struct() {
    let input = serde_json::json!({
        "source_code": "pub struct Config {\n    pub size: usize,\n    pub name: String,\n}",
        "language": "rust"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    let entities = v["entities"].as_array().unwrap();
    let names: Vec<&str> = entities
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Config"), "missing Config in {names:?}");

    let config = entities.iter().find(|e| e["name"] == "Config").unwrap();
    assert_eq!(config["entity_type"], "struct");
}

#[test]
fn extract_rust_impl_produces_relationships() {
    let input = serde_json::json!({
        "source_code": "trait Greet {\n    fn greet(&self);\n}\n\nstruct Bot;\n\nimpl Greet for Bot {\n    fn greet(&self) {}\n}",
        "file_path": "src/bot.rs"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    let rels = v["relationships"].as_array().unwrap();
    let implements: Vec<_> = rels
        .iter()
        .filter(|r| r["rel_type"] == "implements")
        .collect();
    assert!(
        !implements.is_empty(),
        "should have implements relationship"
    );
    assert_eq!(implements[0]["source"], "Bot");
    assert_eq!(implements[0]["target"], "Greet");
}

#[test]
fn extract_python_class() {
    let input = serde_json::json!({
        "source_code": "class Animal:\n    def speak(self):\n        pass\n\nclass Dog(Animal):\n    def speak(self):\n        return 'woof'\n",
        "language": "python",
        "file_path": "animals.py"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    assert_eq!(v["language"], "python");

    let entities = v["entities"].as_array().unwrap();
    let names: Vec<&str> = entities
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Animal"), "missing Animal in {names:?}");
    assert!(names.contains(&"Dog"), "missing Dog in {names:?}");

    // Dog extends Animal.
    let rels = v["relationships"].as_array().unwrap();
    let extends: Vec<_> = rels.iter().filter(|r| r["rel_type"] == "extends").collect();
    assert!(!extends.is_empty(), "should have extends relationship");
    assert_eq!(extends[0]["source"], "Dog");
    assert_eq!(extends[0]["target"], "Animal");
}

#[test]
fn extract_go_function() {
    let input = serde_json::json!({
        "source_code": "package main\n\nfunc Add(a, b int) int {\n    return a + b\n}\n",
        "language": "go",
        "file_path": "math.go"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    assert_eq!(v["language"], "go");

    let entities = v["entities"].as_array().unwrap();
    let names: Vec<&str> = entities
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Add"), "missing Add in {names:?}");
}

#[test]
fn extract_go_struct_and_method() {
    let input = serde_json::json!({
        "source_code": "package main\n\ntype Server struct {\n    Host string\n}\n\nfunc (s *Server) Start() error {\n    return nil\n}\n",
        "file_path": "server.go"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    let entities = v["entities"].as_array().unwrap();
    let names: Vec<&str> = entities
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Server"), "missing Server in {names:?}");
    assert!(
        names.contains(&"Server.Start"),
        "missing Server.Start in {names:?}"
    );

    let rels = v["relationships"].as_array().unwrap();
    let contains: Vec<_> = rels
        .iter()
        .filter(|r| r["rel_type"] == "contains")
        .collect();
    assert!(!contains.is_empty(), "should have contains relationship");
}

#[test]
fn language_detection_from_file_path() {
    // No explicit language — should detect from file_path extension.
    let input = serde_json::json!({
        "source_code": "def greet():\n    return 'hi'\n",
        "file_path": "util.py"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "extraction should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    assert_eq!(v["language"], "python");
    let entities = v["entities"].as_array().unwrap();
    assert!(!entities.is_empty(), "should detect python and extract");
}

#[test]
fn unknown_language_returns_empty() {
    let input = serde_json::json!({
        "source_code": "<html><body>hello</body></html>",
        "file_path": "index.html"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "unknown language should still succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    let entities = v["entities"].as_array().unwrap();
    assert!(
        entities.is_empty(),
        "unknown language should produce no entities"
    );
}

#[test]
fn empty_source_returns_empty() {
    let input = serde_json::json!({
        "source_code": "",
        "language": "rust"
    });
    let (stdout, success) = run_extractor(&input.to_string());
    assert!(success, "empty source should succeed");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nraw: {stdout}"));

    let entities = v["entities"].as_array().unwrap();
    assert!(entities.is_empty());
}

#[test]
fn missing_source_code_fails() {
    let input = serde_json::json!({
        "language": "rust"
    });
    let (_, success) = run_extractor(&input.to_string());
    assert!(!success, "missing source_code should fail");
}

#[test]
fn file_hash_is_deterministic() {
    let input = serde_json::json!({
        "source_code": "fn foo() {}",
        "language": "rust"
    });
    let (stdout1, _) = run_extractor(&input.to_string());
    let (stdout2, _) = run_extractor(&input.to_string());
    let v1: serde_json::Value = serde_json::from_str(&stdout1).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&stdout2).unwrap();
    assert_eq!(v1["file_hash"], v2["file_hash"]);
}

#[test]
fn file_hash_changes_with_content() {
    let input1 = serde_json::json!({
        "source_code": "fn foo() {}",
        "language": "rust"
    });
    let input2 = serde_json::json!({
        "source_code": "fn bar() {}",
        "language": "rust"
    });
    let (stdout1, _) = run_extractor(&input1.to_string());
    let (stdout2, _) = run_extractor(&input2.to_string());
    let v1: serde_json::Value = serde_json::from_str(&stdout1).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&stdout2).unwrap();
    assert_ne!(v1["file_hash"], v2["file_hash"]);
}
