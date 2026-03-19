//! Entity resolution — resolve extracted entities against the graph
//! and store them as nodes with extraction provenance.
//!
//! Extracted from `pipeline.rs` to keep the module focused.  Contains
//! the core `resolve_and_store_entity` flow, node creation, mention
//! bumping, and alias management.

use crate::error::Result;
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::storage::traits::NodeAliasRepo;
use crate::types::ids::{AliasId, ChunkId, NodeId, SourceId};

use super::super::ingestion_helpers::entity_name_lock_key;
use super::super::source::SourceService;
use super::types::ExtractionProvenance;

impl SourceService {
    /// Resolve an extracted entity against the graph and store it.
    ///
    /// If a resolver is available, uses it to match against existing
    /// nodes. Otherwise creates a new node. Returns the `NodeId`.
    ///
    /// Uses a transaction-scoped advisory lock to prevent concurrent
    /// workers from duplicating nodes.
    pub(crate) async fn resolve_and_store_entity(
        &self,
        entity: &crate::ingestion::extractor::ExtractedEntity,
        provenance: ExtractionProvenance,
        extraction_method: &str,
        source_id: SourceId,
        source_domain: Option<&str>,
    ) -> Result<Option<NodeId>> {
        use crate::ingestion::resolver::MatchType;
        use sqlx::Row;

        let lock_key = entity_name_lock_key(&entity.name);

        let mut tx = self.repo.pool().begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(&mut *tx)
            .await?;

        // --- critical section: read-check-write under lock ---

        let active_resolver = if self.pipeline.resolve_enabled {
            self.resolver.as_ref()
        } else {
            None
        };
        let (node_id, match_type) = if let Some(resolver) = active_resolver {
            let resolved = resolver.resolve(entity).await?;
            match resolved.match_type {
                MatchType::Deferred => {
                    // Route to unresolved_entities pool for Tier 5 HDBSCAN.
                    let mut unresolved = crate::models::unresolved_entity::UnresolvedEntity::new(
                        source_id,
                        entity.name.clone(),
                        entity.entity_type.clone(),
                        entity.confidence,
                    );
                    unresolved.description = entity.description.clone();
                    match &provenance {
                        ExtractionProvenance::Chunk(cid) => {
                            unresolved.chunk_id = Some(cid.into_uuid());
                        }
                        ExtractionProvenance::Statement(sid) => {
                            unresolved.statement_id = Some(sid.into_uuid());
                        }
                    }
                    use crate::storage::traits::UnresolvedEntityRepo;
                    UnresolvedEntityRepo::create(self.repo.as_ref(), &unresolved).await?;
                    tx.commit().await?;
                    tracing::debug!(
                        entity_name = %entity.name,
                        "deferred entity to Tier 5 pool"
                    );
                    return Ok(None);
                }
                MatchType::New => {
                    let node = self
                        .create_node_in_tx(&mut tx, entity, source_domain)
                        .await?;
                    (node.id, MatchType::New)
                }
                ref mt => {
                    let match_type = mt.clone();
                    if let Some(nid) = resolved.node_id {
                        Self::bump_mention_in_tx(&mut tx, nid).await?;
                        (nid, match_type)
                    } else {
                        let node = self
                            .create_node_in_tx(&mut tx, entity, source_domain)
                            .await?;
                        (node.id, MatchType::New)
                    }
                }
            }
        } else {
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
                let node = self
                    .create_node_in_tx(&mut tx, entity, source_domain)
                    .await?;
                (node.id, MatchType::New)
            }
        };

        // Only create aliases from vector matches (high confidence).
        // Fuzzy matches are too unreliable to persist as aliases —
        // short names like "EC" would get aliased to "Error Correction"
        // and then pollute future resolution (#122).
        if let (MatchType::Vector, ExtractionProvenance::Chunk(chunk_id)) =
            (&match_type, &provenance)
        {
            self.ensure_alias_in_tx(&mut tx, &entity.name, node_id, *chunk_id)
                .await?;
        }

        // For code entities resolved to existing nodes: compare
        // ast_hash. If the hash changed, clear semantic_summary so
        // the node gets re-summarized. Always update ast_hash.
        if !matches!(match_type, MatchType::New) {
            if let Some(new_hash) = entity
                .metadata
                .as_ref()
                .and_then(|m| m.get("ast_hash"))
                .and_then(|v| v.as_str())
            {
                let row: Option<(Option<serde_json::Value>,)> =
                    sqlx::query_as("SELECT properties FROM nodes WHERE id = $1")
                        .bind(node_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                let old_hash = row
                    .as_ref()
                    .and_then(|(p,)| p.as_ref())
                    .and_then(|p| p.get("ast_hash"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if old_hash != new_hash {
                    // Hash changed → clear semantic_summary, update
                    // ast_hash and description.
                    let mut props = row
                        .and_then(|(p,)| p)
                        .unwrap_or_else(|| serde_json::json!({}));
                    props["ast_hash"] = serde_json::json!(new_hash);
                    props.as_object_mut().map(|o| o.remove("semantic_summary"));
                    sqlx::query(
                        "UPDATE nodes SET properties = $2, \
                             description = COALESCE($3, description) \
                         WHERE id = $1",
                    )
                    .bind(node_id)
                    .bind(&props)
                    .bind(&entity.description)
                    .execute(&mut *tx)
                    .await?;
                    tracing::debug!(
                        node = %entity.name,
                        "ast_hash changed, cleared semantic_summary"
                    );
                } else {
                    // Hash unchanged — just update ast_hash (idempotent).
                    sqlx::query(
                        "UPDATE nodes \
                         SET properties = jsonb_set(\
                             COALESCE(properties, '{}'), \
                             '{ast_hash}', $2::jsonb) \
                         WHERE id = $1",
                    )
                    .bind(node_id)
                    .bind(serde_json::json!(new_hash))
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        let ext_id = uuid::Uuid::new_v4();
        match provenance {
            ExtractionProvenance::Chunk(chunk_id) => {
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
                .bind(extraction_method)
                .bind(entity.confidence)
                .execute(&mut *tx)
                .await?;
            }
            ExtractionProvenance::Statement(stmt_id) => {
                sqlx::query(
                    "INSERT INTO extractions (
                        id, statement_id, entity_type, entity_id,
                        extraction_method, confidence, is_superseded
                    ) VALUES ($1, $2, $3, $4, $5, $6, false)",
                )
                .bind(ext_id)
                .bind(stmt_id)
                .bind("node")
                .bind(node_id.into_uuid())
                .bind(extraction_method)
                .bind(entity.confidence)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        Ok(Some(node_id))
    }

    /// Insert a new node inside an existing transaction.
    async fn create_node_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entity: &crate::ingestion::extractor::ExtractedEntity,
        source_domain: Option<&str>,
    ) -> Result<Node> {
        use crate::models::node::derive_entity_class_with_context;
        let mut node = Node::new(entity.name.clone(), entity.entity_type.clone());
        // Override entity_class with source-domain-aware derivation.
        // This prevents code-typed entities (struct, function) extracted
        // from non-code sources from being classified as Code.
        node.entity_class = Some(
            derive_entity_class_with_context(&entity.entity_type, source_domain)
                .as_str()
                .to_string(),
        );
        node.description = entity.description.clone();

        // Merge entity metadata (e.g. ast_hash) into node properties.
        if let Some(ref meta) = entity.metadata {
            if let Some(obj) = meta.as_object() {
                if let Some(p) = node.properties.as_object_mut() {
                    for (k, v) in obj {
                        p.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        let confidence_json = node.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "INSERT INTO nodes (
                id, canonical_name, node_type, entity_class,
                description,
                properties, confidence_breakdown,
                clearance_level, first_seen, last_seen,
                mention_count
            ) VALUES (
                $1, $2, $3, $4,
                $5,
                $6, $7,
                $8, $9, $10,
                $11
            )",
        )
        .bind(node.id)
        .bind(&node.canonical_name)
        .bind(&node.node_type)
        .bind(&node.entity_class)
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

    /// Bump `mention_count` and `last_seen` for an existing node.
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

    /// Minimum alias length to prevent short-name pollution (#122).
    ///
    /// 2-letter aliases like "EC", "EL", "ER" match too many unrelated
    /// contexts. Require at least 3 characters.
    const MIN_ALIAS_LEN: usize = 3;

    /// Create an alias inside a transaction if one doesn't exist.
    ///
    /// Validates that the alias is long enough and doesn't already
    /// point to a different node (conflict avoidance).
    async fn ensure_alias_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        use sqlx::Row;

        // Reject short aliases that cause noise (#122).
        if alias_text.trim().len() < Self::MIN_ALIAS_LEN {
            tracing::debug!(
                alias = alias_text,
                "skipping alias creation: too short (< {} chars)",
                Self::MIN_ALIAS_LEN,
            );
            return Ok(());
        }

        // Check if alias already exists (for any node).
        let existing: Option<uuid::Uuid> = sqlx::query(
            "SELECT node_id FROM node_aliases \
             WHERE LOWER(alias) = LOWER($1) \
             LIMIT 1",
        )
        .bind(alias_text)
        .fetch_optional(&mut **tx)
        .await?
        .map(|r| r.get("node_id"));

        match existing {
            Some(existing_node_id) if existing_node_id == node_id.into_uuid() => {
                // Already exists for this node — nothing to do.
            }
            Some(existing_node_id) => {
                // Alias already points to a different node — skip to
                // avoid conflicting aliases (#122).
                tracing::debug!(
                    alias = alias_text,
                    target_node = %node_id.into_uuid(),
                    existing_node = %existing_node_id,
                    "skipping alias creation: already points to different node",
                );
            }
            None => {
                // New alias — create it.
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
        }
        Ok(())
    }

    /// Create an alias for a node if one doesn't already exist.
    ///
    /// Same validation as `ensure_alias_in_tx`: minimum length and
    /// conflict avoidance.
    pub(crate) async fn ensure_alias(
        &self,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        // Reject short aliases (#122).
        if alias_text.trim().len() < Self::MIN_ALIAS_LEN {
            return Ok(());
        }

        let existing = NodeAliasRepo::find_by_alias(&*self.repo, alias_text).await?;
        // Skip if alias already exists (for any node) — prevents
        // conflicting aliases pointing to different entities.
        if !existing.is_empty() {
            return Ok(());
        }
        let alias = NodeAlias {
            id: AliasId::new(),
            node_id,
            alias: alias_text.to_string(),
            source_chunk_id: Some(chunk_id),
        };
        NodeAliasRepo::create(&*self.repo, &alias).await?;
        Ok(())
    }
}
