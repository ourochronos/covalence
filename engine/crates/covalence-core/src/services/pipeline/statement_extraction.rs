//! Statement-level entity extraction.
//!
//! Extracts entities and relationships from stored statements for a
//! given source. Uses a four-phase approach:
//!
//! 0. **Novelty gate** (#195) — skip near-paraphrase statements
//!    using embedding cosine similarity (greedy furthest-first).
//! 1. **Extract** — concurrent LLM extraction from novel statements.
//! 2. **Dedup** — accumulate entities across all statements and
//!    resolve each unique entity name once (not once per statement).
//! 3. **Link** — process relationships per-statement using the
//!    resolved `name_to_node` map.
//!
//! Cross-statement dedup (#196) prevents redundant resolution:
//! if "Rust" is extracted from 50 statements, it's resolved once
//! instead of 50 times (50 advisory locks + potential embedding
//! API calls). Each source statement still gets an extraction
//! provenance record linking it to the resolved node.
//!
//! Novelty gating (#195, Lesson 1 reconciliation) prevents redundant
//! LLM extraction calls: statements whose embedding is >= the
//! threshold similarity to an already-selected statement are skipped
//! entirely. The 0.92 default only skips near-paraphrases.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::ingestion::embedder::truncate_and_validate;
use crate::ingestion::extractor::{ExtractedEntity, ExtractionContext};
use crate::ingestion::statement_cluster::cosine_similarity_f32;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::storage::traits::{EdgeRepo, ExtractionRepo, NodeRepo};
use crate::types::ids::{NodeId, SourceId, StatementId};

use super::super::noise_filter::is_noise_entity;
use super::super::source::SourceService;
use super::types::ExtractionProvenance;

/// Select statements whose embeddings are sufficiently novel for
/// entity extraction.
///
/// Uses greedy furthest-first ordering: start with the first
/// embedded statement, then for each remaining statement compute
/// its max cosine similarity to any already-selected statement.
/// If that max is below `threshold`, the statement introduces
/// novel content worth extracting.
///
/// Statements without embeddings (None) are always included — we
/// can't assess novelty without a vector.
///
/// Returns indices into the input slice of statements to extract.
fn select_novel_statements(embeddings: &[Option<&[f32]>], threshold: f64) -> Vec<usize> {
    let n = embeddings.len();
    if n == 0 {
        return vec![];
    }

    let mut selected: Vec<usize> = Vec::new();

    // Seed with the first embedded statement.
    for (i, emb) in embeddings.iter().enumerate() {
        if emb.is_some() {
            selected.push(i);
            break;
        }
    }
    // If no embeddings at all, return all indices.
    if selected.is_empty() {
        return (0..n).collect();
    }

    for i in 0..n {
        if selected.contains(&i) {
            continue;
        }
        match embeddings[i] {
            None => {
                // No embedding — always include.
                selected.push(i);
            }
            Some(emb) => {
                let max_sim = selected
                    .iter()
                    .filter_map(|&j| embeddings[j].map(|sel| cosine_similarity_f32(emb, sel)))
                    .fold(f64::NEG_INFINITY, f64::max);
                if max_sim < threshold {
                    selected.push(i);
                }
            }
        }
    }

    selected
}

/// An entity deduplicated across multiple statements.
///
/// When the same entity name appears in multiple statements from the
/// same source, we keep the highest-confidence instance and track
/// all originating statement IDs for multi-provenance.
struct DedupedEntity {
    /// The entity instance with the highest confidence.
    entity: ExtractedEntity,
    /// All statement IDs that produced this entity name.
    source_statements: Vec<StatementId>,
}

impl SourceService {
    /// Run entity extraction on stored statements.
    ///
    /// Three-phase pipeline:
    /// 1. Concurrent LLM extraction from all non-evicted statements.
    /// 2. Cross-statement entity dedup — accumulate all entities,
    ///    keep the highest-confidence instance per lowercase name,
    ///    resolve once per unique entity.
    /// 3. Per-statement relationship linking using the resolved
    ///    `name_to_node` map.
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

        // ── Phase 0: Novelty gating (#195) ─────────────────────
        //
        // Skip near-paraphrase statements to avoid redundant LLM
        // extraction calls. Greedy furthest-first: each statement
        // is included only if its embedding is below the cosine
        // threshold to all already-selected statements.
        let non_evicted: Vec<_> = statements.iter().filter(|s| !s.is_evicted).collect();

        let extractable: Vec<_> = if self.pipeline.statement_novelty_enabled {
            let embeddings: Vec<Option<&[f32]>> =
                non_evicted.iter().map(|s| s.embedding.as_deref()).collect();
            let novel_indices =
                select_novel_statements(&embeddings, self.pipeline.statement_novelty_threshold);
            let skipped = non_evicted.len() - novel_indices.len();
            if skipped > 0 {
                tracing::info!(
                    total = non_evicted.len(),
                    novel = novel_indices.len(),
                    skipped,
                    threshold = self.pipeline.statement_novelty_threshold,
                    "statement novelty gating"
                );
            }
            novel_indices.into_iter().map(|i| non_evicted[i]).collect()
        } else {
            non_evicted
        };

        // ── Phase 1: Concurrent LLM extraction ──────────────────
        let batch_futures: Vec<_> = extractable
            .iter()
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

        let raw_results = futures::future::join_all(batch_futures).await;

        // Collect successful results; warn and skip failures.
        let mut extraction_results = Vec::new();
        for result in raw_results {
            match result {
                Ok(r) => extraction_results.push(r),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "statement entity extraction failed, skipping"
                    );
                }
            }
        }

        // ── Phase 2: Cross-statement entity dedup ───────────────
        //
        // Accumulate all entities across all statements. For each
        // unique name (case-insensitive), keep the highest-confidence
        // instance and track all originating statement IDs.
        let mut entity_map: HashMap<String, DedupedEntity> = HashMap::new();
        let mut raw_entity_count = 0usize;

        for (stmt_id, extraction) in &extraction_results {
            for entity in &extraction.entities {
                if is_noise_entity(&entity.name, &entity.entity_type) {
                    continue;
                }
                raw_entity_count += 1;
                let key = entity.name.to_lowercase();
                entity_map
                    .entry(key)
                    .and_modify(|d| {
                        d.source_statements.push(*stmt_id);
                        if entity.confidence > d.entity.confidence {
                            d.entity = entity.clone();
                        }
                    })
                    .or_insert(DedupedEntity {
                        entity: entity.clone(),
                        source_statements: vec![*stmt_id],
                    });
            }
        }

        let deduped_count = entity_map.len();
        if raw_entity_count > deduped_count {
            tracing::info!(
                raw = raw_entity_count,
                deduped = deduped_count,
                saved = raw_entity_count - deduped_count,
                "cross-statement entity dedup"
            );
        }

        // Resolve each unique entity once and populate name_to_node.
        let mut name_to_node: HashMap<String, NodeId> = HashMap::new();
        let mut entity_count = 0usize;

        for (key, deduped) in &entity_map {
            // Resolve using the first statement as the primary
            // provenance (the one inside the transaction).
            let primary_stmt = deduped.source_statements[0];
            let node_id = self
                .resolve_and_store_entity(
                    &deduped.entity,
                    ExtractionProvenance::Statement(primary_stmt),
                    "llm_statement",
                    source_id,
                    source_domain,
                )
                .await?;

            let Some(node_id) = node_id else {
                continue;
            };
            name_to_node.insert(key.clone(), node_id);
            entity_count += 1;

            // Create additional extraction records for the remaining
            // source statements. This preserves full provenance —
            // each statement that mentioned this entity gets a link
            // to the resolved node — without re-running resolution.
            for &stmt_id in &deduped.source_statements[1..] {
                let ext_record = Extraction::from_statement(
                    stmt_id,
                    ExtractedEntityType::Node,
                    node_id.into_uuid(),
                    "llm_statement".to_string(),
                    deduped.entity.confidence,
                );
                ExtractionRepo::create(&*self.repo, &ext_record).await?;
            }
        }

        // ── Phase 3: Per-statement relationship linking ─────────
        //
        // Relationships are context-dependent — each belongs to its
        // source statement. The name_to_node map (populated above)
        // resolves entity names to node IDs for edge creation.
        for (stmt_id, extraction) in &extraction_results {
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
                    *stmt_id,
                    ExtractedEntityType::Edge,
                    edge.id.into_uuid(),
                    "llm_statement".to_string(),
                    rel.confidence,
                );
                ExtractionRepo::create(&*self.repo, &ext_record).await?;
            }
        }

        // ── Embed new node descriptions ─────────────────────────
        if !name_to_node.is_empty() {
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();
                use crate::storage::traits::PipelineRepo;
                let has_embedding_ids: Vec<uuid::Uuid> =
                    PipelineRepo::list_node_ids_with_embeddings(
                        &*self.repo,
                        &node_ids.iter().map(|n| n.into_uuid()).collect::<Vec<_>>(),
                    )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a unit vector pointing in one direction.
    fn unit_vec(dim: usize, axis: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[axis] = 1.0;
        v
    }

    /// Helper: create a vector close to another (high cosine sim).
    fn near_vec(base: &[f32], perturbation: f32) -> Vec<f32> {
        base.iter().map(|v| v + perturbation).collect()
    }

    #[test]
    fn novelty_empty_input() {
        assert!(select_novel_statements(&[], 0.92).is_empty());
    }

    #[test]
    fn novelty_single_statement_always_selected() {
        let emb = vec![1.0f32, 0.0, 0.0];
        let embeddings = vec![Some(emb.as_slice())];
        let selected = select_novel_statements(&embeddings, 0.92);
        assert_eq!(selected, vec![0]);
    }

    #[test]
    fn novelty_all_none_embeddings_returns_all() {
        let embeddings: Vec<Option<&[f32]>> = vec![None, None, None];
        let mut selected = select_novel_statements(&embeddings, 0.92);
        selected.sort();
        assert_eq!(selected, vec![0, 1, 2]);
    }

    #[test]
    fn novelty_identical_embeddings_selects_only_first() {
        let emb = vec![1.0f32, 0.0, 0.0];
        let embeddings = vec![
            Some(emb.as_slice()),
            Some(emb.as_slice()),
            Some(emb.as_slice()),
        ];
        let selected = select_novel_statements(&embeddings, 0.92);
        // Only the first should be selected — the others are identical
        // (cosine = 1.0 >= 0.92).
        assert_eq!(selected, vec![0]);
    }

    #[test]
    fn novelty_orthogonal_embeddings_all_selected() {
        let a = unit_vec(3, 0);
        let b = unit_vec(3, 1);
        let c = unit_vec(3, 2);
        let embeddings = vec![Some(a.as_slice()), Some(b.as_slice()), Some(c.as_slice())];
        let mut selected = select_novel_statements(&embeddings, 0.92);
        selected.sort();
        // All are orthogonal (cosine = 0.0 < 0.92), all novel.
        assert_eq!(selected, vec![0, 1, 2]);
    }

    #[test]
    fn novelty_near_duplicate_filtered() {
        let base = vec![1.0f32, 0.0, 0.0];
        let near = near_vec(&base, 0.001); // very close to base
        let far = unit_vec(3, 1); // orthogonal
        let embeddings = vec![
            Some(base.as_slice()),
            Some(near.as_slice()),
            Some(far.as_slice()),
        ];
        let selected = select_novel_statements(&embeddings, 0.92);
        // base selected first, near is too similar, far is novel.
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&0));
        assert!(selected.contains(&2));
        assert!(!selected.contains(&1));
    }

    #[test]
    fn novelty_threshold_controls_aggressiveness() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.9f32, 0.436]; // cosine ~ 0.9 with a
        let embeddings = vec![Some(a.as_slice()), Some(b.as_slice())];

        // Threshold 0.95 — b is below, both selected.
        let selected = select_novel_statements(&embeddings, 0.95);
        assert_eq!(selected.len(), 2);

        // Threshold 0.85 — b is above, only a selected.
        let selected = select_novel_statements(&embeddings, 0.85);
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn novelty_none_embeddings_always_included() {
        let emb = vec![1.0f32, 0.0, 0.0];
        let embeddings = vec![
            Some(emb.as_slice()),
            None, // no embedding — must be included
            Some(emb.as_slice()),
        ];
        let selected = select_novel_statements(&embeddings, 0.92);
        // Index 0 (seed), 1 (None → always included).
        // Index 2 is identical to 0, filtered.
        assert!(selected.contains(&0));
        assert!(selected.contains(&1));
        assert!(!selected.contains(&2));
    }
}
