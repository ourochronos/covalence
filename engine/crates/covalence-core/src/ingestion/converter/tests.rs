//! Tests for the converter module.

use super::*;
use crate::ingestion::utils::decode_html_entities;

#[tokio::test]
async fn markdown_passthrough() {
    let converter = MarkdownConverter;
    let input = b"# Hello\n\nWorld";
    let result = converter
        .convert(input, "text/markdown")
        .await
        .expect("conversion should succeed");
    assert_eq!(result, "# Hello\n\nWorld");
}

#[tokio::test]
async fn markdown_invalid_utf8() {
    let converter = MarkdownConverter;
    // Invalid UTF-8: 0xFF is not a valid start byte.
    let input: &[u8] = &[0x48, 0x65, 0x6C, 0xFF, 0x6F];
    let result = converter
        .convert(input, "text/markdown")
        .await
        .expect("conversion should succeed");
    // The invalid byte should be replaced with U+FFFD.
    assert!(result.contains('\u{FFFD}'));
    assert!(result.starts_with("Hel"));
}

#[tokio::test]
async fn plain_text_wrapping() {
    let converter = PlainTextConverter;
    let input = b"Just some text.";
    let result = converter
        .convert(input, "text/plain")
        .await
        .expect("conversion should succeed");
    assert!(result.starts_with("# Untitled Document"));
    assert!(result.contains("Just some text."));
}

#[tokio::test]
async fn html_strips_tags() {
    let converter = HtmlConverter;
    let input = b"<p>Hello <b>world</b></p>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("conversion should succeed");
    assert!(result.contains("Hello world"));
    assert!(!result.contains("<p>"));
    assert!(!result.contains("<b>"));
}

#[tokio::test]
async fn html_converts_headings() {
    let converter = HtmlConverter;
    let input = b"<h1>Title</h1><h2>Subtitle</h2>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("conversion should succeed");
    assert!(result.contains("# Title"));
    assert!(result.contains("## Subtitle"));
}

#[tokio::test]
async fn html_converts_lists() {
    let converter = HtmlConverter;
    let input = b"<ul><li>One</li><li>Two</li></ul>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("conversion should succeed");
    assert!(result.contains("- One"));
    assert!(result.contains("- Two"));
}

#[tokio::test]
async fn html_removes_script_and_style() {
    let converter = HtmlConverter;
    let input = b"<p>Hello</p><script>alert('xss')</script><p>World</p>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("conversion should succeed");
    assert!(!result.contains("alert"));
    assert!(!result.contains("script"));
    assert!(result.contains("Hello"));
    assert!(result.contains("World"));
}

#[tokio::test]
async fn html_decodes_entities() {
    let converter = HtmlConverter;
    let input = b"<p>A &amp; B &lt; C &gt; D</p>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("conversion should succeed");
    assert!(result.contains("A & B < C > D"));
}

#[tokio::test]
async fn html_handles_br_tags() {
    let converter = HtmlConverter;
    let input = b"Line one<br>Line two<br/>Line three";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("conversion should succeed");
    assert!(result.contains("Line one\nLine two\nLine three"));
}

#[tokio::test]
async fn registry_dispatches_by_content_type() {
    let registry = ConverterRegistry::new();

    // Markdown
    let md = registry
        .convert(b"# Test", "text/markdown")
        .await
        .expect("markdown should work");
    assert_eq!(md, "# Test");

    // Plain text
    let txt = registry
        .convert(b"Hello", "text/plain")
        .await
        .expect("plain text should work");
    assert!(txt.contains("# Untitled Document"));
    assert!(txt.contains("Hello"));

    // HTML
    let html = registry
        .convert(b"<p>Hi</p>", "text/html")
        .await
        .expect("html should work");
    assert!(html.contains("Hi"));
    assert!(!html.contains("<p>"));
}

#[tokio::test]
async fn registry_unknown_type_error() {
    let registry = ConverterRegistry::new();
    let result = registry.convert(b"data", "application/pdf").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("no converter registered"));
    assert!(err.contains("application/pdf"));
}

#[tokio::test]
async fn registry_strips_charset_parameter() {
    let registry = ConverterRegistry::new();
    let result = registry
        .convert(b"<p>Hello</p>", "text/html; charset=utf-8")
        .await
        .expect("should handle content type with params");
    assert!(result.contains("Hello"));
}

#[tokio::test]
async fn registry_custom_converter() {
    /// Test converter that uppercases content.
    struct UpperConverter;

    #[async_trait::async_trait]
    impl SourceConverter for UpperConverter {
        async fn convert(
            &self,
            content: &[u8],
            _content_type: &str,
        ) -> crate::error::Result<String> {
            let text = String::from_utf8_lossy(content);
            Ok(text.to_uppercase())
        }

        fn supported_types(&self) -> &[&str] {
            &["text/x-upper"]
        }
    }

    let mut registry = ConverterRegistry::new();
    registry.register(Box::new(UpperConverter));

    let result = registry
        .convert(b"hello", "text/x-upper")
        .await
        .expect("custom converter should work");
    assert_eq!(result, "HELLO");
}

#[tokio::test]
async fn registry_default_trait() {
    // Ensure Default impl works.
    let registry = ConverterRegistry::default();
    let result = registry
        .convert(b"test", "text/plain")
        .await
        .expect("default registry should work");
    assert!(result.contains("test"));
}

#[test]
fn strip_html_empty_input() {
    assert_eq!(html::strip_html(""), "");
}

#[test]
fn strip_html_no_tags() {
    assert_eq!(html::strip_html("Just plain text"), "Just plain text");
}

#[test]
fn strip_html_nested_tags() {
    let result = html::strip_html("<div><p>Nested <em>content</em></p></div>");
    assert!(result.contains("Nested content"));
}

#[test]
fn collapse_newlines_works() {
    let input = "a\n\n\n\n\nb";
    let result = html::collapse_newlines(input);
    assert_eq!(result, "a\n\nb");
}

#[test]
fn decode_entities_all() {
    let input = "&amp; &lt; &gt; &quot; &nbsp; &#39;";
    let result = decode_html_entities(input);
    // &nbsp; decodes to a regular space, so there are two
    // spaces between the quote and apostrophe.
    assert_eq!(result, "& < > \"   '");
}

#[test]
fn remove_block_elements_case_insensitive() {
    let input = "<SCRIPT>bad</SCRIPT>good";
    // Use strip_html which calls remove_block_elements internally.
    let result = html::strip_html(input);
    assert_eq!(result, "good");
}

#[test]
fn supported_types_markdown() {
    let c = MarkdownConverter;
    assert_eq!(c.supported_types(), &["text/markdown", "text/x-markdown"]);
}

#[test]
fn supported_types_plain() {
    let c = PlainTextConverter;
    assert_eq!(c.supported_types(), &["text/plain"]);
}

#[test]
fn supported_types_html() {
    let c = HtmlConverter;
    assert_eq!(c.supported_types(), &["text/html", "application/xhtml+xml"]);
}

#[test]
fn supported_types_readerlm() {
    let c = ReaderLmConverter::new("http://localhost:8432".into());
    assert_eq!(c.supported_types(), &["text/html", "application/xhtml+xml"]);
}

#[tokio::test]
async fn readerlm_fallback_when_sidecar_down() {
    // Point at a non-existent sidecar — should fall back to
    // HtmlConverter's strip_html.
    let converter = ReaderLmConverter::new("http://127.0.0.1:1".into());
    let input = b"<p>Hello <b>world</b></p>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("should fall back gracefully");
    assert!(result.contains("Hello world"));
    assert!(!result.contains("<p>"));
}

#[tokio::test]
async fn readerlm_fallback_preserves_structure() {
    let converter = ReaderLmConverter::new("http://127.0.0.1:1".into());
    let input = b"<h1>Title</h1><ul><li>One</li><li>Two</li></ul>";
    let result = converter
        .convert(input, "text/html")
        .await
        .expect("fallback should preserve structure");
    assert!(result.contains("# Title"));
    assert!(result.contains("- One"));
    assert!(result.contains("- Two"));
}

#[tokio::test]
async fn registry_readerlm_takes_priority() {
    // ReaderLmConverter registered front should be dispatched
    // for text/html instead of HtmlConverter.
    let mut registry = ConverterRegistry::new();
    registry.register_front(Box::new(ReaderLmConverter::new(
        "http://127.0.0.1:1".into(),
    )));

    // Even though the sidecar is down, ReaderLmConverter's
    // fallback (strip_html) produces the same result as
    // HtmlConverter — proving it was dispatched first.
    let result = registry
        .convert(b"<p>Test</p>", "text/html")
        .await
        .expect("should work via readerlm fallback");
    assert!(result.contains("Test"));
}

#[tokio::test]
async fn register_front_takes_priority() {
    /// Converter that always returns "FRONT" for text/plain.
    struct FrontConverter;

    #[async_trait::async_trait]
    impl SourceConverter for FrontConverter {
        async fn convert(
            &self,
            _content: &[u8],
            _content_type: &str,
        ) -> crate::error::Result<String> {
            Ok("FRONT".to_string())
        }

        fn supported_types(&self) -> &[&str] {
            &["text/plain"]
        }
    }

    let mut registry = ConverterRegistry::new();
    registry.register_front(Box::new(FrontConverter));

    let result = registry
        .convert(b"hello", "text/plain")
        .await
        .expect("front converter should win");
    assert_eq!(result, "FRONT");
}

// --- Table linearization tests ---

#[test]
fn linearize_basic_table() {
    let input = "\
| Model | Developer | Year |
|-------|-----------|------|
| GraphRAG | Microsoft Research | 2024 |
| HDBSCAN | Campello et al. | 2013 |";
    let result = linearize_tables(input);
    assert_eq!(
        result,
        "Model: GraphRAG, Developer: Microsoft Research, Year: 2024.\n\
         Model: HDBSCAN, Developer: Campello et al., Year: 2013."
    );
}

#[test]
fn linearize_preserves_non_table_content() {
    let input = "\
# Heading

Some paragraph text.

| A | B |
|---|---|
| 1 | 2 |

More text after.";
    let result = linearize_tables(input);
    assert!(result.contains("# Heading"));
    assert!(result.contains("Some paragraph text."));
    assert!(result.contains("A: 1, B: 2."));
    assert!(result.contains("More text after."));
}

#[test]
fn linearize_empty_cells_skipped() {
    let input = "\
| Name | Value |
|------|-------|
| foo |  |
| bar | 42 |";
    let result = linearize_tables(input);
    assert!(result.contains("Name: foo."));
    assert!(result.contains("Name: bar, Value: 42."));
}

#[test]
fn linearize_no_table() {
    let input = "Just plain markdown\nwith no tables.";
    let result = linearize_tables(input);
    assert_eq!(result, input);
}

#[test]
fn linearize_two_column_key_value() {
    let input = "\
| Key | Value |
|-----|-------|
| name | Covalence |
| version | 0.1.0 |
| language | Rust |";
    let result = linearize_tables(input);
    assert!(result.contains("Key: name, Value: Covalence."));
    assert!(result.contains("Key: version, Value: 0.1.0."));
    assert!(result.contains("Key: language, Value: Rust."));
}

#[test]
fn linearize_adjacent_tables() {
    let input = "\
| A | B |
|---|---|
| 1 | 2 |

| X | Y |
|---|---|
| 3 | 4 |";
    let result = linearize_tables(input);
    assert!(result.contains("A: 1, B: 2."));
    assert!(result.contains("X: 3, Y: 4."));
}

#[test]
fn linearize_colon_separator() {
    // Separator with colons for alignment.
    let input = "\
| Left | Center | Right |
|:-----|:------:|------:|
| a | b | c |";
    let result = linearize_tables(input);
    assert!(result.contains("Left: a, Center: b, Right: c."));
}

#[test]
fn is_separator_row_positive() {
    assert!(table::is_separator_row("|---|---|"));
    assert!(table::is_separator_row("| --- | --- |"));
    assert!(table::is_separator_row("|:---|:---:|---:|"));
    assert!(table::is_separator_row("| :--- | :---: | ---: |"));
}

#[test]
fn is_separator_row_negative() {
    assert!(!table::is_separator_row("| a | b |"));
    assert!(!table::is_separator_row(""));
    assert!(!table::is_separator_row("| | |"));
}

#[test]
fn parse_table_row_strips_pipes() {
    let cells = table::parse_table_row("| foo | bar | baz |");
    assert_eq!(cells, vec!["foo", "bar", "baz"]);
}

#[test]
fn linearize_large_table() {
    let mut input = String::from("| ID | Name |\n|---|---|\n");
    for i in 0..50 {
        input.push_str(&format!("| {i} | item-{i} |\n"));
    }
    let result = linearize_tables(&input);
    assert!(result.contains("ID: 0, Name: item-0."));
    assert!(result.contains("ID: 49, Name: item-49."));
    // No pipe characters should remain in linearized output.
    for line in result.lines() {
        if line.contains("ID:") {
            assert!(!line.contains('|'));
        }
    }
}

#[tokio::test]
async fn registry_linearizes_tables_in_markdown() {
    let registry = ConverterRegistry::new();
    let md_with_table = b"# Test\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
    let result = registry
        .convert(md_with_table, "text/markdown")
        .await
        .expect("should convert");
    assert!(result.contains("A: 1, B: 2."));
    assert!(result.contains("# Test"));
}

// --- HTML windowing tests ---

#[test]
fn split_html_small_passthrough() {
    let html = "<p>Hello world</p>";
    let windows = html::split_html_windows(html, 50_000);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0], html);
}

#[test]
fn split_html_at_structural_boundaries() {
    let html = format!(
        "<div>{}</div><div>{}</div><div>{}</div>",
        "a".repeat(200),
        "b".repeat(200),
        "c".repeat(200),
    );
    let windows = html::split_html_windows(&html, 300);
    assert!(
        windows.len() >= 2,
        "expected split, got {} windows",
        windows.len()
    );
}

#[test]
fn split_html_no_structural_tags() {
    let html = "x".repeat(200);
    let windows = html::split_html_windows(&html, 100);
    assert!(windows.len() >= 2);
    // Each window should be <= max_chars
    for w in &windows {
        assert!(w.len() <= 100);
    }
}

// --- Conversion quality tests ---

#[test]
fn quality_check_empty_fails() {
    assert_eq!(check_conversion_quality(""), Some("empty output"));
    assert_eq!(check_conversion_quality("   "), Some("empty output"));
}

#[test]
fn quality_check_short_passes() {
    // Short output is OK for short input
    assert_eq!(check_conversion_quality("Hello world"), None);
}

#[test]
fn low_quality_large_input_tiny_output() {
    assert!(is_low_quality_conversion(5000, "Hello"));
}

#[test]
fn low_quality_small_input_ok() {
    assert!(!is_low_quality_conversion(100, "Hello"));
}

#[test]
fn low_quality_adequate_output() {
    let output = "This is a sufficiently long output with many words to \
                  pass the quality check for conversion results.";
    assert!(!is_low_quality_conversion(5000, output));
}

// --- PDF converter tests ---

#[test]
fn pdf_converter_supported_types() {
    let conv = PdfConverter::new("http://localhost:9999".into());
    assert_eq!(conv.supported_types(), &["application/pdf"]);
}

#[test]
fn pdf_converter_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PdfConverter>();
}

#[tokio::test]
async fn registry_rejects_pdf_without_converter() {
    let registry = ConverterRegistry::new();
    let result = registry.convert(b"%PDF-1.4", "application/pdf").await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("no converter registered"));
}

// --- Boilerplate removal tests ---

#[test]
fn strip_boilerplate_removes_nav() {
    let html_input = "<body><nav><a href='/'>Home</a></nav><p>Article content</p></body>";
    let result = html::strip_boilerplate(html_input);
    assert!(!result.contains("Home"));
    assert!(result.contains("Article content"));
}

#[test]
fn strip_boilerplate_removes_header_footer() {
    let html_input = "<header>Site Header</header>\
                     <main>Content</main>\
                     <footer>Copyright 2024</footer>";
    let result = html::strip_boilerplate(html_input);
    assert!(!result.contains("Site Header"));
    assert!(!result.contains("Copyright"));
    assert!(result.contains("Content"));
}

#[test]
fn strip_boilerplate_removes_aside() {
    let html_input = "<article>Main text</article><aside>Related articles</aside>";
    let result = html::strip_boilerplate(html_input);
    assert!(!result.contains("Related articles"));
    assert!(result.contains("Main text"));
}

#[test]
fn strip_boilerplate_removes_cookie_div() {
    let html_input = "<div class='cookie-consent'>Accept cookies</div>\
                     <p>Real content here</p>";
    let result = html::strip_boilerplate(html_input);
    assert!(!result.contains("Accept cookies"));
    assert!(result.contains("Real content here"));
}

#[test]
fn strip_boilerplate_preserves_clean_html() {
    let html_input = "<h1>Title</h1><p>Paragraph one.</p><p>Paragraph two.</p>";
    let result = html::strip_boilerplate(html_input);
    assert_eq!(result, html_input);
}

#[test]
fn strip_boilerplate_removes_noscript() {
    let html_input = "<p>Content</p><noscript>Enable JavaScript</noscript>";
    let result = html::strip_boilerplate(html_input);
    assert!(!result.contains("Enable JavaScript"));
    assert!(result.contains("Content"));
}

#[tokio::test]
async fn html_converter_strips_boilerplate() {
    let converter = HtmlConverter;
    let input = b"<nav>Menu</nav><h1>Title</h1><p>Content</p><footer>Footer</footer>";
    let result = converter.convert(input, "text/html").await.unwrap();
    assert!(!result.contains("Menu"));
    assert!(!result.contains("Footer"));
    assert!(result.contains("Title"));
    assert!(result.contains("Content"));
}
