//! PostgreSQL repository implementations.
//!
//! Each repository operates on the sqlx connection pool and implements
//! the corresponding trait from `storage::traits`.

mod article;
mod audit;
mod chunk;
mod edge;
mod extraction;
mod ledger;
mod node;
mod node_alias;
mod node_landmark;
mod pg_adapter;
mod pg_admin;
mod pg_analysis;
mod pg_ask;
mod pg_config;
mod pg_job_queue;
mod pg_ontology;
mod pg_pipeline;
mod pg_queue_ext;
mod section;
mod source;
mod statement;
mod trace;
mod unresolved_entity;

use sqlx::postgres::PgPoolOptions;

use crate::error::Result;

/// PostgreSQL-backed repository providing all domain storage operations.
pub struct PgRepo {
    pool: sqlx::PgPool,
}

impl PgRepo {
    /// Connect to PostgreSQL and create a new repository.
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Create a repository from an existing connection pool.
    pub fn from_pool(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
}

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_pool_compiles() {
        // Verify PgRepo is Send + Sync (required by traits)
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<PgRepo>();
    }
}
