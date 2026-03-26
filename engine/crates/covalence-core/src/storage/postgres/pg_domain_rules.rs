//! DomainRuleRepo, AlignmentRuleRepo, and DomainGroupRepo
//! implementations for PostgreSQL.

use crate::error::Result;
use crate::storage::traits::{AlignmentRuleRepo, DomainGroupRepo, DomainRuleRepo};

use super::PgRepo;

impl DomainRuleRepo for PgRepo {
    async fn match_rules(&self, source_type: &str, uri: Option<&str>) -> Result<Vec<String>> {
        // Load all active rules ordered by priority.
        let rules: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT match_type, match_value, domain_id \
             FROM domain_rules \
             WHERE is_active = true \
             ORDER BY priority ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut matched = Vec::new();
        let uri_str = uri.unwrap_or("");

        for (match_type, match_value, domain_id) in &rules {
            let is_match = match match_type.as_str() {
                "source_type" => source_type == match_value,
                "uri_prefix" => !uri_str.is_empty() && uri_str.starts_with(match_value.as_str()),
                "uri_regex" => {
                    if uri_str.is_empty() {
                        false
                    } else {
                        match regex::Regex::new(match_value) {
                            Ok(re) => re.is_match(uri_str),
                            Err(e) => {
                                tracing::warn!(
                                    pattern = %match_value,
                                    error = %e,
                                    "invalid domain rule regex — skipping"
                                );
                                false
                            }
                        }
                    }
                }
                other => {
                    tracing::warn!(
                        match_type = other,
                        "unknown domain rule match_type — skipping"
                    );
                    false
                }
            };

            if is_match && !matched.contains(domain_id) {
                matched.push(domain_id.clone());
            }
        }

        Ok(matched)
    }
}

#[allow(clippy::type_complexity)]
impl AlignmentRuleRepo for PgRepo {
    async fn list_active(
        &self,
    ) -> Result<
        Vec<(
            i32,
            String,
            String,
            String,
            String,
            String,
            serde_json::Value,
        )>,
    > {
        let rows: Vec<(
            i32,
            String,
            String,
            String,
            String,
            String,
            serde_json::Value,
        )> = sqlx::query_as(
            "SELECT id, name, description, check_type, \
                    source_group, target_group, parameters \
             FROM alignment_rules \
             WHERE is_active = true \
             ORDER BY sort_order ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

impl DomainGroupRepo for PgRepo {
    async fn list_all(&self) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT group_name, domain_id \
             FROM domain_groups \
             ORDER BY group_name, sort_order ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn domain_rule_regex_patterns_are_valid() {
        // Verify the seed patterns from migration 011 compile.
        let patterns = [
            "^file://spec/",
            "^file://docs/adr/",
            "^https://arxiv",
            "^https?://",
        ];
        for p in &patterns {
            assert!(regex::Regex::new(p).is_ok(), "pattern {p} should compile");
        }
    }
}
