//! Source adapter service.
//!
//! Matches incoming sources to adapter configurations stored in PG.
//! Each adapter defines: converter, normalization profile, prompt
//! template, and domain classification. No code needed for most
//! source types — just JSONB config.

use std::sync::Arc;

use crate::error::Result;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::AdapterRepo;

/// A source adapter configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceAdapter {
    pub id: uuid::Uuid,
    pub name: String,
    pub description: Option<String>,
    pub match_domain: Option<String>,
    pub match_mime: Option<String>,
    pub match_uri_regex: Option<String>,
    pub converter: Option<String>,
    pub normalization: String,
    pub prompt_template: Option<String>,
    pub default_source_type: String,
    pub default_domain: Option<String>,
    pub webhook_url: Option<String>,
    pub coref_enabled: bool,
    pub statement_enabled: bool,
    pub is_active: bool,
}

/// Service for matching sources to adapters.
pub struct AdapterService {
    repo: Arc<PgRepo>,
}

impl AdapterService {
    /// Create a new adapter service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self { repo }
    }

    /// Find the best matching adapter for a given URI and MIME type.
    ///
    /// Priority: domain match > MIME match > regex match > generic.
    pub async fn match_adapter(
        &self,
        uri: Option<&str>,
        mime: Option<&str>,
    ) -> Result<Option<SourceAdapter>> {
        // Extract domain from URI if available.
        let domain = uri
            .and_then(|u| url::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(|h| h.to_string()));

        // Try domain match first (most specific).
        if let Some(ref d) = domain {
            if let Some(adapter) = self.find_by_domain(d).await? {
                // If both domain and MIME match, prefer that.
                if let Some(m) = mime {
                    if adapter.match_mime.as_deref() == Some(m) {
                        return Ok(Some(adapter));
                    }
                }
                // Domain match without MIME is still good.
                if adapter.match_mime.is_none() {
                    return Ok(Some(adapter));
                }
            }
        }

        // Try MIME match.
        if let Some(m) = mime {
            if let Some(adapter) = self.find_by_mime(m).await? {
                return Ok(Some(adapter));
            }
        }

        Ok(None)
    }

    /// Find adapter by domain match.
    async fn find_by_domain(&self, domain: &str) -> Result<Option<SourceAdapter>> {
        AdapterRepo::find_by_domain(&*self.repo, domain).await
    }

    /// Find adapter by MIME type match.
    async fn find_by_mime(&self, mime: &str) -> Result<Option<SourceAdapter>> {
        AdapterRepo::find_by_mime(&*self.repo, mime).await
    }

    /// List all adapters (for WebUI).
    pub async fn list_all(&self) -> Result<Vec<SourceAdapter>> {
        AdapterRepo::list_all(&*self.repo).await
    }

    /// Create or update an adapter.
    pub async fn upsert(&self, adapter: &SourceAdapter) -> Result<()> {
        AdapterRepo::upsert(&*self.repo, adapter).await
    }
}
