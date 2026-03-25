//! Tests for the AST extractor module.

use super::*;
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
