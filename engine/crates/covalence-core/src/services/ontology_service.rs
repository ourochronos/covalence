//! Ontology service — loads configurable knowledge schema from DB.
//!
//! Replaces hardcoded entity types, relationship types, domains,
//! and view mappings with database-driven configuration per ADR-0022.
//!
//! The ontology is cached in memory and refreshed periodically
//! (via ConfigService polling) or on demand.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::Result;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::OntologyRepo;

/// A universal entity category.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntityCategory {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

/// A domain-specific entity type.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntityType {
    pub id: String,
    pub category: String,
    pub label: String,
    pub description: Option<String>,
}

/// A universal relationship type.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelUniversal {
    pub id: String,
    pub label: String,
    pub is_symmetric: bool,
}

/// A domain-specific relationship type.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelType {
    pub id: String,
    pub universal: Option<String>,
    pub label: String,
}

/// A domain classification.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Domain {
    pub id: String,
    pub label: String,
    pub is_internal: bool,
}

/// Cached ontology — loaded from DB, used for all type lookups.
#[derive(Debug, Clone, Default)]
pub struct OntologyCache {
    /// Entity type → category mapping.
    pub type_to_category: HashMap<String, String>,
    /// All active entity types.
    pub entity_types: Vec<EntityType>,
    /// Relationship type → universal mapping.
    pub rel_to_universal: HashMap<String, String>,
    /// All active relationship types.
    pub rel_types: Vec<RelType>,
    /// All domains.
    pub domains: Vec<Domain>,
    /// Internal domains (for DDSS boost).
    pub internal_domains: HashSet<String>,
    /// View → edge type sets.
    pub view_edges: HashMap<String, HashSet<String>>,
    /// Universal categories.
    pub categories: Vec<EntityCategory>,
    /// Universal relationship types.
    pub rel_universals: Vec<RelUniversal>,
    /// Named domain groups (e.g. "specification" → ["spec", "design"]).
    pub domain_groups: HashMap<String, Vec<String>>,
}

/// Ontology service with cached lookups.
pub struct OntologyService {
    repo: Arc<PgRepo>,
    cache: Arc<RwLock<OntologyCache>>,
}

impl OntologyService {
    /// Create a new ontology service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self {
            repo,
            cache: Arc::new(RwLock::new(OntologyCache::default())),
        }
    }

    /// Load the full ontology from DB into cache.
    pub async fn refresh(&self) -> Result<()> {
        let mut cache = OntologyCache::default();

        // Categories
        let cat_rows = OntologyRepo::list_categories(&*self.repo).await?;
        cache.categories = cat_rows
            .into_iter()
            .map(|(id, label, description)| EntityCategory {
                id,
                label,
                description,
            })
            .collect();

        // Entity types
        let et_rows = OntologyRepo::list_entity_types(&*self.repo).await?;
        for (id, category, label, description) in et_rows {
            cache.type_to_category.insert(id.clone(), category.clone());
            cache.entity_types.push(EntityType {
                id,
                category,
                label,
                description,
            });
        }

        // Universal relationship types
        let ru_rows = OntologyRepo::list_rel_universals(&*self.repo).await?;
        cache.rel_universals = ru_rows
            .into_iter()
            .map(|(id, label, is_symmetric)| RelUniversal {
                id,
                label,
                is_symmetric,
            })
            .collect();

        // Relationship types
        let rt_rows = OntologyRepo::list_rel_types(&*self.repo).await?;
        for (id, universal, label) in rt_rows {
            if let Some(ref u) = universal {
                cache.rel_to_universal.insert(id.clone(), u.clone());
            }
            cache.rel_types.push(RelType {
                id,
                universal,
                label,
            });
        }

        // Domains
        let dom_rows = OntologyRepo::list_domains(&*self.repo).await?;
        for (id, label, is_internal) in dom_rows {
            if is_internal {
                cache.internal_domains.insert(id.clone());
            }
            cache.domains.push(Domain {
                id,
                label,
                is_internal,
            });
        }

        // View → edge type mappings
        let ve_rows = OntologyRepo::list_view_edges(&*self.repo).await?;
        for (view, rel) in ve_rows {
            cache.view_edges.entry(view).or_default().insert(rel);
        }

        // Domain groups
        use crate::storage::traits::DomainGroupRepo;
        match DomainGroupRepo::list_all(&*self.repo).await {
            Ok(dg_rows) => {
                for (group_name, domain_id) in dg_rows {
                    cache
                        .domain_groups
                        .entry(group_name)
                        .or_default()
                        .push(domain_id);
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to load domain groups (table may not exist yet)"
                );
            }
        }

        let mut guard = self.cache.write().await;
        *guard = cache;
        Ok(())
    }

    /// Get the cached ontology (read lock).
    pub async fn get(&self) -> tokio::sync::RwLockReadGuard<'_, OntologyCache> {
        self.cache.read().await
    }

    /// Get category for an entity type.
    pub async fn category_for_type(&self, entity_type: &str) -> Option<String> {
        let cache = self.cache.read().await;
        cache.type_to_category.get(entity_type).cloned()
    }

    /// Get universal relationship for a domain-specific type.
    pub async fn universal_for_rel(&self, rel_type: &str) -> Option<String> {
        let cache = self.cache.read().await;
        cache.rel_to_universal.get(rel_type).cloned()
    }

    /// Check if a domain is internal (for DDSS boost).
    pub async fn is_internal_domain(&self, domain: &str) -> bool {
        let cache = self.cache.read().await;
        cache.internal_domains.contains(domain)
    }

    /// Get edge types for a view.
    pub async fn edges_for_view(&self, view: &str) -> HashSet<String> {
        let cache = self.cache.read().await;
        cache.view_edges.get(view).cloned().unwrap_or_default()
    }

    /// Get domain IDs belonging to a named group.
    ///
    /// Returns an empty vec if the group doesn't exist.
    pub async fn domains_for_group(&self, group: &str) -> Vec<String> {
        let cache = self.cache.read().await;
        cache.domain_groups.get(group).cloned().unwrap_or_default()
    }

    /// Start the polling refresh loop.
    pub fn spawn_refresh_loop(self: &Arc<Self>, interval_secs: u64) {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(e) = svc.refresh().await {
                    tracing::warn!(error = %e, "ontology refresh failed");
                }
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
            }
        });
    }
}
