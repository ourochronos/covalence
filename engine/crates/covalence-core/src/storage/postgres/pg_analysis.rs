//! AnalysisRepo implementation for PostgreSQL.
//!
//! Handles cross-domain analysis, coverage, erosion, blast-radius,
//! and alignment queries previously in the analysis service layer.

use crate::error::Result;
use crate::storage::traits::AnalysisRepo;

use super::PgRepo;

#[allow(clippy::type_complexity)]
impl AnalysisRepo for PgRepo {
    async fn component_exists(&self, name: &str) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM nodes \
             WHERE LOWER(canonical_name) = LOWER($1) \
               AND node_type = 'component')",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    async fn list_component_nodes(&self) -> Result<Vec<(uuid::Uuid, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT * FROM sp_list_component_nodes()")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn list_code_nodes_with_paths(
        &self,
        code_entity_class: &str,
        code_domain: &str,
    ) -> Result<Vec<(uuid::Uuid, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT DISTINCT n.id, n.canonical_name, \
                    COALESCE( \
                      n.properties->>'file_path', \
                      (SELECT s.uri FROM extractions ex \
                       JOIN chunks c ON ex.chunk_id = c.id \
                       JOIN sources s ON c.source_id = s.id \
                       WHERE ex.entity_id = n.id \
                       ORDER BY CASE WHEN $2 = ANY(s.domains) \
                                     THEN 0 ELSE 1 END \
                       LIMIT 1), \
                      '' \
                    ) AS path \
             FROM nodes n \
             WHERE n.entity_class = $1 \
               AND n.node_type != 'code_test' \
               AND n.canonical_name NOT LIKE 'test_%' \
               AND EXISTS ( \
                 SELECT 1 FROM extractions ex \
                 JOIN chunks c ON ex.chunk_id = c.id \
                 JOIN sources s ON c.source_id = s.id \
                 WHERE ex.entity_id = n.id \
                   AND $2 = ANY(s.domains) \
               )",
        )
        .bind(code_entity_class)
        .bind(code_domain)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn check_edge_exists_sp(
        &self,
        source_id: uuid::Uuid,
        target_id: uuid::Uuid,
        rel_type: &str,
    ) -> Result<bool> {
        let exists: bool = sqlx::query_scalar("SELECT sp_check_edge_exists($1, $2, $3)")
            .bind(source_id)
            .bind(target_id)
            .bind(rel_type)
            .fetch_one(&self.pool)
            .await?;
        Ok(exists)
    }

    async fn find_nearest_domain_nodes(
        &self,
        comp_id: uuid::Uuid,
        domains: &[String],
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, f64)>> {
        let rows: Vec<(uuid::Uuid, String, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, \
                    (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
             FROM nodes n \
             WHERE n.entity_class = 'domain' \
               AND n.embedding IS NOT NULL \
               AND n.id != $1 \
               AND EXISTS ( \
                 SELECT 1 FROM extractions ex \
                 JOIN chunks c ON ex.chunk_id = c.id \
                 JOIN sources s ON c.source_id = s.id \
                 WHERE ex.entity_id = n.id \
                   AND s.domains && $3 \
               ) \
             ORDER BY dist ASC \
             LIMIT $2",
        )
        .bind(comp_id)
        .bind(limit)
        .bind(domains)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn get_orphan_code_nodes(&self) -> Result<Vec<(uuid::Uuid, String, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String, String)> =
            sqlx::query_as("SELECT * FROM sp_get_orphan_code_nodes()")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn get_unimplemented_specs(&self) -> Result<Vec<(uuid::Uuid, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT * FROM sp_get_unimplemented_specs()")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn count_spec_concepts(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT sp_count_spec_concepts()")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    async fn count_implemented_specs(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT sp_count_implemented_specs()")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    async fn find_component_code_nodes(
        &self,
        comp_id: uuid::Uuid,
        rel_type: &str,
    ) -> Result<Vec<(uuid::Uuid, String, Option<String>, f64)>> {
        let rows: Vec<(uuid::Uuid, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, \
                    n.properties->>'semantic_summary', \
                    (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
             FROM nodes n \
             JOIN edges e ON e.source_node_id = n.id \
             WHERE e.target_node_id = $1 \
               AND e.rel_type = $2 \
               AND n.embedding IS NOT NULL \
             ORDER BY dist DESC",
        )
        .bind(comp_id)
        .bind(rel_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn get_research_source_bridges(
        &self,
        min_cluster_size: i64,
        limit: i64,
        domain_filter: Option<&str>,
    ) -> Result<Vec<(uuid::Uuid, String, Option<String>, i64, i64)>> {
        let rows: Vec<(uuid::Uuid, String, Option<String>, i64, i64)> =
            sqlx::query_as("SELECT * FROM sp_get_research_source_bridges($1, $2, $3)")
                .bind(min_cluster_size)
                .bind(limit)
                .bind(domain_filter)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn list_source_representative_nodes(
        &self,
        source_id: uuid::Uuid,
    ) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT n.canonical_name, n.node_type \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON c.id = ex.chunk_id \
             WHERE c.source_id = $1 \
             ORDER BY n.canonical_name \
             LIMIT 10",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_connected_components(
        &self,
        source_id: uuid::Uuid,
        rel_type: &str,
    ) -> Result<Vec<(String,)>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT comp.canonical_name \
             FROM nodes comp \
             JOIN edges e ON e.source_node_id = comp.id \
             WHERE e.rel_type = $2 \
               AND comp.node_type = 'component' \
               AND e.target_node_id IN ( \
                 SELECT n.id FROM nodes n \
                 JOIN extractions ex ON ex.entity_id = n.id \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 WHERE c.source_id = $1 \
               )",
        )
        .bind(source_id)
        .bind(rel_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn resolve_node_by_name(
        &self,
        name: &str,
    ) -> Result<Option<(uuid::Uuid, String, String)>> {
        let row: Option<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT * FROM sp_resolve_node_by_name($1)")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn resolve_node_fuzzy(
        &self,
        name: &str,
        limit: i32,
    ) -> Result<Option<(uuid::Uuid, String, String)>> {
        let row: Option<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT * FROM sp_resolve_node_fuzzy($1, $2)")
                .bind(name)
                .bind(limit)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn get_node_component(
        &self,
        node_id: uuid::Uuid,
    ) -> Result<Option<(uuid::Uuid, String)>> {
        let row: Option<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT * FROM sp_get_node_component($1)")
                .bind(node_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn get_invalidated_neighbors(
        &self,
        node_id: uuid::Uuid,
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String, String)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                        e.rel_type \
                 FROM edges e \
                 JOIN nodes n ON n.id = CASE \
                     WHEN e.source_node_id = $1 \
                         THEN e.target_node_id \
                     ELSE e.source_node_id END \
                 WHERE e.invalid_at IS NOT NULL \
                   AND (e.source_node_id = $1 OR e.target_node_id = $1) \
                 LIMIT $2",
        )
        .bind(node_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_nearest_research_nodes(
        &self,
        query_vec: &[f64],
        research_domains: &[String],
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, String, Option<String>, f64)>> {
        let rows: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                    n.properties->>'semantic_summary', \
                    (n.embedding <=> $1::vector) AS dist \
             FROM nodes n \
             WHERE n.embedding IS NOT NULL \
               AND n.node_type != 'component' \
               AND EXISTS ( \
                 SELECT 1 FROM extractions ex \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 JOIN sources s ON s.id = c.source_id \
                 WHERE ex.entity_id = n.id \
                   AND s.domains && $2 \
               ) \
             ORDER BY dist ASC \
             LIMIT $3",
        )
        .bind(query_vec)
        .bind(research_domains)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_theoretical_components(
        &self,
        node_ids: &[uuid::Uuid],
        rel_type: &str,
    ) -> Result<Vec<(uuid::Uuid, String)>> {
        let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT DISTINCT comp.id, comp.canonical_name \
             FROM nodes comp \
             JOIN edges e ON e.source_node_id = comp.id \
             WHERE comp.entity_class = 'analysis' \
               AND e.rel_type = $2 \
               AND e.target_node_id = ANY($1)",
        )
        .bind(node_ids)
        .bind(rel_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_component_code_by_embedding(
        &self,
        query_vec: &[f64],
        comp_ids: &[uuid::Uuid],
        rel_type: &str,
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, String, Option<String>, f64)>> {
        let rows: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                    n.properties->>'semantic_summary', \
                    (n.embedding <=> $1::vector) AS dist \
             FROM nodes n \
             JOIN edges e ON e.source_node_id = n.id \
             WHERE e.rel_type = $4 \
               AND e.target_node_id = ANY($2) \
               AND n.embedding IS NOT NULL \
             ORDER BY dist ASC \
             LIMIT $3",
        )
        .bind(query_vec)
        .bind(comp_ids)
        .bind(limit)
        .bind(rel_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_domain_evidence(
        &self,
        query_vec: &[f64],
        domains: &[String],
        entity_class: Option<&str>,
        code_domain: Option<&str>,
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, String, Option<String>, f64)>> {
        // Two code paths: one for code evidence (uses entity_class + domain),
        // one for research/spec evidence (uses domain list).
        if let (Some(ec), Some(cd)) = (entity_class, code_domain) {
            let rows: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, n.node_type, \
                        COALESCE(n.properties->>'semantic_summary', \
                                 n.description), \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.entity_class = $3 \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND $4 = ANY(s.domains) \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(query_vec)
            .bind(limit)
            .bind(ec)
            .bind(cd)
            .fetch_all(&self.pool)
            .await?;
            Ok(rows)
        } else {
            let rows: Vec<(uuid::Uuid, String, String, Option<String>, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, n.node_type, \
                        n.description, \
                        (n.embedding <=> $1::vector) AS dist \
                 FROM nodes n \
                 WHERE n.embedding IS NOT NULL \
                   AND n.node_type NOT IN ('component') \
                   AND EXISTS ( \
                     SELECT 1 FROM extractions ex \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     JOIN sources s ON s.id = c.source_id \
                     WHERE ex.entity_id = n.id \
                       AND s.domains && $3 \
                   ) \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(query_vec)
            .bind(limit)
            .bind(domains)
            .fetch_all(&self.pool)
            .await?;
            Ok(rows)
        }
    }

    async fn find_code_ahead(
        &self,
        distance_threshold: f64,
        limit: i64,
    ) -> Result<Vec<(String, String, String, Option<f64>, Option<String>)>> {
        // The SP declares `p_limit INT`. sqlx binds Rust `i64` as PG `bigint`,
        // and PG's overload resolution is type-strict — without the explicit
        // `::INT` cast, lookup fails with "function sp_find_code_ahead(double
        // precision, bigint) does not exist". Same fix applied to all four
        // alignment SPs below.
        let rows: Vec<(String, String, String, Option<f64>, Option<String>)> =
            sqlx::query_as("SELECT * FROM sp_find_code_ahead($1, $2::INT)")
                .bind(distance_threshold)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn check_spec_ahead(&self, limit: i64) -> Result<Vec<(String, String, i32)>> {
        let rows: Vec<(String, String, i32)> =
            sqlx::query_as("SELECT * FROM sp_check_spec_ahead($1::INT)")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn find_design_contradictions(
        &self,
        distance_threshold: f64,
        limit: i64,
    ) -> Result<Vec<(String, String, f64, String)>> {
        let rows: Vec<(String, String, f64, String)> =
            sqlx::query_as("SELECT * FROM sp_find_design_contradictions($1, $2::INT)")
                .bind(distance_threshold)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn find_stale_design(&self, limit: i64) -> Result<Vec<(String, String, f64, String)>> {
        let rows: Vec<(String, String, f64, String)> =
            sqlx::query_as("SELECT * FROM sp_find_stale_design($1::INT)")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }
}
