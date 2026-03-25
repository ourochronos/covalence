//! OntologyRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::storage::traits::OntologyRepo;

use super::PgRepo;

impl OntologyRepo for PgRepo {
    async fn list_categories(&self) -> Result<Vec<(String, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT id, label, description FROM ontology_categories ORDER BY sort_order",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                let label: String = r.get("label");
                let description: Option<String> = r.get("description");
                (id, label, description)
            })
            .collect())
    }

    async fn list_entity_types(&self) -> Result<Vec<(String, String, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT id, category, label, description \
             FROM ontology_entity_types WHERE is_active = true",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                let category: String = r.get("category");
                let label: String = r.get("label");
                let description: Option<String> = r.get("description");
                (id, category, label, description)
            })
            .collect())
    }

    async fn list_rel_universals(&self) -> Result<Vec<(String, String, bool)>> {
        let rows = sqlx::query("SELECT id, label, is_symmetric FROM ontology_rel_universals")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                let label: String = r.get("label");
                let is_symmetric: bool = r.get("is_symmetric");
                (id, label, is_symmetric)
            })
            .collect())
    }

    async fn list_rel_types(&self) -> Result<Vec<(String, Option<String>, String)>> {
        let rows = sqlx::query(
            "SELECT id, universal, label FROM ontology_rel_types WHERE is_active = true",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                let universal: Option<String> = r.get("universal");
                let label: String = r.get("label");
                (id, universal, label)
            })
            .collect())
    }

    async fn list_domains(&self) -> Result<Vec<(String, String, bool)>> {
        let rows =
            sqlx::query("SELECT id, label, is_internal FROM ontology_domains ORDER BY sort_order")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                let label: String = r.get("label");
                let is_internal: bool = r.get("is_internal");
                (id, label, is_internal)
            })
            .collect())
    }

    async fn list_view_edges(&self) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query("SELECT view_name, rel_type FROM ontology_view_edges")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let view: String = r.get("view_name");
                let rel: String = r.get("rel_type");
                (view, rel)
            })
            .collect())
    }
}
