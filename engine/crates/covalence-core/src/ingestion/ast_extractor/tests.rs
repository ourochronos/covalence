//! Tests for the AST extractor module.

use super::*;
use crate::ingestion::code_chunker::CodeLanguage;
use crate::ingestion::extractor::ExtractionContext;

fn make_context(uri: &str) -> ExtractionContext {
    ExtractionContext {
        source_type: Some("code".to_string()),
        source_uri: Some(uri.to_string()),
        source_title: None,
    }
}

#[tokio::test]
async fn rust_struct_extraction() {
    let source = r#"
pub struct Config {
    pub chunk_size: usize,
    pub embed_dim: usize,
    name: String,
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("config.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 1);
    let entity = &result.entities[0];
    assert_eq!(entity.name, "Config");
    assert_eq!(entity.entity_type, "struct");
    assert_eq!(entity.confidence, 1.0);
    let desc = entity.description.as_deref().unwrap();
    assert!(desc.contains("Rust struct"), "desc: {desc}");
    assert!(desc.contains("chunk_size"), "desc: {desc}");
    assert!(desc.contains("embed_dim"), "desc: {desc}");
    assert!(desc.contains("name"), "desc: {desc}");
}

#[tokio::test]
async fn rust_struct_fields_not_separate_entities() {
    let source = r#"
pub struct Config {
    pub chunk_size: usize,
    pub embed_dim: usize,
    pub name: String,
    pub overlap: usize,
    pub batch: usize,
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("config.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // Fields must NOT produce separate entities. Only 1 entity
    // for the struct itself.
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].name, "Config");
    assert_eq!(result.entities[0].entity_type, "struct");
}

#[tokio::test]
async fn rust_function_extraction() {
    let source = r#"
pub fn process(input: &str, limit: usize) -> Result<Vec<String>> {
    todo!()
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("lib.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 1);
    let entity = &result.entities[0];
    assert_eq!(entity.name, "process");
    assert_eq!(entity.entity_type, "function");
    assert_eq!(entity.confidence, 1.0);
    // Signature should be in the description.
    let desc = entity.description.as_deref().unwrap();
    assert!(desc.contains("process"));
    assert!(desc.contains("input"));
}

#[tokio::test]
async fn rust_impl_with_trait() {
    let source = r#"
trait Drawable {
    fn draw(&self);
}

struct Circle {
    radius: f64,
}

impl Drawable for Circle {
    fn draw(&self) {
        println!("drawing circle");
    }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("shapes.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // Should have: trait Drawable, struct Circle, impl block
    let types: Vec<&str> = result
        .entities
        .iter()
        .map(|e| e.entity_type.as_str())
        .collect();
    assert!(types.contains(&"trait"));
    assert!(types.contains(&"struct"));
    assert!(types.contains(&"impl_block"));

    // Should have an `implements` relationship.
    let implements: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "implements")
        .collect();
    assert!(!implements.is_empty());
    let rel = &implements[0];
    assert_eq!(rel.source_name, "Circle");
    assert_eq!(rel.target_name, "Drawable");
    assert_eq!(rel.confidence, 1.0);
}

#[tokio::test]
async fn rust_impl_without_trait() {
    let source = r#"
struct Foo {
    value: i32,
}

impl Foo {
    fn new(value: i32) -> Self {
        Self { value }
    }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("foo.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // Should have an `extends` relationship (impl → struct).
    let extends: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "extends")
        .collect();
    assert!(!extends.is_empty());
    assert_eq!(extends[0].target_name, "Foo");

    // Should have a `contains` relationship for the method.
    let contains: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "contains")
        .collect();
    assert!(!contains.is_empty());
    assert_eq!(contains[0].target_name, "new");
}

#[tokio::test]
async fn rust_use_declarations() {
    let source = r#"
use std::collections::HashMap;
use crate::error::Result;
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("lib.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // Use declarations produce `imports` relationships.
    let imports: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "imports")
        .collect();
    assert_eq!(imports.len(), 2);
    assert_eq!(imports[0].target_name, "std::collections::HashMap");
    assert_eq!(imports[1].target_name, "crate::error::Result");
}

#[tokio::test]
async fn rust_enum_extraction() {
    let source = r#"
pub enum Color {
    Red,
    Green,
    Blue,
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("color.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 1);
    let entity = &result.entities[0];
    assert_eq!(entity.name, "Color");
    assert_eq!(entity.entity_type, "enum");
    assert_eq!(entity.confidence, 1.0);
    let desc = entity.description.as_deref().unwrap();
    assert!(desc.contains("Rust enum"), "desc: {desc}");
    assert!(desc.contains("Red"), "desc: {desc}");
    assert!(desc.contains("Green"), "desc: {desc}");
    assert!(desc.contains("Blue"), "desc: {desc}");
}

#[tokio::test]
async fn rust_const_and_static() {
    let source = r#"
const MAX_SIZE: usize = 1024;
static COUNTER: AtomicU64 = AtomicU64::new(0);
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("constants.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 2);
    assert!(result.entities.iter().all(|e| e.entity_type == "constant"));
    assert!(result.entities.iter().all(|e| e.confidence == 1.0));
}

#[tokio::test]
async fn rust_macro_extraction() {
    let source = r#"
macro_rules! define_id {
    ($name:ident) => {
        pub struct $name(uuid::Uuid);
    };
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("macros.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].name, "define_id");
    assert_eq!(result.entities[0].entity_type, "macro");
    assert_eq!(result.entities[0].confidence, 1.0);
}

#[tokio::test]
async fn rust_mod_extraction() {
    let source = r#"
mod tests {
    fn test_something() {}
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("lib.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    let mods: Vec<_> = result
        .entities
        .iter()
        .filter(|e| e.entity_type == "module")
        .collect();
    assert_eq!(mods.len(), 1);
    assert_eq!(mods[0].name, "tests");
}

#[tokio::test]
async fn python_class_extraction() {
    let source = r#"
class MyService:
    def __init__(self, config):
        self.config = config

    def process(self, data):
        return data

    def cleanup(self):
        pass
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("service.py");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // 1 class + 3 methods extracted individually
    assert_eq!(result.entities.len(), 4);
    let class = &result.entities[0];
    assert_eq!(class.name, "MyService");
    assert_eq!(class.entity_type, "class");
    assert_eq!(class.confidence, 1.0);
    assert!(class.description.as_deref().unwrap().contains("3 methods"));

    // Methods extracted as individual function entities
    let method_names: Vec<&str> = result.entities[1..]
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert!(method_names.contains(&"__init__"));
    assert!(method_names.contains(&"process"));
    assert!(method_names.contains(&"cleanup"));
}

#[tokio::test]
async fn python_function_extraction() {
    let source = r#"
def process_data(items: list, limit: int = 10) -> dict:
    result = {}
    for item in items[:limit]:
        result[item] = True
    return result
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("utils.py");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 1);
    let entity = &result.entities[0];
    assert_eq!(entity.name, "process_data");
    assert_eq!(entity.entity_type, "function");
    assert_eq!(entity.confidence, 1.0);
    // Signature should be in description.
    let desc = entity.description.as_deref().unwrap();
    assert!(desc.contains("process_data"));
}

#[tokio::test]
async fn python_class_inheritance() {
    let source = r#"
class Animal:
    def speak(self):
        pass

class Dog(Animal):
    def speak(self):
        return "woof"
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("animals.py");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // 2 classes + 2 methods (one speak() each)
    assert_eq!(result.entities.len(), 4);

    // Dog should have an `extends` relationship to Animal.
    let extends: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "extends")
        .collect();
    assert_eq!(extends.len(), 1);
    assert_eq!(extends[0].source_name, "Dog");
    assert_eq!(extends[0].target_name, "Animal");
}

#[tokio::test]
async fn python_decorated_definition() {
    let source = r#"
@staticmethod
def helper():
    pass

@classmethod
def create(cls):
    pass
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("utils.py");
    let result = extractor.extract(source, &ctx).await.unwrap();

    // Decorated definitions should be unwrapped.
    assert_eq!(result.entities.len(), 2);
    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"helper"));
    assert!(names.contains(&"create"));
}

#[tokio::test]
async fn empty_source_produces_no_entities() {
    let extractor = AstExtractor::new();
    let ctx = make_context("empty.rs");
    let result = extractor.extract("", &ctx).await.unwrap();

    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}

#[tokio::test]
async fn all_confidences_are_one() {
    let source = r#"
struct Foo { x: i32 }
enum Bar { A, B }
trait Baz { fn run(&self); }
fn helper() {}
impl Baz for Foo { fn run(&self) {} }
const X: i32 = 0;
mod inner {}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("mix.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    for entity in &result.entities {
        assert_eq!(
            entity.confidence, 1.0,
            "entity {} has confidence != 1.0",
            entity.name
        );
    }
    for rel in &result.relationships {
        assert_eq!(
            rel.confidence, 1.0,
            "relationship {} has confidence != 1.0",
            rel.rel_type
        );
    }
}

#[tokio::test]
async fn unknown_language_returns_empty() {
    let extractor = AstExtractor::new();
    let ctx = ExtractionContext {
        source_type: Some("web_page".to_string()),
        source_uri: Some("index.html".to_string()),
        source_title: None,
    };
    let result = extractor.extract("some content", &ctx).await.unwrap();
    assert!(result.entities.is_empty());
    assert!(result.relationships.is_empty());
}

#[tokio::test]
async fn markdown_wrapped_code_extraction() {
    // Simulate the output from code_to_markdown.
    let source = r#"# struct Config

```rust
pub struct Config {
    pub size: usize,
}
```

# fn process

```rust
fn process() {
    todo!()
}
```
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("config.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();

    assert_eq!(result.entities.len(), 2);
    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Config"));
    assert!(names.contains(&"process"));
}

#[test]
fn unwrap_markdown_preserves_raw_code() {
    let raw = "fn foo() {}";
    assert_eq!(unwrap_markdown_code(raw), raw);
}

#[test]
fn unwrap_markdown_strips_fences() {
    let md = "# heading\n\n```rust\nfn foo() {}\n```\n";
    let result = unwrap_markdown_code(md);
    assert_eq!(result.trim(), "fn foo() {}");
}

#[test]
fn parse_impl_header_trait() {
    let (ty, tr) = rust::parse_impl_header("impl Display for Config");
    assert_eq!(ty, "Config");
    assert_eq!(tr.as_deref(), Some("Display"));
}

#[test]
fn parse_impl_header_plain() {
    let (ty, tr) = rust::parse_impl_header("impl Config");
    assert_eq!(ty, "Config");
    assert!(tr.is_none());
}

#[test]
fn parse_impl_header_generic() {
    let (ty, tr) = rust::parse_impl_header("impl<T: Clone> Iterator for Foo<T>");
    assert_eq!(ty, "Foo");
    assert_eq!(tr.as_deref(), Some("Iterator"));
}

#[test]
fn strip_generics_works() {
    assert_eq!(rust::strip_generics("Vec<T>"), "Vec");
    assert_eq!(rust::strip_generics("Config"), "Config");
    assert_eq!(rust::strip_generics("HashMap<K, V>"), "HashMap");
}

#[test]
fn default_trait_impl() {
    let _extractor: AstExtractor = Default::default();
}

#[test]
fn extract_signature_unicode_no_panic() {
    // Signature > 120 bytes with multi-byte chars must not panic
    // at the truncation boundary.
    let sig_body = "ä".repeat(65); // 65 × 2 bytes = 130 bytes
    let text = format!("fn {sig_body}() {{\n    todo!()\n}}");
    let result = common::extract_signature_before_brace(&text);
    assert!(
        result.ends_with("..."),
        "expected truncated sig, got: {result}"
    );
}

// ── Go extraction tests ─────────────────────────────────────

fn go_context() -> ExtractionContext {
    ExtractionContext {
        source_uri: Some("file://cmd/root.go".to_string()),
        source_type: Some("code".to_string()),
        source_title: Some("root.go".to_string()),
    }
}

#[tokio::test]
async fn go_function_extraction() {
    let source = r#"package main

func Hello() {
    fmt.Println("hello")
}

func Add(a, b int) int {
    return a + b
}
"#;
    let md = crate::ingestion::code_chunker::code_to_markdown(
        source.trim(),
        crate::ingestion::code_chunker::CodeLanguage::Go,
    )
    .unwrap();
    let extractor = AstExtractor::new();
    let result = extractor.extract(&md, &go_context()).await.unwrap();
    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Hello"), "missing Hello in {names:?}");
    assert!(names.contains(&"Add"), "missing Add in {names:?}");
}

#[tokio::test]
async fn go_struct_extraction() {
    let source = r#"package main

type Server struct {
    Host string
    Port int
}
"#;
    let md = crate::ingestion::code_chunker::code_to_markdown(
        source.trim(),
        crate::ingestion::code_chunker::CodeLanguage::Go,
    )
    .unwrap();
    let extractor = AstExtractor::new();
    let result = extractor.extract(&md, &go_context()).await.unwrap();
    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Server"), "missing Server in {names:?}");
    let server = result.entities.iter().find(|e| e.name == "Server").unwrap();
    assert_eq!(server.entity_type, "struct");
}

#[tokio::test]
async fn go_interface_extraction() {
    let source = r#"package main

type Reader interface {
    Read(p []byte) (n int, err error)
}
"#;
    let md = crate::ingestion::code_chunker::code_to_markdown(
        source.trim(),
        crate::ingestion::code_chunker::CodeLanguage::Go,
    )
    .unwrap();
    let extractor = AstExtractor::new();
    let result = extractor.extract(&md, &go_context()).await.unwrap();
    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Reader"), "missing Reader in {names:?}");
    let reader = result.entities.iter().find(|e| e.name == "Reader").unwrap();
    assert_eq!(reader.entity_type, "trait");
}

#[tokio::test]
async fn go_method_extraction() {
    let source = r#"package main

type Server struct {
    Host string
}

func (s *Server) Start() error {
    return nil
}
"#;
    let md = crate::ingestion::code_chunker::code_to_markdown(
        source.trim(),
        crate::ingestion::code_chunker::CodeLanguage::Go,
    )
    .unwrap();
    let extractor = AstExtractor::new();
    let result = extractor.extract(&md, &go_context()).await.unwrap();
    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"Server.Start"),
        "missing Server.Start in {names:?}"
    );
    // Should have a `contains` relationship from Server to Server.Start.
    let contains = result
        .relationships
        .iter()
        .find(|r| r.rel_type == "contains" && r.source_name == "Server");
    assert!(contains.is_some(), "missing contains relationship");
}

#[tokio::test]
async fn go_embedded_type_extraction() {
    let source = r#"package main

type Base struct {
    ID int
}

type Child struct {
    Base
    Name string
}
"#;
    let md = crate::ingestion::code_chunker::code_to_markdown(
        source.trim(),
        crate::ingestion::code_chunker::CodeLanguage::Go,
    )
    .unwrap();
    let extractor = AstExtractor::new();
    let result = extractor.extract(&md, &go_context()).await.unwrap();
    let extends = result
        .relationships
        .iter()
        .find(|r| r.rel_type == "extends" && r.source_name == "Child");
    assert!(
        extends.is_some(),
        "missing extends relationship: {:?}",
        result.relationships
    );
    assert_eq!(extends.unwrap().target_name, "Base");
}

#[tokio::test]
async fn ast_hash_present_on_code_entities() {
    let source = r#"
pub fn hello() -> String {
    "hello".to_string()
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let ctx = make_context("lib.rs");
    let result = extractor.extract(source, &ctx).await.unwrap();
    assert_eq!(result.entities.len(), 1);

    let meta = result.entities[0].metadata.as_ref().unwrap();
    let hash = meta.get("ast_hash").unwrap().as_str().unwrap();
    assert_eq!(hash.len(), 64, "SHA-256 hex should be 64 chars");

    // Same source → same hash (deterministic).
    let result2 = extractor.extract(source, &ctx).await.unwrap();
    let hash2 = result2.entities[0]
        .metadata
        .as_ref()
        .unwrap()
        .get("ast_hash")
        .unwrap()
        .as_str()
        .unwrap();
    assert_eq!(hash, hash2);
}

#[tokio::test]
async fn ast_hash_changes_on_code_change() {
    let source1 = "pub fn greet() -> &'static str { \"hi\" }";
    let source2 = "pub fn greet() -> &'static str { \"hello\" }";

    let extractor = AstExtractor::new();
    let ctx = make_context("lib.rs");

    let r1 = extractor.extract(source1, &ctx).await.unwrap();
    let r2 = extractor.extract(source2, &ctx).await.unwrap();

    let h1 = r1.entities[0]
        .metadata
        .as_ref()
        .unwrap()
        .get("ast_hash")
        .unwrap()
        .as_str()
        .unwrap();
    let h2 = r2.entities[0]
        .metadata
        .as_ref()
        .unwrap()
        .get("ast_hash")
        .unwrap()
        .as_str()
        .unwrap();
    assert_ne!(h1, h2, "different code should produce different hashes");
}

// ── TypeScript extraction tests ─────────────────────────────

fn ts_context() -> ExtractionContext {
    ExtractionContext {
        source_uri: Some("file://src/app.ts".to_string()),
        source_type: Some("code".to_string()),
        source_title: Some("app.ts".to_string()),
    }
}

#[tokio::test]
async fn typescript_class_and_interface() {
    let source = r#"
interface Serializable {
    serialize(): string;
}

class Config implements Serializable {
    constructor(public name: string) {}

    serialize(): string {
        return this.name;
    }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &ts_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"Serializable"),
        "missing Serializable in {names:?}"
    );
    assert!(names.contains(&"Config"), "missing Config in {names:?}");

    let config = result.entities.iter().find(|e| e.name == "Config").unwrap();
    assert_eq!(config.entity_type, "class");

    let iface = result
        .entities
        .iter()
        .find(|e| e.name == "Serializable")
        .unwrap();
    assert_eq!(iface.entity_type, "interface");

    // Check implements relationship.
    let implements: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "implements")
        .collect();
    assert!(
        !implements.is_empty(),
        "missing implements: {:?}",
        result.relationships
    );
    assert_eq!(implements[0].source_name, "Config");
    assert_eq!(implements[0].target_name, "Serializable");
}

#[tokio::test]
async fn typescript_function_and_arrow() {
    let source = r#"
function greet(name: string): string {
    return `Hello, ${name}`;
}

const add = (a: number, b: number): number => a + b;
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &ts_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"greet"), "missing greet in {names:?}");
    assert!(names.contains(&"add"), "missing add in {names:?}");

    let greet = result.entities.iter().find(|e| e.name == "greet").unwrap();
    assert_eq!(greet.entity_type, "function");

    let add = result.entities.iter().find(|e| e.name == "add").unwrap();
    assert_eq!(add.entity_type, "arrow_function");
}

#[tokio::test]
async fn typescript_enum_and_type_alias() {
    let source = r#"
type ID = string | number;

enum Direction {
    Up,
    Down,
    Left,
    Right,
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &ts_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"ID"), "missing ID in {names:?}");
    assert!(
        names.contains(&"Direction"),
        "missing Direction in {names:?}"
    );

    let id = result.entities.iter().find(|e| e.name == "ID").unwrap();
    assert_eq!(id.entity_type, "type_alias");

    let dir = result
        .entities
        .iter()
        .find(|e| e.name == "Direction")
        .unwrap();
    assert_eq!(dir.entity_type, "enum");
}

#[tokio::test]
async fn typescript_import_relationship() {
    let source = r#"
import { Component } from '@angular/core';
import * as fs from 'fs';

function init() {}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &ts_context()).await.unwrap();

    let imports: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "imports")
        .collect();
    assert_eq!(imports.len(), 2, "expected 2 imports: {imports:?}");
    let targets: Vec<&str> = imports.iter().map(|r| r.target_name.as_str()).collect();
    assert!(
        targets.contains(&"@angular/core"),
        "missing @angular/core in {targets:?}"
    );
    assert!(targets.contains(&"fs"), "missing fs in {targets:?}");
}

// ── JavaScript extraction tests ─────────────────────────────

fn js_context() -> ExtractionContext {
    ExtractionContext {
        source_uri: Some("file://src/app.js".to_string()),
        source_type: Some("code".to_string()),
        source_title: Some("app.js".to_string()),
    }
}

#[tokio::test]
async fn javascript_function_and_class() {
    let source = r#"
function processData(items) {
    return items.map(i => i.value);
}

class EventEmitter {
    constructor() {
        this.listeners = {};
    }

    on(event, fn) {
        this.listeners[event] = fn;
    }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &js_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"processData"),
        "missing processData in {names:?}"
    );
    assert!(
        names.contains(&"EventEmitter"),
        "missing EventEmitter in {names:?}"
    );

    let class = result
        .entities
        .iter()
        .find(|e| e.name == "EventEmitter")
        .unwrap();
    assert_eq!(class.entity_type, "class");
    assert!(
        class.description.as_deref().unwrap().contains("2 methods"),
        "desc: {:?}",
        class.description
    );

    // Methods extracted as individual entities.
    assert!(
        names.contains(&"constructor"),
        "missing constructor in {names:?}"
    );
    assert!(names.contains(&"on"), "missing on in {names:?}");
}

#[tokio::test]
async fn javascript_arrow_function() {
    let source = r#"
const multiply = (a, b) => a * b;
const identity = x => x;
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &js_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"multiply"), "missing multiply in {names:?}");
    assert!(names.contains(&"identity"), "missing identity in {names:?}");

    let mult = result
        .entities
        .iter()
        .find(|e| e.name == "multiply")
        .unwrap();
    assert_eq!(mult.entity_type, "arrow_function");
}

#[tokio::test]
async fn javascript_class_extends() {
    let source = r#"
class Animal {
    speak() { return "..."; }
}

class Dog extends Animal {
    speak() { return "woof"; }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &js_context()).await.unwrap();

    let extends: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "extends")
        .collect();
    assert!(!extends.is_empty(), "missing extends relationship");
    assert_eq!(extends[0].source_name, "Dog");
    assert_eq!(extends[0].target_name, "Animal");
}

// ── Java extraction tests ───────────────────────────────────

fn java_context() -> ExtractionContext {
    ExtractionContext {
        source_uri: Some("file://src/App.java".to_string()),
        source_type: Some("code".to_string()),
        source_title: Some("App.java".to_string()),
    }
}

#[tokio::test]
async fn java_class_and_method() {
    let source = r#"
public class Calculator {
    public int add(int a, int b) {
        return a + b;
    }

    private double multiply(double x, double y) {
        return x * y;
    }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &java_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"Calculator"),
        "missing Calculator in {names:?}"
    );
    assert!(names.contains(&"add"), "missing add in {names:?}");
    assert!(names.contains(&"multiply"), "missing multiply in {names:?}");

    let class = result
        .entities
        .iter()
        .find(|e| e.name == "Calculator")
        .unwrap();
    assert_eq!(class.entity_type, "class");
    assert!(
        class.description.as_deref().unwrap().contains("2 methods"),
        "desc: {:?}",
        class.description
    );

    // Methods should have `contains` relationships.
    let contains: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "contains" && r.source_name == "Calculator")
        .collect();
    assert_eq!(contains.len(), 2, "expected 2 contains: {contains:?}");
}

#[tokio::test]
async fn java_interface_and_implements() {
    let source = r#"
public interface Readable {
    String read();
}

public class FileReader implements Readable {
    public String read() {
        return "content";
    }
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &java_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Readable"), "missing Readable in {names:?}");
    assert!(
        names.contains(&"FileReader"),
        "missing FileReader in {names:?}"
    );

    let iface = result
        .entities
        .iter()
        .find(|e| e.name == "Readable")
        .unwrap();
    assert_eq!(iface.entity_type, "interface");

    // Check implements relationship.
    let implements: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "implements")
        .collect();
    assert!(
        !implements.is_empty(),
        "missing implements: {:?}",
        result.relationships
    );
}

#[tokio::test]
async fn java_enum_extraction() {
    let source = r#"
public enum Color {
    RED,
    GREEN,
    BLUE;
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &java_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Color"), "missing Color in {names:?}");

    let color = result.entities.iter().find(|e| e.name == "Color").unwrap();
    assert_eq!(color.entity_type, "enum");
    let desc = color.description.as_deref().unwrap();
    assert!(desc.contains("RED"), "desc: {desc}");
    assert!(desc.contains("GREEN"), "desc: {desc}");
    assert!(desc.contains("BLUE"), "desc: {desc}");
}

#[tokio::test]
async fn java_import_extraction() {
    let source = r#"
import java.util.List;
import java.util.Map;

public class App {}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &java_context()).await.unwrap();

    let imports: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "imports")
        .collect();
    assert_eq!(imports.len(), 2, "expected 2 imports: {imports:?}");
    let targets: Vec<&str> = imports.iter().map(|r| r.target_name.as_str()).collect();
    assert!(
        targets.contains(&"java.util.List"),
        "missing List in {targets:?}"
    );
    assert!(
        targets.contains(&"java.util.Map"),
        "missing Map in {targets:?}"
    );
}

// ── C extraction tests ──────────────────────────────────────

fn c_context() -> ExtractionContext {
    ExtractionContext {
        source_uri: Some("file://src/main.c".to_string()),
        source_type: Some("code".to_string()),
        source_title: Some("main.c".to_string()),
    }
}

#[tokio::test]
async fn c_function_extraction() {
    let source = r#"
int add(int a, int b) {
    return a + b;
}

static void helper(void) {
    /* internal */
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &c_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"add"), "missing add in {names:?}");
    assert!(names.contains(&"helper"), "missing helper in {names:?}");

    let add = result.entities.iter().find(|e| e.name == "add").unwrap();
    assert_eq!(add.entity_type, "function");

    let helper = result.entities.iter().find(|e| e.name == "helper").unwrap();
    assert!(
        helper.description.as_deref().unwrap().contains("static"),
        "desc: {:?}",
        helper.description
    );
}

#[tokio::test]
async fn c_struct_extraction() {
    let source = r#"
struct Point {
    int x;
    int y;
};
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &c_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Point"), "missing Point in {names:?}");

    let point = result.entities.iter().find(|e| e.name == "Point").unwrap();
    assert_eq!(point.entity_type, "struct");
    assert!(
        point.description.as_deref().unwrap().contains("2 fields"),
        "desc: {:?}",
        point.description
    );
}

#[tokio::test]
async fn c_enum_extraction() {
    let source = r#"
enum Color {
    RED,
    GREEN,
    BLUE
};
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &c_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Color"), "missing Color in {names:?}");

    let color = result.entities.iter().find(|e| e.name == "Color").unwrap();
    assert_eq!(color.entity_type, "enum");
    let desc = color.description.as_deref().unwrap();
    assert!(desc.contains("RED"), "desc: {desc}");
    assert!(desc.contains("GREEN"), "desc: {desc}");
    assert!(desc.contains("BLUE"), "desc: {desc}");
}

#[tokio::test]
async fn c_macro_extraction() {
    let source = r#"
#define MAX_SIZE 1024
#define SQUARE(x) ((x) * (x))
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &c_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"MAX_SIZE"), "missing MAX_SIZE in {names:?}");
    assert!(names.contains(&"SQUARE"), "missing SQUARE in {names:?}");

    for entity in &result.entities {
        assert_eq!(entity.entity_type, "macro");
        assert_eq!(entity.confidence, 1.0);
    }
}

#[tokio::test]
async fn c_include_extraction() {
    let source = r#"
#include <stdio.h>
#include "myheader.h"

int main(void) {
    return 0;
}
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &c_context()).await.unwrap();

    let imports: Vec<_> = result
        .relationships
        .iter()
        .filter(|r| r.rel_type == "imports")
        .collect();
    assert_eq!(imports.len(), 2, "expected 2 imports: {imports:?}");
    let targets: Vec<&str> = imports.iter().map(|r| r.target_name.as_str()).collect();
    assert!(
        targets.contains(&"stdio.h"),
        "missing stdio.h in {targets:?}"
    );
    assert!(
        targets.contains(&"myheader.h"),
        "missing myheader.h in {targets:?}"
    );
}

#[tokio::test]
async fn c_typedef_extraction() {
    let source = r#"
typedef unsigned long size_t;

typedef struct {
    int x;
    int y;
} Point;
"#
    .trim();
    let extractor = AstExtractor::new();
    let result = extractor.extract(source, &c_context()).await.unwrap();

    let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"size_t"), "missing size_t in {names:?}");
    assert!(names.contains(&"Point"), "missing Point in {names:?}");

    let size_t = result.entities.iter().find(|e| e.name == "size_t").unwrap();
    assert_eq!(size_t.entity_type, "type_alias");

    let point = result.entities.iter().find(|e| e.name == "Point").unwrap();
    assert_eq!(point.entity_type, "type_alias");
}

// ── Language detection tests for new languages ──────────────

#[test]
fn typescript_language_detection() {
    assert_eq!(
        CodeLanguage::from_extension("ts"),
        Some(CodeLanguage::TypeScript)
    );
    assert_eq!(
        CodeLanguage::from_extension("tsx"),
        Some(CodeLanguage::TypeScript)
    );
    assert_eq!(
        CodeLanguage::from_mime("text/typescript"),
        Some(CodeLanguage::TypeScript)
    );
    assert_eq!(
        CodeLanguage::from_mime("application/typescript"),
        Some(CodeLanguage::TypeScript)
    );
    assert_eq!(
        CodeLanguage::from_uri("src/app.ts"),
        Some(CodeLanguage::TypeScript)
    );
}

#[test]
fn javascript_language_detection() {
    assert_eq!(
        CodeLanguage::from_extension("js"),
        Some(CodeLanguage::JavaScript)
    );
    assert_eq!(
        CodeLanguage::from_extension("jsx"),
        Some(CodeLanguage::JavaScript)
    );
    assert_eq!(
        CodeLanguage::from_mime("text/javascript"),
        Some(CodeLanguage::JavaScript)
    );
    assert_eq!(
        CodeLanguage::from_mime("application/javascript"),
        Some(CodeLanguage::JavaScript)
    );
}

#[test]
fn java_language_detection() {
    assert_eq!(
        CodeLanguage::from_extension("java"),
        Some(CodeLanguage::Java)
    );
    assert_eq!(
        CodeLanguage::from_mime("text/x-java"),
        Some(CodeLanguage::Java)
    );
    assert_eq!(
        CodeLanguage::from_mime("text/x-java-source"),
        Some(CodeLanguage::Java)
    );
    assert_eq!(
        CodeLanguage::from_uri("src/Main.java"),
        Some(CodeLanguage::Java)
    );
}

#[test]
fn c_language_detection() {
    assert_eq!(CodeLanguage::from_extension("c"), Some(CodeLanguage::C));
    assert_eq!(CodeLanguage::from_extension("h"), Some(CodeLanguage::C));
    assert_eq!(CodeLanguage::from_mime("text/x-c"), Some(CodeLanguage::C));
    assert_eq!(
        CodeLanguage::from_mime("text/x-csrc"),
        Some(CodeLanguage::C)
    );
    assert_eq!(
        CodeLanguage::from_mime("text/x-chdr"),
        Some(CodeLanguage::C)
    );
    assert_eq!(CodeLanguage::from_uri("lib/util.c"), Some(CodeLanguage::C));
    assert_eq!(
        CodeLanguage::from_uri("include/util.h"),
        Some(CodeLanguage::C)
    );
}
