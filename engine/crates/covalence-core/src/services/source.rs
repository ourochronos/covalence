//! Source service — ingestion orchestration and source management.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::config::TableDimensions;
use crate::error::{Error, Result};
use crate::ingestion::converter::ConverterRegistry;
use crate::ingestion::coreference::CorefResolver;
use crate::ingestion::embedder::{Embedder, truncate_and_validate};
use crate::ingestion::extractor::Extractor;
use crate::ingestion::landscape::ExtractionMethod;
use crate::ingestion::pg_resolver::PgResolver;
use crate::ingestion::resolver::EntityResolver;
use crate::models::chunk::Chunk;
use crate::models::chunk::ChunkLevel as ModelChunkLevel;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::models::source::{Source, SourceType, UpdateClass};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{
    ChunkRepo, EdgeRepo, ExtractionRepo, NodeAliasRepo, NodeRepo, SourceRepo,
};
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{AliasId, ChunkId, NodeId, SourceId};

/// Result of a cascading delete operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteResult {
    /// Whether the source was found and deleted.
    pub deleted: bool,
    /// Number of chunks deleted alongside the source.
    pub chunks_deleted: u64,
}

/// Service for source ingestion and management.
pub struct SourceService {
    repo: Arc<PgRepo>,
    embedder: Option<Arc<dyn Embedder>>,
    extractor: Option<Arc<dyn Extractor>>,
    resolver: Option<Arc<dyn EntityResolver>>,
    /// Maximum number of concurrent LLM extraction calls.
    extract_concurrency: usize,
    /// Resolver for normalizing relationship type labels via
    /// trigram similarity against existing edge types.
    rel_type_resolver: Option<Arc<PgResolver>>,
    /// Optional format converter registry for pre-processing
    /// content before the parser stage. When set, incoming
    /// content is run through the matching converter to produce
    /// Markdown before parsing.
    converter_registry: Option<ConverterRegistry>,
    /// Per-table embedding dimensions for truncation.
    table_dims: TableDimensions,
}

impl SourceService {
    /// Default extraction concurrency when not configured.
    const DEFAULT_EXTRACT_CONCURRENCY: usize = 8;

    /// Create a new source service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self {
            repo,
            embedder: None,
            extractor: None,
            resolver: None,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver: None,
            converter_registry: None,
            table_dims: TableDimensions::default(),
        }
    }

    /// Create a new source service with optional AI components.
    pub fn with_ai(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver: None,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver: None,
            converter_registry: None,
            table_dims: TableDimensions::default(),
        }
    }

    /// Create a new source service with full AI pipeline.
    pub fn with_full_pipeline(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
        resolver: Option<Arc<dyn EntityResolver>>,
        rel_type_resolver: Option<Arc<PgResolver>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver,
            converter_registry: None,
            table_dims: TableDimensions::default(),
        }
    }

    /// Attach a converter registry for pre-processing content
    /// before the parser stage.
    ///
    /// When set, the `ingest` method will run incoming content
    /// through the matching converter to produce Markdown before
    /// passing it to the parser.
    pub fn with_converter_registry(mut self, registry: ConverterRegistry) -> Self {
        self.converter_registry = Some(registry);
        self
    }

    /// Set per-table embedding dimensions.
    pub fn with_table_dims(mut self, dims: TableDimensions) -> Self {
        self.table_dims = dims;
        self
    }

    /// Set the maximum number of concurrent LLM extraction calls.
    pub fn with_extract_concurrency(mut self, concurrency: usize) -> Self {
        self.extract_concurrency = concurrency;
        self
    }

    /// Ingest new content through the full pipeline.
    ///
    /// Stages: hash, dedup/supersede, parse, normalize, chunk, embed,
    /// landscape analysis, extract (gated by landscape), resolve,
    /// embed nodes.
    ///
    /// **Embedding landscape gating**: After landscape analysis, only
    /// chunks with `FullExtraction` or `FullExtractionWithReview`
    /// extraction methods are sent to the LLM extractor. Chunks
    /// classified as `EmbeddingLinkage` or `DeltaCheck` skip
    /// expensive LLM calls.
    ///
    /// **Source update classes**: When a URI is provided and an
    /// existing source shares that URI, the system detects the
    /// update class (correction, versioned, refactor) based on
    /// content overlap, marks the old source as superseded, and
    /// links the new source via `supersedes_id`.
    pub async fn ingest(
        &self,
        content: &[u8],
        source_type: &str,
        mime: &str,
        uri: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<SourceId> {
        let hash = Sha256::digest(content).to_vec();

        // Dedup check — exact content hash match
        if let Some(existing) = SourceRepo::get_by_hash(&*self.repo, &hash).await? {
            return Ok(existing.id);
        }

        let st = SourceType::from_str_opt(source_type)
            .ok_or_else(|| Error::InvalidInput(format!("unknown source type: {source_type}")))?;

        // --- Source update class detection ---
        //
        // If a URI is provided, check whether an existing source
        // shares that URI. If content hashes differ, determine the
        // update class and mark the old source as superseded.
        let supersedes_info = if let Some(uri_str) = uri {
            self.detect_source_update(uri_str, content).await?
        } else {
            None
        };

        // Stage 1.5: Convert content if a converter registry is
        // configured.
        let (parse_content, parse_mime): (std::borrow::Cow<'_, [u8]>, &str) =
            if let Some(ref registry) = self.converter_registry {
                let converted = registry.convert(content, mime).await?;
                (
                    std::borrow::Cow::Owned(converted.into_bytes()),
                    "text/markdown",
                )
            } else {
                (std::borrow::Cow::Borrowed(content), mime)
            };

        // Stage 2: Parse
        let parsed = crate::ingestion::parser::parse(&parse_content, parse_mime)?;

        // Stage 3: Normalize
        let normalized = crate::ingestion::normalize::normalize(&parsed.body);

        // Create source record
        let mut source = Source::new(st, hash);
        source.uri = uri.map(|s| s.to_string());
        source.title = parsed.title;
        source.metadata = metadata;
        source.raw_content = String::from_utf8(content.to_vec()).ok();

        // Apply supersession metadata if detected
        if let Some(ref info) = supersedes_info {
            source.supersedes_id = Some(info.old_source_id);
            source.update_class = Some(info.update_class.as_str().to_string());
            source.content_version = info.new_version;
        }

        SourceRepo::create(&*self.repo, &source).await?;

        // Mark old source as superseded if applicable
        if let Some(ref info) = supersedes_info {
            self.mark_superseded(info.old_source_id, &info.update_class)
                .await?;
        }

        // Stage 4: Chunk
        let chunk_outputs = crate::ingestion::chunker::chunk_document(&normalized, 1000, 200);

        // Build a map from chunker UUIDs to ChunkIds
        let mut id_map = std::collections::HashMap::new();
        for co in &chunk_outputs {
            id_map.insert(co.id, ChunkId::from_uuid(co.id));
        }

        // Build all chunks for batch insertion
        let chunks: Vec<Chunk> = chunk_outputs
            .iter()
            .enumerate()
            .map(|(ordinal, co)| {
                let chunk_id = id_map[&co.id];
                let level = match co.level {
                    crate::ingestion::chunker::ChunkLevel::Section => ModelChunkLevel::Section,
                    crate::ingestion::chunker::ChunkLevel::Paragraph => ModelChunkLevel::Paragraph,
                };

                let content_hash = Sha256::digest(co.text.as_bytes()).to_vec();
                let token_count = co.text.split_whitespace().count() as i32;
                let hierarchy = co
                    .heading_path
                    .iter()
                    .map(|h| sanitize_ltree_label(h))
                    .collect::<Vec<_>>()
                    .join(".");

                let mut chunk = Chunk::new(
                    source.id,
                    level,
                    ordinal as i32,
                    co.text.clone(),
                    content_hash,
                    token_count,
                );
                chunk.id = chunk_id;
                if let Some(parent_uuid) = co.parent_id {
                    if let Some(&pid) = id_map.get(&parent_uuid) {
                        chunk = chunk.with_parent(pid);
                    }
                }
                if !hierarchy.is_empty() {
                    chunk = chunk.with_hierarchy(hierarchy);
                }
                chunk
            })
            .collect();

        // Store all chunks in a single batch INSERT
        ChunkRepo::batch_create(&*self.repo, &chunks).await?;

        // Stage 5: Embed chunks
        //
        // Uses contextual chunk embeddings when supported (e.g.,
        // Voyage `voyage-context-3`). Each chunk embedding reflects
        // the surrounding document context (late chunking).
        //
        // Also runs landscape analysis to determine which chunks
        // need LLM extraction.
        let landscape_results = if let Some(ref embedder) = self.embedder {
            let texts: Vec<String> = chunk_outputs.iter().map(|co| co.text.clone()).collect();
            let embeddings = embedder.embed_document_chunks(&texts).await?;

            for (co, emb) in chunk_outputs.iter().zip(embeddings.iter()) {
                let chunk_id = id_map[&co.id];
                let truncated = truncate_and_validate(emb, self.table_dims.chunk, "chunks")?;
                ChunkRepo::update_embedding(&*self.repo, chunk_id, &truncated).await?;
            }

            // Embed the full normalized text for source-level search
            let source_embeddings = embedder.embed(std::slice::from_ref(&normalized)).await?;
            if let Some(emb) = source_embeddings.first() {
                let truncated = truncate_and_validate(emb, self.table_dims.source, "sources")?;
                let emb_f32: Vec<f32> = truncated.iter().map(|&v| v as f32).collect();
                SourceRepo::update_embedding(&*self.repo, source.id, &emb_f32).await?;
            }

            // Stage 5.5: Embedding landscape analysis.
            //
            // Determine extraction method per chunk based on
            // embedding topology.
            let parent_embeddings: Vec<Option<&Vec<f64>>> = chunk_outputs
                .iter()
                .map(|co| {
                    co.parent_id.and_then(|pid| {
                        chunk_outputs
                            .iter()
                            .position(|c| c.id == pid)
                            .and_then(|idx| embeddings.get(idx))
                    })
                })
                .collect();

            let landscape = crate::ingestion::landscape::analyze_landscape(
                &embeddings,
                &parent_embeddings,
                None,
            );

            // Store landscape results on chunks
            for lr in &landscape {
                if lr.chunk_index < chunk_outputs.len() {
                    let chunk_id = id_map[&chunk_outputs[lr.chunk_index].id];
                    let metrics_json = serde_json::to_value(&lr.metrics).ok();
                    ChunkRepo::update_landscape(
                        &*self.repo,
                        chunk_id,
                        lr.parent_alignment,
                        lr.extraction_method.as_str(),
                        metrics_json,
                    )
                    .await?;
                }
            }

            Some(landscape)
        } else {
            None
        };

        // Stage 5.5: Co-reference resolution across chunks
        let coref_resolver = CorefResolver::new();
        let coref_links = coref_resolver.resolve(&chunk_outputs);
        let mut coref_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for link in &coref_links {
            coref_map.insert(link.mention.to_lowercase(), link.referent.to_lowercase());
        }

        // Stages 6-7: Extract entities + resolve + create
        // nodes/edges.
        //
        // Landscape gating: Only send chunks with FullExtraction
        // or FullExtractionWithReview to the LLM. Chunks classified
        // as EmbeddingLinkage or DeltaCheck are skipped.
        if let Some(ref extractor) = self.extractor {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));

            // Determine which chunks should go through LLM
            // extraction based on landscape analysis
            let extraction_futures: Vec<_> = chunk_outputs
                .iter()
                .enumerate()
                .filter(|(i, _co)| should_extract(*i, landscape_results.as_deref()))
                .map(|(_i, co)| {
                    let sem = Arc::clone(&semaphore);
                    let ext = Arc::clone(extractor);
                    let text = co.text.clone();
                    let chunk_uuid = co.id;
                    async move {
                        let _permit = sem.acquire().await.map_err(|_| {
                            Error::Ingestion("extraction semaphore closed".to_string())
                        })?;
                        let result = ext.extract(&text).await?;
                        Ok::<_, Error>((chunk_uuid, result))
                    }
                })
                .collect();

            let extraction_results = futures::future::join_all(extraction_futures).await;

            // Phase 2: Process extraction results sequentially
            let mut name_to_node: std::collections::HashMap<String, NodeId> =
                std::collections::HashMap::new();

            for extraction_result in extraction_results {
                let (chunk_uuid, extraction) = extraction_result?;
                let chunk_id = id_map[&chunk_uuid];

                for entity in &extraction.entities {
                    let node_id = self.resolve_and_store_entity(entity, chunk_id).await?;
                    name_to_node.insert(entity.name.to_lowercase(), node_id);

                    if let Some(referent) = coref_map.get(&entity.name.to_lowercase()) {
                        name_to_node.entry(referent.clone()).or_insert(node_id);
                        self.ensure_alias(&entity.name, node_id, chunk_id).await?;
                    }
                }

                for rel in &extraction.relationships {
                    let src_key = coref_map
                        .get(&rel.source_name.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| rel.source_name.to_lowercase());
                    let tgt_key = coref_map
                        .get(&rel.target_name.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| rel.target_name.to_lowercase());

                    let source_id = match name_to_node.get(&src_key) {
                        Some(&id) => id,
                        None => continue,
                    };
                    let target_id = match name_to_node.get(&tgt_key) {
                        Some(&id) => id,
                        None => continue,
                    };

                    let resolved_rel_type = if let Some(ref rtr) = self.rel_type_resolver {
                        rtr.resolve_rel_type(&rel.rel_type).await?
                    } else {
                        rel.rel_type.clone()
                    };

                    let mut edge = Edge::new(source_id, target_id, resolved_rel_type);
                    edge.confidence = rel.confidence;
                    EdgeRepo::create(&*self.repo, &edge).await?;

                    let ext_record = Extraction::new(
                        chunk_id,
                        ExtractedEntityType::Edge,
                        edge.id.into_uuid(),
                        "llm".to_string(),
                        rel.confidence,
                    );
                    ExtractionRepo::create(&*self.repo, &ext_record).await?;
                }
            }

            // Stage 7.5: Embed node descriptions
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();

                if !node_ids.is_empty() {
                    let mut texts: Vec<String> = Vec::with_capacity(node_ids.len());
                    let mut valid_ids: Vec<NodeId> = Vec::with_capacity(node_ids.len());

                    for &nid in &node_ids {
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
                        let embeddings = embedder.embed(&texts).await?;
                        for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                            let truncated =
                                truncate_and_validate(emb, self.table_dims.node, "nodes")?;
                            NodeRepo::update_embedding(&*self.repo, *nid, &truncated).await?;
                        }
                    }
                }
            }
        }

        Ok(source.id)
    }

    /// Detect whether an existing source with the same URI exists
    /// and determine the update class based on content overlap.
    ///
    /// Returns `None` if no existing source shares the URI.
    async fn detect_source_update(
        &self,
        uri: &str,
        new_content: &[u8],
    ) -> Result<Option<SupersedesInfo>> {
        use sqlx::Row;

        // Query for existing source with this URI
        let row = sqlx::query(
            "SELECT id, content_hash, raw_content, content_version \
             FROM sources \
             WHERE uri = $1 \
             ORDER BY content_version DESC \
             LIMIT 1",
        )
        .bind(uri)
        .fetch_optional(self.repo.pool())
        .await?;

        let row = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let old_id: SourceId = row.get("id");
        let old_content: Option<String> = row.get("raw_content");
        let old_version: i32 = row.get("content_version");

        let new_text = String::from_utf8_lossy(new_content);

        let update_class = match old_content {
            Some(ref old_text) => detect_update_class(old_text, &new_text),
            None => UpdateClass::Versioned,
        };

        Ok(Some(SupersedesInfo {
            old_source_id: old_id,
            update_class,
            new_version: old_version + 1,
        }))
    }

    /// Mark an old source as superseded by updating its
    /// `update_class` to reflect the supersession.
    async fn mark_superseded(&self, old_id: SourceId, update_class: &UpdateClass) -> Result<()> {
        sqlx::query("UPDATE sources SET update_class = $2 WHERE id = $1")
            .bind(old_id)
            .bind(update_class.as_str())
            .execute(self.repo.pool())
            .await?;
        Ok(())
    }

    /// Resolve an extracted entity against the graph and store it.
    ///
    /// If a resolver is available, uses it to match against existing
    /// nodes. Otherwise creates a new node. Returns the `NodeId`
    /// (existing or new).
    ///
    /// On a fuzzy match, automatically creates a [`NodeAlias`] so that
    /// future exact lookups will match without falling back to trigram
    /// search.
    ///
    /// The entire read-check-write cycle runs inside a PostgreSQL
    /// transaction that holds a transaction-scoped advisory lock
    /// (`pg_advisory_xact_lock`) on a hash of the entity name. This
    /// prevents two concurrent workers from both creating the same
    /// node when they see "not found" at the same time. The lock is
    /// automatically released when the transaction commits or rolls
    /// back.
    async fn resolve_and_store_entity(
        &self,
        entity: &crate::ingestion::extractor::ExtractedEntity,
        chunk_id: ChunkId,
    ) -> Result<NodeId> {
        use crate::ingestion::resolver::MatchType;
        use sqlx::Row;

        let lock_key = entity_name_lock_key(&entity.name);

        // Begin an explicit transaction so the advisory lock is
        // scoped to it and auto-released on commit/rollback.
        let mut tx = self.repo.pool().begin().await?;

        // Acquire a transaction-scoped advisory lock. This blocks
        // until any other transaction holding the same key commits
        // or rolls back.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(&mut *tx)
            .await?;

        // --- critical section: read-check-write under lock ---

        let (node_id, match_type) = if let Some(ref resolver) = self.resolver {
            let resolved = resolver.resolve(entity).await?;
            match resolved.match_type {
                MatchType::New => {
                    let node = self.create_node_in_tx(&mut tx, entity).await?;
                    (node.id, MatchType::New)
                }
                ref mt => {
                    let match_type = mt.clone();
                    if let Some(nid) = resolved.node_id {
                        Self::bump_mention_in_tx(&mut tx, nid).await?;
                        (nid, match_type)
                    } else {
                        let node = self.create_node_in_tx(&mut tx, entity).await?;
                        (node.id, MatchType::New)
                    }
                }
            }
        } else {
            // No resolver — check by name, create if new.
            let existing: Option<NodeId> = sqlx::query(
                "SELECT id FROM nodes \
                 WHERE LOWER(canonical_name) = LOWER($1)",
            )
            .bind(&entity.name)
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| r.get("id"));

            if let Some(nid) = existing {
                Self::bump_mention_in_tx(&mut tx, nid).await?;
                (nid, MatchType::Exact)
            } else {
                let node = self.create_node_in_tx(&mut tx, entity).await?;
                (node.id, MatchType::New)
            }
        };

        // Auto-alias on fuzzy match so subsequent exact lookups hit
        // without requiring another fuzzy search.
        if match_type == MatchType::Fuzzy {
            self.ensure_alias_in_tx(&mut tx, &entity.name, node_id, chunk_id)
                .await?;
        }

        // Create extraction provenance record inside the same tx.
        let ext_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO extractions (
                id, chunk_id, entity_type, entity_id,
                extraction_method, confidence, is_superseded
            ) VALUES ($1, $2, $3, $4, $5, $6, false)",
        )
        .bind(ext_id)
        .bind(chunk_id)
        .bind("node")
        .bind(node_id.into_uuid())
        .bind("llm")
        .bind(entity.confidence)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(node_id)
    }

    /// Insert a new node inside an existing transaction.
    async fn create_node_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entity: &crate::ingestion::extractor::ExtractedEntity,
    ) -> Result<Node> {
        let mut node = Node::new(entity.name.clone(), entity.entity_type.clone());
        node.description = entity.description.clone();

        let confidence_json = node.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "INSERT INTO nodes (
                id, canonical_name, node_type, description,
                properties, confidence_breakdown,
                clearance_level, first_seen, last_seen,
                mention_count
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6,
                $7, $8, $9,
                $10
            )",
        )
        .bind(node.id)
        .bind(&node.canonical_name)
        .bind(&node.node_type)
        .bind(&node.description)
        .bind(&node.properties)
        .bind(&confidence_json)
        .bind(node.clearance_level.as_i32())
        .bind(node.first_seen)
        .bind(node.last_seen)
        .bind(node.mention_count)
        .execute(&mut **tx)
        .await?;

        Ok(node)
    }

    /// Bump `mention_count` and `last_seen` for an existing node
    /// inside a transaction.
    async fn bump_mention_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE nodes \
             SET mention_count = mention_count + 1, \
                 last_seen = NOW() \
             WHERE id = $1",
        )
        .bind(node_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Create an alias inside a transaction if one doesn't exist.
    async fn ensure_alias_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        use sqlx::Row;

        let exists: bool = sqlx::query(
            "SELECT EXISTS(
                SELECT 1 FROM node_aliases
                WHERE LOWER(alias) = LOWER($1)
            ) AS exists",
        )
        .bind(alias_text)
        .fetch_one(&mut **tx)
        .await?
        .get("exists");

        if !exists {
            let alias_id = AliasId::new();
            sqlx::query(
                "INSERT INTO node_aliases \
                 (id, node_id, alias, source_chunk_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(alias_id)
            .bind(node_id)
            .bind(alias_text)
            .bind(chunk_id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    /// Create an alias for a node if one doesn't already exist.
    async fn ensure_alias(
        &self,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        let existing = NodeAliasRepo::find_by_alias(&*self.repo, alias_text).await?;
        let already_exists = existing
            .iter()
            .any(|a| a.alias.eq_ignore_ascii_case(alias_text));
        if !already_exists {
            let alias = NodeAlias {
                id: AliasId::new(),
                node_id,
                alias: alias_text.to_string(),
                source_chunk_id: Some(chunk_id),
            };
            NodeAliasRepo::create(&*self.repo, &alias).await?;
        }
        Ok(())
    }

    /// Get a source by ID.
    pub async fn get(&self, id: SourceId) -> Result<Option<Source>> {
        SourceRepo::get(&*self.repo, id).await
    }

    /// List sources with pagination.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Source>> {
        SourceRepo::list(&*self.repo, limit, offset).await
    }

    /// Delete a source and its associated chunks.
    pub async fn delete(&self, id: SourceId) -> Result<DeleteResult> {
        let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, id).await?;
        let deleted = SourceRepo::delete(&*self.repo, id).await?;
        Ok(DeleteResult {
            deleted,
            chunks_deleted,
        })
    }

    /// Publish a source by promoting its clearance level from
    /// `LocalStrict` (0) to `FederatedTrusted` (1).
    ///
    /// Returns an error if the source is not found or is already at a
    /// clearance level above `LocalStrict`.
    pub async fn publish(&self, id: SourceId) -> Result<Source> {
        let mut source =
            SourceRepo::get(&*self.repo, id)
                .await?
                .ok_or_else(|| Error::NotFound {
                    entity_type: "source",
                    id: id.to_string(),
                })?;

        if source.clearance_level != ClearanceLevel::LocalStrict {
            return Err(Error::InvalidInput(format!(
                "source {} is already at clearance level {}",
                id, source.clearance_level
            )));
        }

        source.clearance_level = ClearanceLevel::FederatedTrusted;
        SourceRepo::update(&*self.repo, &source).await?;
        Ok(source)
    }

    /// Count total number of sources.
    pub async fn count(&self) -> Result<i64> {
        SourceRepo::count(&*self.repo).await
    }

    /// Get all chunks for a source.
    pub async fn get_chunks(
        &self,
        source_id: SourceId,
    ) -> Result<Vec<crate::models::chunk::Chunk>> {
        ChunkRepo::list_by_source(&*self.repo, source_id).await
    }
}

/// Fixed namespace offset for entity-resolution advisory locks.
///
/// This constant is XORed into the hash to avoid collisions with
/// advisory locks used for other purposes in the same database.
const ENTITY_LOCK_NAMESPACE: i64 = 0x436F_7661_6C65_6E63; // "Covalenc"

/// Produce a deterministic i64 hash from an entity name for use as a
/// PostgreSQL advisory lock key.
///
/// Uses the first 8 bytes of a SHA-256 hash of the lowercased,
/// trimmed name, XORed with [`ENTITY_LOCK_NAMESPACE`] to avoid
/// collisions with other advisory lock users.
pub(crate) fn entity_name_lock_key(name: &str) -> i64 {
    let canonical = name.trim().to_lowercase();
    let hash = Sha256::digest(canonical.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash[..8]);
    i64::from_le_bytes(bytes) ^ ENTITY_LOCK_NAMESPACE
}

/// Information about a source supersession detected during
/// URI-based update class analysis.
struct SupersedesInfo {
    /// The ID of the source being superseded.
    old_source_id: SourceId,
    /// The detected update class.
    update_class: UpdateClass,
    /// The version number for the new source.
    new_version: i32,
}

/// Determine whether a chunk should go through LLM extraction
/// based on landscape analysis results.
///
/// Returns `true` if:
/// - No landscape results exist (all chunks should be extracted)
/// - The chunk's extraction method is `FullExtraction` or
///   `FullExtractionWithReview`
///
/// Returns `false` for `EmbeddingLinkage` (high parent alignment,
/// redundant content) and `DeltaCheck` (moderate alignment, only
/// delta check needed).
fn should_extract(
    chunk_index: usize,
    landscape: Option<&[crate::ingestion::landscape::ChunkLandscapeResult]>,
) -> bool {
    match landscape {
        None => true, // No landscape data — extract everything
        Some(results) => {
            match results.get(chunk_index) {
                None => true, // Missing result — extract
                Some(lr) => matches!(
                    lr.extraction_method,
                    ExtractionMethod::FullExtraction | ExtractionMethod::FullExtractionWithReview
                ),
            }
        }
    }
}

/// Detect the update class by comparing old and new content.
///
/// Uses a simple word-level Jaccard similarity metric:
/// - `>=80%` overlap: `Correction` (minor fix to existing content)
/// - `<20%` overlap: `Refactor` (structural rewrite)
/// - Otherwise: `Versioned` (normal update)
///
/// This is a lightweight heuristic. Production systems may use
/// more sophisticated diff algorithms.
fn detect_update_class(old_text: &str, new_text: &str) -> UpdateClass {
    let overlap = content_overlap(old_text, new_text);
    if overlap >= 0.80 {
        UpdateClass::Correction
    } else if overlap < 0.20 {
        UpdateClass::Refactor
    } else {
        UpdateClass::Versioned
    }
}

/// Compute word-level Jaccard similarity between two texts.
///
/// Returns a value in `[0.0, 1.0]` representing the proportion
/// of shared words between the two texts. Returns 0.0 if both
/// texts are empty.
fn content_overlap(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

/// Sanitize a string for use as an ltree label.
///
/// ltree labels can only contain alphanumeric characters and
/// underscores. All other characters (spaces, hyphens, punctuation,
/// etc.) are replaced with `_`. Empty labels become `"_"`.
fn sanitize_ltree_label(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_name_lock_key_is_deterministic() {
        let a = entity_name_lock_key("Rust");
        let b = entity_name_lock_key("Rust");
        assert_eq!(a, b);
    }

    #[test]
    fn entity_name_lock_key_is_case_insensitive() {
        let lower = entity_name_lock_key("rust");
        let upper = entity_name_lock_key("RUST");
        let mixed = entity_name_lock_key("RuSt");
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
    }

    #[test]
    fn entity_name_lock_key_trims_whitespace() {
        let plain = entity_name_lock_key("rust");
        let padded = entity_name_lock_key("  rust  ");
        assert_eq!(plain, padded);
    }

    #[test]
    fn entity_name_lock_key_differs_for_different_names() {
        let a = entity_name_lock_key("rust");
        let b = entity_name_lock_key("python");
        assert_ne!(a, b);
    }

    #[test]
    fn entity_name_lock_key_includes_namespace() {
        // Compute what the raw hash would be without the XOR and
        // verify the function's output differs (proving the
        // namespace is applied).
        let canonical = "test_entity";
        let hash = Sha256::digest(canonical.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash[..8]);
        let raw = i64::from_le_bytes(bytes);

        let keyed = entity_name_lock_key(canonical);
        assert_ne!(raw, keyed);
        assert_eq!(keyed, raw ^ ENTITY_LOCK_NAMESPACE);
    }

    #[test]
    fn entity_name_lock_key_empty_string() {
        // Should not panic on empty input.
        let _ = entity_name_lock_key("");
    }

    #[test]
    fn sanitize_ltree_label_basic() {
        assert_eq!(sanitize_ltree_label("hello world"), "hello_world");
        assert_eq!(sanitize_ltree_label(""), "_");
        assert_eq!(sanitize_ltree_label("a-b"), "a_b");
        assert_eq!(sanitize_ltree_label("abc_123"), "abc_123");
    }

    // --- Content overlap tests ---

    #[test]
    fn content_overlap_identical() {
        let text = "the quick brown fox jumps over the lazy dog";
        let overlap = content_overlap(text, text);
        assert!((overlap - 1.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_no_shared_words() {
        let a = "alpha beta gamma";
        let b = "one two three";
        let overlap = content_overlap(a, b);
        assert!((overlap - 0.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_partial() {
        let a = "the quick brown fox";
        let b = "the slow brown bear";
        // Shared: "the", "brown" = 2
        // Union: "the", "quick", "brown", "fox", "slow", "bear" = 6
        let overlap = content_overlap(a, b);
        assert!((overlap - 2.0 / 6.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_both_empty() {
        assert!((content_overlap("", "") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_one_empty() {
        assert!((content_overlap("hello", "") - 0.0).abs() < 1e-10);
        assert!((content_overlap("", "hello") - 0.0).abs() < 1e-10);
    }

    // --- Update class detection tests ---

    #[test]
    fn detect_update_class_correction() {
        // >80% overlap = correction (minor edit)
        // 9/10 words shared = 0.9 Jaccard
        let old = "the quick brown fox jumps over the lazy dog today";
        let new = "the quick brown fox leaps over the lazy dog today";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Correction);
    }

    #[test]
    fn detect_update_class_refactor() {
        // <20% overlap = refactor (complete rewrite)
        let old = "alpha beta gamma delta epsilon";
        let new = "one two three four five six seven";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Refactor);
    }

    #[test]
    fn detect_update_class_versioned() {
        // Between 20%-80% overlap = versioned (significant update)
        // Shared: a, b, c, d, e = 5 out of union 10 = 0.5
        let old = "a b c d e f g h i j";
        let new = "a b c d e k l m n o";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Versioned);
    }

    // --- Landscape gating tests ---

    #[test]
    fn should_extract_no_landscape() {
        // No landscape data — extract everything
        assert!(should_extract(0, None));
        assert!(should_extract(5, None));
    }

    #[test]
    fn should_extract_full_extraction() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.3),
            extraction_method: ExtractionMethod::FullExtraction,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(should_extract(0, Some(&results)));
    }

    #[test]
    fn should_extract_full_extraction_with_review() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.1),
            extraction_method: ExtractionMethod::FullExtractionWithReview,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(should_extract(0, Some(&results)));
    }

    #[test]
    fn should_not_extract_embedding_linkage() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.9),
            extraction_method: ExtractionMethod::EmbeddingLinkage,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(!should_extract(0, Some(&results)));
    }

    #[test]
    fn should_not_extract_delta_check() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.7),
            extraction_method: ExtractionMethod::DeltaCheck,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(!should_extract(0, Some(&results)));
    }

    #[test]
    fn should_extract_missing_index() {
        // Index not in landscape results — extract
        assert!(should_extract(5, Some(&[])));
    }
}
