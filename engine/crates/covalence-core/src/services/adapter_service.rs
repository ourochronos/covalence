//! Source adapter service.
//!
//! Matches incoming sources to adapter configurations stored in PG.
//! Each adapter defines: converter, normalization profile, prompt
//! template, and domain classification. No code needed for most
//! source types — just JSONB config.

use std::sync::Arc;

use sqlx::Row;

use crate::error::Result;
use crate::storage::postgres::PgRepo;

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
        let row = sqlx::query(
            "SELECT * FROM source_adapters \
             WHERE match_domain = $1 AND is_active = true \
             LIMIT 1",
        )
        .bind(domain)
        .fetch_optional(self.repo.pool())
        .await?;

        Ok(row.map(|r| adapter_from_row(&r)))
    }

    /// Find adapter by MIME type match.
    async fn find_by_mime(&self, mime: &str) -> Result<Option<SourceAdapter>> {
        let row = sqlx::query(
            "SELECT * FROM source_adapters \
             WHERE match_mime = $1 AND match_domain IS NULL AND is_active = true \
             LIMIT 1",
        )
        .bind(mime)
        .fetch_optional(self.repo.pool())
        .await?;

        Ok(row.map(|r| adapter_from_row(&r)))
    }

    /// List all adapters (for WebUI).
    pub async fn list_all(&self) -> Result<Vec<SourceAdapter>> {
        let rows = sqlx::query("SELECT * FROM source_adapters ORDER BY name")
            .fetch_all(self.repo.pool())
            .await?;

        Ok(rows.iter().map(adapter_from_row).collect())
    }

    /// Create or update an adapter.
    pub async fn upsert(&self, adapter: &SourceAdapter) -> Result<()> {
        sqlx::query(
            "INSERT INTO source_adapters (
                id, name, description, match_domain, match_mime,
                match_uri_regex, converter, normalization, prompt_template,
                default_source_type, default_domain, webhook_url,
                coref_enabled, statement_enabled, is_active, updated_at
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, NOW()
             ) ON CONFLICT (name) DO UPDATE SET
                description = $3, match_domain = $4, match_mime = $5,
                match_uri_regex = $6, converter = $7, normalization = $8,
                prompt_template = $9, default_source_type = $10,
                default_domain = $11, webhook_url = $12,
                coref_enabled = $13, statement_enabled = $14,
                is_active = $15, updated_at = NOW()",
        )
        .bind(adapter.id)
        .bind(&adapter.name)
        .bind(&adapter.description)
        .bind(&adapter.match_domain)
        .bind(&adapter.match_mime)
        .bind(&adapter.match_uri_regex)
        .bind(&adapter.converter)
        .bind(&adapter.normalization)
        .bind(&adapter.prompt_template)
        .bind(&adapter.default_source_type)
        .bind(&adapter.default_domain)
        .bind(&adapter.webhook_url)
        .bind(adapter.coref_enabled)
        .bind(adapter.statement_enabled)
        .bind(adapter.is_active)
        .execute(self.repo.pool())
        .await?;

        Ok(())
    }
}

fn adapter_from_row(row: &sqlx::postgres::PgRow) -> SourceAdapter {
    SourceAdapter {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        match_domain: row.get("match_domain"),
        match_mime: row.get("match_mime"),
        match_uri_regex: row.get("match_uri_regex"),
        converter: row.get("converter"),
        normalization: row
            .try_get("normalization")
            .unwrap_or_else(|_| "default".to_string()),
        prompt_template: row.get("prompt_template"),
        default_source_type: row
            .try_get("default_source_type")
            .unwrap_or_else(|_| "document".to_string()),
        default_domain: row.get("default_domain"),
        webhook_url: row.get("webhook_url"),
        coref_enabled: row.try_get("coref_enabled").unwrap_or(true),
        statement_enabled: row.try_get("statement_enabled").unwrap_or(true),
        is_active: row.try_get("is_active").unwrap_or(true),
    }
}
