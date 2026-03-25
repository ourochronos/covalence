//! Job type handlers for the retry queue.
//!
//! Each `JobKind` maps to a handler function that performs the actual
//! work: source processing, chunk extraction, entity summarization,
//! source summary composition, edge synthesis, and batch embedding.

use std::sync::Arc;

use crate::error::Result;
use crate::models::retry_job::RetryJob;
use crate::services::admin::AdminService;
use crate::services::source::SourceService;
use crate::types::ids::SourceId;

use super::fan_in::{try_advance_to_compose, try_advance_to_summarize};

/// Parse a typed payload from a job's JSON, producing a clear error on failure.
fn parse_payload<T: serde::de::DeserializeOwned>(job: &RetryJob) -> Result<T> {
    serde_json::from_value(job.payload.clone())
        .map_err(|e| crate::error::Error::Queue(format!("invalid payload for {:?}: {e}", job.kind)))
}

/// Parse a UUID string, producing a clear queue error on failure.
fn parse_uuid(s: &str, field: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(s)
        .map_err(|e| crate::error::Error::Queue(format!("invalid {field} UUID: {e}")))
}

/// Require a service or return a queue error.
fn require_svc<'a, T>(svc: Option<&'a Arc<T>>, name: &str) -> Result<&'a Arc<T>> {
    svc.ok_or_else(|| crate::error::Error::Queue(format!("{name} not available")))
}

/// Execute a single job based on its kind and typed payload.
pub(crate) async fn execute_job(
    job: &RetryJob,
    source_service: Option<&Arc<SourceService>>,
    admin_service: Option<&Arc<AdminService>>,
) -> Result<()> {
    use crate::models::retry_job::*;

    match job.kind {
        JobKind::ProcessSource => {
            let p: SourcePayload = parse_payload(job)?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            match svc.process_accepted(source_id).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Mark the source as failed so status is queryable.
                    use crate::storage::traits::PipelineRepo;
                    let _ =
                        PipelineRepo::update_source_status(&*svc.repo, source_id, "failed").await;
                    Err(e)
                }
            }
        }
        JobKind::ReprocessSource => {
            let p: SourcePayload = parse_payload(job)?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            require_svc(source_service, "source_service")?
                .reprocess(source_id)
                .await?;
            Ok(())
        }
        JobKind::SynthesizeEdges => {
            let p: SynthesizePayload = parse_payload(job)?;
            let svc = require_svc(admin_service, "admin_service")?;
            let result = svc
                .synthesize_cooccurrence_edges(p.min_cooccurrences, p.max_degree)
                .await?;
            tracing::info!(
                edges_created = result.edges_created,
                candidates = result.candidates_evaluated,
                "edge synthesis job complete"
            );
            Ok(())
        }
        JobKind::ExtractStatements | JobKind::ExtractEntities => {
            tracing::warn!(
                kind = ?job.kind,
                "job kind not yet independently implemented, marking as succeeded"
            );
            Ok(())
        }
        JobKind::ExtractChunk => {
            let p: ExtractChunkPayload = parse_payload(job)?;
            let chunk_uuid = parse_uuid(&p.chunk_id, "chunk_id")?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            extract_single_chunk(svc, chunk_uuid, source_id, job).await
        }
        JobKind::SummarizeEntity => {
            let p: SummarizePayload = parse_payload(job)?;
            let node_id = crate::types::ids::NodeId::from_uuid(parse_uuid(&p.node_id, "node_id")?);
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            summarize_single_entity(svc, node_id, source_id).await
        }
        JobKind::ComposeSourceSummary => {
            let p: SourcePayload = parse_payload(job)?;
            let source_id = SourceId::from_uuid(parse_uuid(&p.source_id, "source_id")?);
            let svc = require_svc(source_service, "source_service")?;
            compose_source_summary_job(svc, source_id).await
        }
        JobKind::EmbedBatch => {
            let p: EmbedBatchPayload = parse_payload(job)?;
            let item_ids: Vec<uuid::Uuid> = p
                .item_ids
                .iter()
                .filter_map(|s| uuid::Uuid::parse_str(s).ok())
                .collect();
            let svc = require_svc(source_service, "source_service")?;
            embed_batch_job(svc, &p.item_table, &item_ids).await
        }
    }
}

/// Generate a semantic summary for a single code entity.
///
/// This is the async-pipeline equivalent of the sequential summary
/// loop in pipeline.rs Stage 7.25. Each entity gets its own job
/// with independent retry.
async fn summarize_single_entity(
    svc: &Arc<SourceService>,
    node_id: crate::types::ids::NodeId,
    source_id: SourceId,
) -> Result<()> {
    use crate::storage::traits::NodeRepo;
    use std::time::Instant;

    let chat = svc
        .chat_backend
        .as_ref()
        .ok_or_else(|| crate::error::Error::Queue("no chat backend for summaries".to_string()))?;

    let node =
        NodeRepo::get(&*svc.repo, node_id)
            .await?
            .ok_or_else(|| crate::error::Error::NotFound {
                entity_type: "node",
                id: node_id.into_uuid().to_string(),
            })?;

    // Skip if already summarized.
    if node
        .properties
        .get("semantic_summary")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
    {
        return Ok(());
    }

    // Build definition pattern for chunk lookup.
    let def_pattern = match node.node_type.as_str() {
        "function" => format!("fn {}", node.canonical_name),
        "struct" => format!("struct {}", node.canonical_name),
        "enum" => format!("enum {}", node.canonical_name),
        "trait" => format!("trait {}", node.canonical_name),
        "impl_block" => format!("impl {}", node.canonical_name),
        "module" => format!("mod {}", node.canonical_name),
        "constant" => format!("const {}", node.canonical_name),
        "macro" => format!("macro_rules! {}", node.canonical_name),
        _ => node.canonical_name.clone(),
    };

    // Find the chunk containing this entity's definition.
    use crate::storage::traits::PipelineRepo;
    let mut chunk_content: Option<String> =
        PipelineRepo::get_chunk_content_for_entity(&*svc.repo, node_id, &def_pattern)
            .await
            .ok()
            .flatten();

    // Fallback: search all chunks from the same source.
    if chunk_content.is_none() {
        chunk_content =
            PipelineRepo::get_chunk_by_source_pattern(&*svc.repo, source_id, &def_pattern)
                .await
                .ok()
                .flatten();
    }

    let raw = chunk_content
        .as_deref()
        .or(node.description.as_deref())
        .unwrap_or(&node.canonical_name);

    if raw.len() < 50 {
        return Ok(()); // Too short to summarize.
    }

    let file_path = node
        .properties
        .get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let prompt = crate::services::prompts::build_summary_prompt(
        &node.canonical_name,
        &node.node_type,
        file_path,
        raw,
    );

    let start = Instant::now();
    let chat_response = chat.chat("", &prompt, false, 0.2).await?;
    let duration_ms = start.elapsed().as_millis() as i64;

    let provider = chat_response.provider;
    let summary = chat_response.text;
    let summary = summary.trim();
    if summary.is_empty() {
        return Ok(());
    }

    // Store summary on the node (properties, description, clear embedding).
    PipelineRepo::update_node_semantic_summary(&*svc.repo, node_id, summary).await?;

    // Record processing metadata on the node.
    PipelineRepo::update_node_processing(
        &*svc.repo,
        node_id,
        &serde_json::json!({
            "model": "haiku",
            "provider": provider,
            "at": chrono::Utc::now().to_rfc3339(),
            "ms": duration_ms,
            "prompt_version": crate::services::prompts::SUMMARY_PROMPT_VERSION,
            "input_chars": raw.len().min(3000),
            "output_chars": summary.len(),
        }),
    )
    .await?;

    tracing::info!(
        node = %node.canonical_name,
        ms = duration_ms,
        "semantic summary generated (async job)"
    );

    // Fan-in: check if all summary jobs for this source are done.
    // If so, auto-enqueue ComposeSourceSummary.
    let nil = uuid::Uuid::nil();
    if source_id.into_uuid() != nil {
        try_advance_to_compose(&svc.repo, source_id).await;
    }

    Ok(())
}

/// Extract entities from a single chunk via the LLM extractor.
async fn extract_single_chunk(
    svc: &Arc<SourceService>,
    chunk_id: uuid::Uuid,
    source_id: SourceId,
    job: &RetryJob,
) -> Result<()> {
    use crate::ingestion::extractor::ExtractionContext;
    use crate::services::pipeline::ExtractionProvenance;
    use crate::storage::traits::{ChunkRepo, SourceRepo};
    use crate::types::ids::ChunkId;
    use std::time::Instant;

    let extractor = svc.extractor.as_ref().ok_or_else(|| {
        crate::error::Error::Queue("no extractor available for extract_chunk".to_string())
    })?;

    let chunk_id_typed = ChunkId::from_uuid(chunk_id);
    let chunk = ChunkRepo::get(&*svc.repo, chunk_id_typed)
        .await?
        .ok_or_else(|| crate::error::Error::NotFound {
            entity_type: "chunk",
            id: chunk_id.to_string(),
        })?;

    let source = SourceRepo::get(&*svc.repo, source_id).await?;
    let context = ExtractionContext {
        source_type: source.as_ref().map(|s| s.source_type.clone()),
        source_uri: source.as_ref().and_then(|s| s.uri.clone()),
        source_title: source.as_ref().and_then(|s| s.title.clone()),
    };
    let source_domain = source.as_ref().and_then(|s| s.domain.clone());

    let start = Instant::now();
    let result = extractor.extract(&chunk.content, &context).await?;
    let duration_ms = start.elapsed().as_millis() as i64;

    // Resolve entities concurrently within this chunk (#169).
    //
    // Bounded concurrency (5) to avoid connection pool exhaustion.
    // Deduplicate by name to prevent advisory lock contention.
    // Errors propagate (fail the chunk for retry) rather than
    // being silently swallowed.
    use futures::stream::{self, TryStreamExt};

    let mut seen_names = std::collections::HashSet::new();
    let filtered_entities: Vec<_> = result
        .entities
        .iter()
        .filter(|e| !crate::services::noise_filter::is_noise_entity(&e.name, &e.entity_type))
        .filter(|e| seen_names.insert(e.name.to_lowercase()))
        .cloned()
        .collect();

    let entity_count = std::sync::atomic::AtomicUsize::new(0);
    let entity_count_ref = &entity_count;

    stream::iter(
        filtered_entities
            .into_iter()
            .map(Ok::<_, crate::error::Error>),
    )
    .try_for_each_concurrent(5, |entity| {
        let svc = Arc::clone(svc);
        let source_domain = source_domain.clone();
        async move {
            let node_id = svc
                .resolve_and_store_entity(
                    &entity,
                    ExtractionProvenance::Chunk(chunk_id_typed),
                    "llm",
                    source_id,
                    source_domain.as_deref(),
                )
                .await?;
            if node_id.is_some() {
                entity_count_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(())
        }
    })
    .await?;

    let entity_count = entity_count.load(std::sync::atomic::Ordering::Relaxed);

    // Mark chunk as processed.
    let ingestion_id = job
        .payload
        .get("ingestion_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    use crate::storage::traits::PipelineRepo;
    PipelineRepo::update_chunk_processing(
        &*svc.repo,
        chunk_id,
        "extraction",
        &serde_json::json!({
            "model": "haiku",
            "at": chrono::Utc::now().to_rfc3339(),
            "ms": duration_ms,
            "entities_found": entity_count,
            "relationships_found": result.relationships.len(),
            "ingestion_id": ingestion_id,
        }),
    )
    .await?;

    tracing::info!(
        chunk_id = %chunk_id,
        entities = entity_count,
        ms = duration_ms,
        "chunk extraction complete (async job)"
    );

    // Fan-in: check if all ExtractChunk jobs for this source are done.
    // If so, enqueue SummarizeEntity jobs for code entities.
    try_advance_to_summarize(&svc.repo, source_id).await;

    Ok(())
}

/// Compose a file-level summary from entity summaries for a code source.
async fn compose_source_summary_job(svc: &Arc<SourceService>, source_id: SourceId) -> Result<()> {
    use crate::ingestion::embedder::truncate_and_validate;
    use crate::ingestion::section_compiler::{SectionSummaryEntry, SourceSummaryInput};
    use crate::storage::traits::SourceRepo;
    use std::time::Instant;

    let summary_compiler = svc.source_summary_compiler.as_ref().ok_or_else(|| {
        crate::error::Error::Queue("no summary compiler for compose_source_summary".to_string())
    })?;

    // Collect entity summaries for this source.
    use crate::storage::traits::PipelineRepo;
    let summaries: Vec<(String, String, String)> =
        PipelineRepo::get_entity_summaries_by_source(&*svc.repo, source_id)
            .await
            .unwrap_or_default();

    if summaries.is_empty() {
        return Ok(());
    }

    let section_entries: Vec<SectionSummaryEntry> = summaries
        .iter()
        .map(|(name, ntype, summary)| SectionSummaryEntry {
            title: format!("{ntype}: {name}"),
            summary: summary.clone(),
        })
        .collect();

    let source = SourceRepo::get(&*svc.repo, source_id).await?;
    let file_name = source
        .as_ref()
        .and_then(|s| s.uri.as_deref())
        .and_then(|u| u.rsplit('/').next())
        .unwrap_or("unknown");

    let start = Instant::now();
    let compilation = summary_compiler
        .compile_source_summary(&SourceSummaryInput {
            section_summaries: section_entries,
            source_title: Some(file_name.to_string()),
        })
        .await?;
    let duration_ms = start.elapsed().as_millis() as i64;

    let provider = compilation.provider;
    let summary = compilation.text;
    let summary = summary.trim();
    if summary.is_empty() {
        return Ok(());
    }

    SourceRepo::update_summary(&*svc.repo, source_id, summary).await?;

    // Re-embed from composed summary.
    if let Some(ref embedder) = svc.embedder {
        if let Ok(vecs) = embedder.embed(&[summary.to_string()]).await {
            if let Some(emb) = vecs.first() {
                if let Ok(t) = truncate_and_validate(emb, svc.table_dims.source, "sources") {
                    let _ = SourceRepo::update_embedding(&*svc.repo, source_id, &t).await;
                }
            }
        }
    }

    // Record processing metadata.
    PipelineRepo::update_source_processing(
        &*svc.repo,
        source_id,
        "compose",
        &serde_json::json!({
            "model": "haiku",
            "provider": provider,
            "at": chrono::Utc::now().to_rfc3339(),
            "ms": duration_ms,
            "entities_composed": summaries.len(),
        }),
    )
    .await?;

    tracing::info!(
        source_id = %source_id,
        entities = summaries.len(),
        ms = duration_ms,
        "source summary composed (async job)"
    );

    // Mark source as complete — unless upstream set it to 'partial'
    // due to failed extraction/summarization jobs (#170).
    PipelineRepo::update_source_status_conditional(&*svc.repo, source_id, "complete", "partial")
        .await?;

    // Wire edge synthesis into the DAG (#173): enqueue after each
    // source completes so co-occurrence edges are built on fresh data
    // rather than relying on the blind 6h timer.
    use crate::storage::traits::JobQueueRepo;
    let synth_payload = serde_json::json!({
        "min_cooccurrences": 1,
        "max_degree": 500,
    });
    let synth_key = format!("synthesize:post:{}", source_id);
    if let Err(e) = JobQueueRepo::enqueue(
        &*svc.repo,
        crate::models::retry_job::JobKind::SynthesizeEdges,
        synth_payload,
        3,
        Some(synth_key.as_str()),
    )
    .await
    {
        tracing::warn!(
            source_id = %source_id,
            error = %e,
            "failed to enqueue post-compose edge synthesis (non-fatal)"
        );
    }

    Ok(())
}

/// Embed a batch of items (nodes or chunks).
async fn embed_batch_job(
    svc: &Arc<SourceService>,
    item_table: &str,
    item_ids: &[uuid::Uuid],
) -> Result<()> {
    use crate::ingestion::embedder::truncate_and_validate;
    use crate::storage::traits::NodeRepo;

    let embedder = svc.embedder.as_ref().ok_or_else(|| {
        crate::error::Error::Queue("no embedder available for embed_batch".to_string())
    })?;

    if item_ids.is_empty() {
        return Ok(());
    }

    match item_table {
        "nodes" => {
            let mut texts = Vec::with_capacity(item_ids.len());
            let mut valid_ids = Vec::with_capacity(item_ids.len());

            for &id in item_ids {
                let node_id = crate::types::ids::NodeId::from_uuid(id);
                if let Ok(Some(node)) = NodeRepo::get(&*svc.repo, node_id).await {
                    // Skip nodes that already have embeddings.
                    use crate::storage::traits::PipelineRepo;
                    let has_emb: bool = PipelineRepo::node_has_embedding(&*svc.repo, id)
                        .await
                        .unwrap_or(true);

                    if has_emb {
                        continue;
                    }

                    let text = match &node.description {
                        Some(desc) if !desc.is_empty() => {
                            format!("{}: {}", node.canonical_name, desc)
                        }
                        _ => node.canonical_name.clone(),
                    };
                    texts.push(text);
                    valid_ids.push(node_id);
                }
            }

            if !texts.is_empty() {
                let embeddings = embedder.embed(&texts).await?;
                for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                    if let Ok(t) = truncate_and_validate(emb, svc.table_dims.node, "nodes") {
                        let _ = NodeRepo::update_embedding(&*svc.repo, *nid, &t).await;
                    }
                }
                tracing::info!(
                    embedded = valid_ids.len(),
                    "batch node embedding complete (async job)"
                );
            }
        }
        _ => {
            tracing::warn!(table = item_table, "embed_batch: unsupported item table");
        }
    }

    Ok(())
}
