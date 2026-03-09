//! Article service — CRUD wrapper for compiled articles.

use std::sync::Arc;

use crate::error::Result;
use crate::models::article::Article;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::ArticleRepo;
use crate::types::ids::ArticleId;

/// Service for compiled article operations.
pub struct ArticleService {
    repo: Arc<PgRepo>,
}

impl ArticleService {
    /// Create a new article service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self { repo }
    }

    /// Get an article by ID.
    pub async fn get(&self, id: ArticleId) -> Result<Option<Article>> {
        ArticleRepo::get(&*self.repo, id).await
    }

    /// List articles within a domain path prefix.
    pub async fn list_by_domain(
        &self,
        domain_prefix: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Article>> {
        ArticleRepo::list_by_domain(&*self.repo, domain_prefix, limit, offset).await
    }

    /// Delete an article by ID.
    pub async fn delete(&self, id: ArticleId) -> Result<bool> {
        ArticleRepo::delete(&*self.repo, id).await
    }
}
