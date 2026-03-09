# ADR-0008: Markdown as Canonical Intermediate Format

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/05-ingestion.md

## Context

The ingestion pipeline accepts multiple formats (PDF, HTML, DOCX, plain text, code, conversations). A canonical intermediate representation is needed before chunking.

## Decision

All parsed output is normalized to extended Markdown before chunking. Document metadata goes in YAML frontmatter. Headings become `#`/`##`/`###`, tables become pipe tables, code becomes fenced blocks, conversation turns become blockquotes.

## Consequences

### Positive

- LLMs are natively trained on Markdown — headings are semantically understood
- Preserves hierarchy without token-heavy markup (unlike HTML)
- Tables render cleanly in pipe syntax
- Single chunking algorithm works on all formats
- Human-readable intermediate representation

### Negative

- Some structural information lost in conversion (e.g., complex HTML layouts)
- YAML frontmatter adds tokens that may not be relevant for embedding
- Binary content (images) reduced to placeholder captions

## Alternatives Considered

- **HTML as intermediate:** Token-heavy, less LLM-friendly
- **Custom AST:** More precise but requires custom tooling for everything
- **Plain text:** Loses all structural information
