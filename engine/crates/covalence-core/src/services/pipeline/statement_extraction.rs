//! Statement-level entity extraction.
//!
//! Extracts entities and relationships from stored statements for a
//! given source. Statements are self-contained text, so they work as
//! extraction input without windowing or batching. Creates extraction
//! records with `statement_id` provenance.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::ingestion::embedder::truncate_and_validate;
use crate::ingestion::extractor::ExtractionContext;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::storage::traits::{EdgeRepo, ExtractionRepo, NodeRepo};
use crate::types::ids::{NodeId, SourceId};

use super::super::noise_filter::is_noise_entity;
use super::super::source::SourceService;
use super::types::ExtractionProvenance;

impl SourceService {
    /// Run entity extraction on stored statements.
    ///
    /// Reuses the existing `Extractor` trait — statements are
    /// self-contained text, so they work as extraction input without
    /// windowing or batching. Creates extraction records with
    /// `statement_id` provenance.
    pub(crate) async fn extract_entities_from_statements(
        &self,
        source_id: SourceId,
        source_domain: Option<&str>,
    ) -> Result<usize> {
        let extractor = match self.extractor.as_ref() {
            Some(e) => e,
            None => return Ok(0),
        };

        let statements =
            crate::storage::traits::StatementRepo::list_by_source(&*self.repo, source_id).await?;
        if statements.is_empty() {
            return Ok(0);
        }

        let extraction_context = Arc::new(ExtractionContext {
            source_type: None,
            source_uri: None,
            source_title: None,
        });

        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));
        let mut entity_count = 0usize;
        let mut name_to_node: std::collections::HashMap<String, NodeId> =
            std::collections::HashMap::new();

        // Extract from each statement (they're already self-contained).
        let batch_futures: Vec<_> = statements
            .iter()
            .filter(|s| !s.is_evicted)
            .map(|stmt| {
                let sem = Arc::clone(&semaphore);
                let ext = Arc::clone(extractor);
                let ctx = Arc::clone(&extraction_context);
                let content = stmt.content.clone();
                let stmt_id = stmt.id;
                async move {
                    let _permit = sem
                        .acquire()
                        .await
                        .map_err(|_| Error::Ingestion("extraction semaphore closed".to_string()))?;
                    let result = ext.extract(&content, &ctx).await?;
                    Ok::<_, Error>((stmt_id, result))
                }
            })
            .collect();

        let results = futures::future::join_all(batch_futures).await;

        for result in results {
            let (stmt_id, extraction) = match result {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "statement entity extraction failed, skipping"
                    );
                    continue;
                }
            };

            for entity in &extraction.entities {
                if is_noise_entity(&entity.name, &entity.entity_type) {
                    continue;
                }
                let node_id = self
                    .resolve_and_store_entity(
                        entity,
                        ExtractionProvenance::Statement(stmt_id),
                        "llm_statement",
                        source_id,
                        source_domain,
                    )
                    .await?;
                let Some(node_id) = node_id else {
                    continue; // Deferred to Tier 5
                };
                name_to_node.insert(entity.name.to_lowercase(), node_id);
                entity_count += 1;
            }

            for rel in &extraction.relationships {
                let src_key = rel.source_name.to_lowercase();
                let tgt_key = rel.target_name.to_lowercase();

                let source_node = match name_to_node.get(&src_key) {
                    Some(&id) => id,
                    None => continue,
                };
                let target_node = match name_to_node.get(&tgt_key) {
                    Some(&id) => id,
                    None => continue,
                };

                if source_node == target_node {
                    continue;
                }

                let resolved_rel_type = if let Some(ref rtr) = self.rel_type_resolver {
                    rtr.resolve_rel_type(&rel.rel_type).await?
                } else {
                    rel.rel_type.clone()
                };

                let mut edge = Edge::new(source_node, target_node, resolved_rel_type);
                edge.confidence = rel.confidence;
                EdgeRepo::create(&*self.repo, &edge).await?;
                self.check_and_invalidate_conflicts(&edge).await?;

                let ext_record = Extraction::from_statement(
                    stmt_id,
                    ExtractedEntityType::Edge,
                    edge.id.into_uuid(),
                    "llm_statement".to_string(),
                    rel.confidence,
                );
                ExtractionRepo::create(&*self.repo, &ext_record).await?;
            }
        }

        // Embed new node descriptions.
        if !name_to_node.is_empty() {
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();
                let has_embedding_ids: Vec<uuid::Uuid> = sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT id FROM nodes \
                     WHERE id = ANY($1) AND embedding IS NOT NULL",
                )
                .bind(node_ids.iter().map(|n| n.into_uuid()).collect::<Vec<_>>())
                .fetch_all(self.repo.pool())
                .await
                .unwrap_or_default();

                let mut texts = Vec::new();
                let mut valid_ids = Vec::new();
                for &nid in &node_ids {
                    if has_embedding_ids.contains(&nid.into_uuid()) {
                        continue;
                    }
                    if let Some(node) = NodeRepo::get(&*self.repo, nid).await? {
                        let text = match &node.description {
                            Some(desc) if !desc.is_empty() => {
                                format!("{}: {}", node.canonical_name, desc)
                            }
                            _ => node.canonical_name.clone(),
                        };
                        texts.push(text);
                        valid_ids.push(nid);
                    }
                }

                if !texts.is_empty() {
                    if let Ok(embeddings) = embedder.embed(&texts).await {
                        for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                            if let Ok(truncated) =
                                truncate_and_validate(emb, self.table_dims.node, "nodes")
                            {
                                let _ =
                                    NodeRepo::update_embedding(&*self.repo, *nid, &truncated).await;
                            }
                        }
                    }
                }
            }
        }

        tracing::info!(
            source_id = %source_id,
            entities = entity_count,
            "entity extraction from statements complete"
        );

        Ok(entity_count)
    }
}
