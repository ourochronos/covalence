//! Source service — ingestion orchestration and source management.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::ingestion::coreference::CorefResolver;
use crate::ingestion::embedder::Embedder;
use crate::ingestion::extractor::Extractor;
use crate::ingestion::resolver::EntityResolver;
use crate::models::chunk::Chunk;
use crate::models::chunk::ChunkLevel as ModelChunkLevel;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::models::source::{Source, SourceType};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{ChunkRepo, EdgeRepo, ExtractionRepo, NodeAliasRepo, SourceRepo};
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
        }
    }

    /// Create a new source service with full AI pipeline.
    pub fn with_full_pipeline(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
        resolver: Option<Arc<dyn EntityResolver>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
        }
    }

    /// Set the maximum number of concurrent LLM extraction calls.
    pub fn with_extract_concurrency(mut self, concurrency: usize) -> Self {
        self.extract_concurrency = concurrency;
        self
    }

    /// Ingest new content through the pipeline: hash, parse, normalize,
    /// chunk, and store.
    ///
    /// Later stages (embed, extract, resolve) require LLM integration and
    /// are not yet wired.
    pub async fn ingest(
        &self,
        content: &[u8],
        source_type: &str,
        mime: &str,
        uri: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<SourceId> {
        let hash = Sha256::digest(content).to_vec();

        // Dedup check
        if let Some(existing) = SourceRepo::get_by_hash(&*self.repo, &hash).await? {
            return Ok(existing.id);
        }

        let st = SourceType::from_str_opt(source_type)
            .ok_or_else(|| Error::InvalidInput(format!("unknown source type: {source_type}")))?;

        // Stage 2: Parse
        let parsed = crate::ingestion::parser::parse(content, mime)?;

        // Stage 3: Normalize
        let normalized = crate::ingestion::normalize::normalize(&parsed.body);

        // Create source record
        let mut source = Source::new(st, hash);
        source.uri = uri.map(|s| s.to_string());
        source.title = parsed.title;
        source.metadata = metadata;
        source.raw_content = String::from_utf8(content.to_vec()).ok();

        SourceRepo::create(&*self.repo, &source).await?;

        // Stage 4: Chunk
        let chunk_outputs = crate::ingestion::chunker::chunk_document(&normalized, 1000, 200);

        // Build a map from chunker UUIDs to ChunkIds for parent references
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
        if let Some(ref embedder) = self.embedder {
            let texts: Vec<String> = chunk_outputs.iter().map(|co| co.text.clone()).collect();
            let embeddings = embedder.embed(&texts).await?;

            for (co, emb) in chunk_outputs.iter().zip(embeddings.iter()) {
                let chunk_id = id_map[&co.id];
                ChunkRepo::update_embedding(&*self.repo, chunk_id, emb).await?;
            }

            // Embed the full normalized text and store on the source
            // record directly. This replaces the old document-level
            // chunk embedding.
            let source_embeddings = embedder.embed(std::slice::from_ref(&normalized)).await?;
            if let Some(emb) = source_embeddings.first() {
                let emb_f32: Vec<f32> = emb.iter().map(|&v| v as f32).collect();
                SourceRepo::update_embedding(&*self.repo, source.id, &emb_f32).await?;
            }
        }

        // Stage 5.5: Co-reference resolution across chunks.
        //
        // Build a mapping from mentions (abbreviations, short forms) to
        // their full referent entity names so the extraction stage can
        // resolve them correctly.
        let coref_resolver = CorefResolver::new();
        let coref_links = coref_resolver.resolve(&chunk_outputs);
        // Index: mention (lowercase) -> referent name (lowercase)
        let mut coref_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for link in &coref_links {
            coref_map.insert(link.mention.to_lowercase(), link.referent.to_lowercase());
        }

        // Stages 6-7: Extract entities + resolve + create nodes/edges
        if let Some(ref extractor) = self.extractor {
            // Phase 1: Run LLM extraction calls concurrently,
            // bounded by a semaphore to limit parallelism.
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));
            let extraction_futures: Vec<_> = chunk_outputs
                .iter()
                .map(|co| {
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

            // Phase 2: Process extraction results sequentially —
            // entity resolution and dedup depend on ordering.
            let mut name_to_node: std::collections::HashMap<String, NodeId> =
                std::collections::HashMap::new();

            for extraction_result in extraction_results {
                let (chunk_uuid, extraction) = extraction_result?;
                let chunk_id = id_map[&chunk_uuid];

                // Process extracted entities
                for entity in &extraction.entities {
                    let node_id = self.resolve_and_store_entity(entity, chunk_id).await?;
                    name_to_node.insert(entity.name.to_lowercase(), node_id);

                    // If this entity name is a co-reference mention,
                    // also register its referent mapping so edge
                    // lookups can find it via the canonical name.
                    if let Some(referent) = coref_map.get(&entity.name.to_lowercase()) {
                        name_to_node.entry(referent.clone()).or_insert(node_id);

                        // Create an alias for the co-reference so
                        // future ingestion resolves it directly.
                        self.ensure_alias(&entity.name, node_id, chunk_id).await?;
                    }
                }

                // Process extracted relationships
                for rel in &extraction.relationships {
                    // Resolve source/target names through co-reference
                    // map if they are abbreviations/mentions.
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

                    let mut edge = Edge::new(source_id, target_id, rel.rel_type.clone());
                    edge.confidence = rel.confidence;
                    EdgeRepo::create(&*self.repo, &edge).await?;

                    // Create extraction provenance for the edge
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
        }

        Ok(source.id)
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
}
