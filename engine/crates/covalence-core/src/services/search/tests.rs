//! Tests for search service components.

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use crate::services::search::filters::source_layer_from_uri;

    #[test]
    fn source_layer_spec() {
        assert_eq!(
            source_layer_from_uri("file://spec/01-architecture.md"),
            Some("spec")
        );
        assert_eq!(
            source_layer_from_uri("file://spec/05-ingestion.md"),
            Some("spec")
        );
    }

    #[test]
    fn source_layer_design() {
        assert_eq!(
            source_layer_from_uri("file://docs/adr/0001-hybrid-property-graph.md"),
            Some("design")
        );
    }

    #[test]
    fn source_layer_code() {
        assert_eq!(
            source_layer_from_uri("file://engine/crates/covalence-core/src/search/fusion.rs"),
            Some("code")
        );
        assert_eq!(
            source_layer_from_uri("file://cli/cmd/search.go"),
            Some("code")
        );
    }

    #[test]
    fn source_layer_research() {
        assert_eq!(
            source_layer_from_uri("https://arxiv.org/html/2501.00309"),
            Some("research")
        );
        assert_eq!(
            source_layer_from_uri("http://example.com/paper.pdf"),
            Some("research")
        );
    }

    #[test]
    fn source_layer_unknown() {
        assert_eq!(source_layer_from_uri("file://README.md"), None);
        assert_eq!(source_layer_from_uri("covalence://internal"), None);
    }
}
