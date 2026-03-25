//! Search result enrichment — fetching metadata for fused results.
//!
//! After fusion produces a ranked list of UUIDs with scores, the
//! enrichment step looks up each result's metadata (node name,
//! chunk content, source URI, article body, etc.) so the API can
//! return complete result objects.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::graph::engine::GraphEngine;
use crate::search::fusion::{FusedResult, RelatedEntity};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{
    ArticleRepo, ChunkRepo, NodeRepo, SectionRepo, SourceRepo, StatementRepo,
};
use crate::types::ids::{ArticleId, ChunkId, NodeId, SectionId, StatementId};

use super::super::search_helpers::{
    derive_chunk_name_qualified, kwic_snippet, truncate_with_ellipsis,
};

/// Enrich fused results with entity metadata (Step 8).
///
/// For each result, looks up the corresponding entity (node, chunk,
/// article, source, statement, section) and populates the result's
/// name, content, snippet, source info, confidence, etc.
pub(super) async fn enrich_results(
    fused: &mut [FusedResult],
    repo: &Arc<PgRepo>,
    query: &str,
    snippets: &mut HashMap<Uuid, String>,
    result_types: &HashMap<Uuid, String>,
) {
    for result in fused.iter_mut() {
        result.snippet = snippets.remove(&result.id);
        let rtype = result_types.get(&result.id).map(|s| s.as_str());
        result.result_type = rtype.map(|s| s.to_string());

        match rtype {
            Some("node") => enrich_node(result, repo).await,
            Some("article") => enrich_article(result, repo, query).await,
            Some("source") => enrich_source(result, repo).await,
            Some("statement") => enrich_statement(result, repo, query).await,
            Some("section") => enrich_section(result, repo, query).await,
            _ => enrich_chunk_or_fallback(result, repo, query).await,
        }
    }
}

/// Enrich a node result.
async fn enrich_node(result: &mut FusedResult, repo: &Arc<PgRepo>) {
    if let Ok(Some(node)) = NodeRepo::get(&**repo, NodeId::from_uuid(result.id)).await {
        result.name = Some(node.canonical_name);
        result.entity_type = Some(node.node_type);
        result.content = node.description.clone();
        result.confidence = node.confidence_breakdown.map(|o| o.projected_probability());
    }
}

/// Enrich an article result.
async fn enrich_article(result: &mut FusedResult, repo: &Arc<PgRepo>, query: &str) {
    if let Ok(Some(article)) = ArticleRepo::get(&**repo, ArticleId::from_uuid(result.id)).await {
        result.name = Some(article.title);
        result.entity_type = Some("article".to_string());
        result.content = Some(article.body.clone());
        result.confidence = article
            .confidence_breakdown
            .map(|o| o.projected_probability());
        if result.snippet.is_none() {
            result.snippet = Some(kwic_snippet(&article.body, query, 300));
        }
    }
}

/// Enrich a source result.
async fn enrich_source(result: &mut FusedResult, repo: &Arc<PgRepo>) {
    if let Ok(Some(source)) =
        SourceRepo::get(&**repo, crate::types::ids::SourceId::from_uuid(result.id)).await
    {
        result.name = source.title.clone();
        result.entity_type = Some("source".to_string());
        result.source_uri = source.uri;
        result.source_title = source.title;
        result.source_type = Some(source.source_type.clone());
        result.source_domain = source.domain.clone();
        result.created_at = Some(source.ingested_at.to_rfc3339());
        // Use truncated raw content for snippet.
        if let Some(ref raw) = source.raw_content {
            result.content = Some(truncate_with_ellipsis(raw, 500));
        }
    }
}

/// Enrich a statement result.
async fn enrich_statement(result: &mut FusedResult, repo: &Arc<PgRepo>, query: &str) {
    if let Ok(Some(stmt)) = StatementRepo::get(&**repo, StatementId::from_uuid(result.id)).await {
        result.content = Some(stmt.content.clone());
        result.entity_type = Some("statement".to_string());
        result.confidence = Some(stmt.confidence);
        result.created_at = Some(stmt.created_at.to_rfc3339());
        // Look up source for URI/title.
        if let Ok(Some(source)) = SourceRepo::get(&**repo, stmt.source_id).await {
            result.source_uri = source.uri;
            result.source_title = source.title.clone();
            result.source_type = Some(source.source_type.clone());
            result.source_domain = source.domain.clone();
        }
        // Use the statement content as name (truncated).
        result.name = Some(truncate_with_ellipsis(&stmt.content, 80));
        // Content-based snippet fallback.
        if result.snippet.is_none() {
            result.snippet = Some(kwic_snippet(&stmt.content, query, 200));
        }
    }
}

/// Enrich a section result.
async fn enrich_section(result: &mut FusedResult, repo: &Arc<PgRepo>, query: &str) {
    if let Ok(Some(sect)) = SectionRepo::get(&**repo, SectionId::from_uuid(result.id)).await {
        result.name = Some(sect.title.clone());
        result.entity_type = Some("section".to_string());
        result.content = Some(sect.summary.clone());
        result.created_at = Some(sect.created_at.to_rfc3339());
        // Look up source for URI/title.
        if let Ok(Some(source)) = SourceRepo::get(&**repo, sect.source_id).await {
            result.source_uri = source.uri;
            result.source_title = source.title.clone();
            result.source_type = Some(source.source_type.clone());
            result.source_domain = source.domain.clone();
        }
        // Content-based snippet fallback.
        if result.snippet.is_none() {
            result.snippet = Some(kwic_snippet(&sect.summary, query, 300));
        }
    }
}

/// Enrich a chunk result, or fall back to source/node lookup.
async fn enrich_chunk_or_fallback(result: &mut FusedResult, repo: &Arc<PgRepo>, query: &str) {
    // Chunk or unknown — try chunk lookup for source_uri and
    // content, then node lookup for compat.
    if let Ok(Some(chunk)) = ChunkRepo::get(&**repo, ChunkId::from_uuid(result.id)).await {
        result.content = Some(chunk.content.clone());
        result.entity_type = Some("chunk".to_string());
        // Look up source first so we can qualify generic chunk
        // headings with the source title.
        let src_title = if let Ok(Some(source)) = SourceRepo::get(&**repo, chunk.source_id).await {
            result.source_uri = source.uri;
            result.source_title = source.title.clone();
            result.source_type = Some(source.source_type.clone());
            result.source_domain = source.domain.clone();
            result.created_at = Some(source.ingested_at.to_rfc3339());
            source.title
        } else {
            None
        };
        result.name = Some(derive_chunk_name_qualified(
            &chunk.content,
            src_title.as_deref(),
        ));
        // Content-based snippet fallback: if no lexical
        // snippet exists, extract a keyword-in-context
        // window around query terms. Falls back to the
        // first 200 chars if no terms match.
        if result.snippet.is_none() {
            result.snippet = Some(kwic_snippet(&chunk.content, query, 200));
        }
        // Parent-context injection: for paragraph-level
        // chunks, prepend truncated parent content to
        // the snippet (avoids a second chunk fetch).
        if chunk.level == "paragraph" {
            if let Some(parent_id) = chunk.parent_chunk_id {
                if let Ok(Some(parent)) = ChunkRepo::get(&**repo, parent_id).await {
                    let parent_ctx = truncate_with_ellipsis(&parent.content, 200);
                    let prefix = format!("[{}: {}]", parent.level, parent_ctx);
                    result.snippet = Some(match result.snippet.take() {
                        Some(s) => format!("{} {}", prefix, s),
                        None => prefix,
                    });
                }
            }
        }
    }
    // If still no entity_type, try source lookup (vector dimension
    // produces source results but result_type may not propagate
    // through fusion).
    if result.entity_type.is_none() {
        if let Ok(Some(source)) =
            SourceRepo::get(&**repo, crate::types::ids::SourceId::from_uuid(result.id)).await
        {
            result.name = source.title.clone();
            result.entity_type = Some("source".to_string());
            result.source_uri = source.uri;
            result.source_title = source.title;
            result.source_type = Some(source.source_type.clone());
            result.source_domain = source.domain.clone();
            result.created_at = Some(source.ingested_at.to_rfc3339());
            if let Some(ref raw) = source.raw_content {
                result.content = Some(truncate_with_ellipsis(raw, 500));
            }
        }
    }
    if let Ok(Some(node)) = NodeRepo::get(&**repo, NodeId::from_uuid(result.id)).await {
        result.name = Some(node.canonical_name);
        result.entity_type = Some(node.node_type);
        // Only set content from node if not already set from chunk.
        if result.content.is_none() {
            result.content = node.description.clone();
        }
        result.confidence = node.confidence_breakdown.map(|o| o.projected_probability());
    }
}

/// Enrich node-type results with 1-hop graph neighbors (Step 8a).
///
/// Attaches relationship context using the graph engine trait
/// (no DB queries) for fast enrichment.
pub(super) async fn enrich_graph_context(
    fused: &mut [FusedResult],
    graph_engine: &Arc<dyn GraphEngine>,
) {
    const MAX_NEIGHBORS: usize = 10;
    for result in fused.iter_mut() {
        if result
            .entity_type
            .as_deref()
            .is_none_or(|t| matches!(t, "chunk" | "article" | "source" | "statement" | "section"))
        {
            continue;
        }
        let mut related = Vec::new();
        // Outgoing neighbors
        if let Ok(out_neighbors) = graph_engine.neighbors_out(result.id).await {
            for n in out_neighbors {
                if n.is_synthetic {
                    continue;
                }
                related.push(RelatedEntity {
                    name: n.name,
                    rel_type: n.rel_type,
                    direction: "outgoing".to_string(),
                });
                if related.len() >= MAX_NEIGHBORS {
                    break;
                }
            }
        }
        // Incoming neighbors (if room)
        if related.len() < MAX_NEIGHBORS {
            if let Ok(in_neighbors) = graph_engine.neighbors_in(result.id).await {
                for n in in_neighbors {
                    if n.is_synthetic {
                        continue;
                    }
                    related.push(RelatedEntity {
                        name: n.name,
                        rel_type: n.rel_type,
                        direction: "incoming".to_string(),
                    });
                    if related.len() >= MAX_NEIGHBORS {
                        break;
                    }
                }
            }
        }
        if !related.is_empty() {
            result.graph_context = Some(related);
        }
    }
}
