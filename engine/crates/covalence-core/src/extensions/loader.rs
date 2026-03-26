//! Extension loader -- reads manifests and seeds DB tables.
//!
//! The loader scans a directory of extension subdirectories, parses
//! each `extension.yaml`, and upserts declarations into the database.
//! All inserts use `ON CONFLICT DO NOTHING` so loading is idempotent.

use std::path::Path;
use std::sync::Arc;

use sqlx::Row;

use crate::error::{Error, Result};
use crate::storage::postgres::PgRepo;

use super::manifest::ExtensionManifest;

/// Loads extension manifests and seeds DB tables with their
/// declarations.
pub struct ExtensionLoader {
    repo: Arc<PgRepo>,
}

impl ExtensionLoader {
    /// Create a new extension loader.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self { repo }
    }

    /// Parse an `extension.yaml` file into an [`ExtensionManifest`].
    pub fn parse_manifest(path: &Path) -> Result<ExtensionManifest> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!(
                "failed to read extension manifest {}: {e}",
                path.display()
            ))
        })?;
        serde_yaml::from_str(&content).map_err(|e| {
            Error::Config(format!(
                "failed to parse extension manifest {}: {e}",
                path.display()
            ))
        })
    }

    /// Scan a directory for extension subdirectories and load each.
    ///
    /// Each subdirectory should contain an `extension.yaml` file.
    /// Returns the list of loaded extension names.
    pub async fn load_directory(&self, dir: &str) -> Result<Vec<String>> {
        let dir_path = Path::new(dir);
        if !dir_path.is_dir() {
            return Err(Error::Config(format!(
                "extensions directory does not exist: {dir}"
            )));
        }

        let entries = std::fs::read_dir(dir_path).map_err(|e| {
            Error::Config(format!("failed to read extensions directory {dir}: {e}"))
        })?;

        let mut loaded = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| {
                Error::Config(format!("failed to read directory entry in {dir}: {e}"))
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("extension.yaml");
            if !manifest_path.exists() {
                tracing::debug!(
                    dir = %path.display(),
                    "skipping directory — no extension.yaml"
                );
                continue;
            }

            match Self::parse_manifest(&manifest_path) {
                Ok(manifest) => {
                    let name = manifest.name.clone();
                    match self.load_manifest(&manifest).await {
                        Ok(()) => {
                            tracing::info!(
                                extension = %name,
                                version = %manifest.version,
                                "loaded extension"
                            );
                            loaded.push(name);
                        }
                        Err(e) => {
                            tracing::warn!(
                                extension = %name,
                                error = %e,
                                "failed to load extension"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        path = %manifest_path.display(),
                        error = %e,
                        "failed to parse extension manifest"
                    );
                }
            }
        }

        loaded.sort();
        Ok(loaded)
    }

    /// Load a single extension manifest into the database.
    ///
    /// Inserts domains, entity types, relationship types, view edges,
    /// noise patterns, domain rules, domain groups, alignment rules,
    /// and hooks.  All inserts are idempotent (ON CONFLICT DO NOTHING).
    pub async fn load_manifest(&self, manifest: &ExtensionManifest) -> Result<()> {
        let pool = self.repo.pool();

        // 1. Domains
        for domain in &manifest.domains {
            sqlx::query(
                "INSERT INTO ontology_domains \
                     (id, label, description, is_internal) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(&domain.id)
            .bind(&domain.label)
            .bind(&domain.description)
            .bind(domain.is_internal)
            .execute(pool)
            .await?;
        }

        // 2. Entity types
        for et in &manifest.entity_types {
            sqlx::query(
                "INSERT INTO ontology_entity_types \
                     (id, category, label, description) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(&et.id)
            .bind(&et.category)
            .bind(&et.label)
            .bind(&et.description)
            .execute(pool)
            .await?;
        }

        // 3. Relationship types
        for rt in &manifest.relationship_types {
            sqlx::query(
                "INSERT INTO ontology_rel_types \
                     (id, universal, label, description) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(&rt.id)
            .bind(&rt.universal)
            .bind(&rt.label)
            .bind(&rt.description)
            .execute(pool)
            .await?;
        }

        // 4. View edges
        for (view, rel_types) in &manifest.view_edges {
            for rel_type in rel_types {
                sqlx::query(
                    "INSERT INTO ontology_view_edges \
                         (view_name, rel_type) \
                     VALUES ($1, $2) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(view)
                .bind(rel_type)
                .execute(pool)
                .await?;
            }
        }

        // 5. Noise patterns (no unique constraint, so check for
        //    existence before inserting)
        for np in &manifest.noise_patterns {
            let exists: bool = sqlx::query(
                "SELECT EXISTS( \
                     SELECT 1 FROM ontology_noise_patterns \
                     WHERE pattern = $1 AND pattern_type = $2 \
                 )",
            )
            .bind(&np.pattern)
            .bind(&np.pattern_type)
            .fetch_one(pool)
            .await?
            .get(0);

            if !exists {
                sqlx::query(
                    "INSERT INTO ontology_noise_patterns \
                         (pattern, pattern_type, description) \
                     VALUES ($1, $2, $3)",
                )
                .bind(&np.pattern)
                .bind(&np.pattern_type)
                .bind(&np.description)
                .execute(pool)
                .await?;
            }
        }

        // 6. Domain rules
        for rule in &manifest.domain_rules {
            // Check for duplicate (match_type, match_value,
            // domain_id) to avoid inserting duplicates.
            let exists: bool = sqlx::query(
                "SELECT EXISTS( \
                     SELECT 1 FROM domain_rules \
                     WHERE match_type = $1 \
                       AND match_value = $2 \
                       AND domain_id = $3 \
                 )",
            )
            .bind(&rule.match_type)
            .bind(&rule.match_value)
            .bind(&rule.domain_id)
            .fetch_one(pool)
            .await?
            .get(0);

            if !exists {
                sqlx::query(
                    "INSERT INTO domain_rules \
                         (priority, match_type, match_value, \
                          domain_id, description) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(rule.priority)
                .bind(&rule.match_type)
                .bind(&rule.match_value)
                .bind(&rule.domain_id)
                .bind(&rule.description)
                .execute(pool)
                .await?;
            }
        }

        // 7. Domain groups
        for (group_name, domain_ids) in &manifest.domain_groups {
            for (idx, domain_id) in domain_ids.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO domain_groups \
                         (group_name, domain_id, sort_order) \
                     VALUES ($1, $2, $3) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(group_name)
                .bind(domain_id)
                .bind(idx as i32)
                .execute(pool)
                .await?;
            }
        }

        // 8. Alignment rules
        for rule in &manifest.alignment_rules {
            sqlx::query(
                "INSERT INTO alignment_rules \
                     (name, description, check_type, \
                      source_group, target_group, parameters) \
                 VALUES ($1, $2, $3, $4, $5, $6) \
                 ON CONFLICT (name) DO NOTHING",
            )
            .bind(&rule.name)
            .bind(&rule.description)
            .bind(&rule.check_type)
            .bind(&rule.source_group)
            .bind(&rule.target_group)
            .bind(&rule.parameters)
            .execute(pool)
            .await?;
        }

        // 9. Hooks
        for hook in &manifest.hooks {
            // Check for duplicate (phase, hook_url) to avoid
            // inserting the same hook twice.
            let exists: bool = sqlx::query(
                "SELECT EXISTS( \
                     SELECT 1 FROM lifecycle_hooks \
                     WHERE phase = $1 AND hook_url = $2 \
                 )",
            )
            .bind(&hook.phase)
            .bind(&hook.url)
            .fetch_one(pool)
            .await?
            .get(0);

            if !exists {
                let id = uuid::Uuid::new_v4();
                let name = format!("{}:{}", manifest.name, hook.phase);
                sqlx::query(
                    "INSERT INTO lifecycle_hooks \
                         (id, name, phase, hook_url, \
                          timeout_ms, fail_open, is_active) \
                     VALUES ($1, $2, $3, $4, $5, $6, true)",
                )
                .bind(id)
                .bind(&name)
                .bind(&hook.phase)
                .bind(&hook.url)
                .bind(hook.timeout_ms)
                .bind(hook.fail_open)
                .execute(pool)
                .await?;
            }
        }

        Ok(())
    }

    /// List the names of extensions found in a directory without
    /// loading them.
    pub fn list_available(dir: &str) -> Result<Vec<String>> {
        let dir_path = Path::new(dir);
        if !dir_path.is_dir() {
            return Ok(Vec::new());
        }

        let entries = std::fs::read_dir(dir_path).map_err(|e| {
            Error::Config(format!("failed to read extensions directory {dir}: {e}"))
        })?;

        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                Error::Config(format!("failed to read directory entry in {dir}: {e}"))
            })?;
            let path = entry.path();
            let manifest_path = path.join("extension.yaml");
            if path.is_dir() && manifest_path.exists() {
                if let Ok(manifest) = Self::parse_manifest(&manifest_path) {
                    names.push(manifest.name);
                }
            }
        }

        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_extension(dir: &Path, name: &str, yaml: &str) {
        let ext_dir = dir.join(name);
        fs::create_dir_all(&ext_dir).unwrap();
        fs::write(ext_dir.join("extension.yaml"), yaml).unwrap();
    }

    #[test]
    fn parse_valid_manifest() {
        let dir = TempDir::new().unwrap();
        let yaml = r#"
name: test-ext
version: "1.0.0"
description: "Test extension"
"#;
        write_extension(dir.path(), "test-ext", yaml);
        let manifest =
            ExtensionLoader::parse_manifest(&dir.path().join("test-ext/extension.yaml")).unwrap();
        assert_eq!(manifest.name, "test-ext");
        assert_eq!(manifest.version, "1.0.0");
    }

    #[test]
    fn parse_invalid_yaml_returns_error() {
        let dir = TempDir::new().unwrap();
        let yaml = "not: [valid: yaml: here";
        write_extension(dir.path(), "bad-ext", yaml);
        let result = ExtensionLoader::parse_manifest(&dir.path().join("bad-ext/extension.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_missing_file_returns_error() {
        let result = ExtensionLoader::parse_manifest(Path::new("/nonexistent/extension.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn list_available_finds_extensions() {
        let dir = TempDir::new().unwrap();
        write_extension(dir.path(), "alpha", "name: alpha\nversion: '1.0.0'");
        write_extension(dir.path(), "beta", "name: beta\nversion: '2.0.0'");
        // Create a non-extension directory (no extension.yaml)
        fs::create_dir_all(dir.path().join("not-an-ext")).unwrap();
        // Create a file (not a directory)
        fs::write(dir.path().join("readme.txt"), "hello").unwrap();

        let names = ExtensionLoader::list_available(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn list_available_empty_on_missing_dir() {
        let names = ExtensionLoader::list_available("/nonexistent/dir").unwrap();
        assert!(names.is_empty());
    }

    /// Verify the shipped default extensions all parse without errors.
    #[test]
    fn default_extensions_parse_successfully() {
        // The extensions directory is at the repo root, three levels
        // up from engine/crates/covalence-core/.
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent());

        let ext_dir = match repo_root {
            Some(root) => root.join("extensions"),
            None => return, // Skip if we can't find the repo root.
        };

        if !ext_dir.is_dir() {
            return; // Skip if extensions dir doesn't exist.
        }

        let names = ExtensionLoader::list_available(ext_dir.to_str().unwrap()).unwrap();
        assert!(!names.is_empty(), "expected at least one default extension");

        for entry in fs::read_dir(&ext_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let manifest_path = path.join("extension.yaml");
            if path.is_dir() && manifest_path.exists() {
                let manifest =
                    ExtensionLoader::parse_manifest(&manifest_path).unwrap_or_else(|e| {
                        panic!("default extension {} failed to parse: {e}", path.display())
                    });
                assert!(
                    !manifest.name.is_empty(),
                    "extension in {} has empty name",
                    path.display()
                );
                assert!(
                    !manifest.version.is_empty(),
                    "extension {} has empty version",
                    manifest.name
                );
            }
        }
    }

    /// Verify idempotent loading: calling `list_available` twice
    /// returns the same results.
    #[test]
    fn list_available_is_deterministic() {
        let dir = TempDir::new().unwrap();
        write_extension(dir.path(), "gamma", "name: gamma\nversion: '1.0.0'");
        write_extension(dir.path(), "delta", "name: delta\nversion: '1.0.0'");

        let first = ExtensionLoader::list_available(dir.path().to_str().unwrap()).unwrap();
        let second = ExtensionLoader::list_available(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn parse_manifest_with_all_sections() {
        let yaml = r#"
name: complete
version: "3.0.0"

domains:
  - id: dom1
    label: Domain 1
    is_internal: true

entity_types:
  - id: et1
    category: concept
    label: ET1

relationship_types:
  - id: rt1
    universal: uses
    label: RT1

view_edges:
  test_view:
    - rt1

noise_patterns:
  - pattern: "noise"

domain_rules:
  - match_type: source_type
    match_value: code
    domain_id: dom1

domain_groups:
  grp1:
    - dom1

alignment_rules:
  - name: rule1
    check_type: ahead
    source_group: grp1
    target_group: grp1

hooks:
  - phase: pre_search
    url: "http://localhost:8080"

config_schema:
  key1:
    type: string
    default: "hello"
"#;
        let manifest: ExtensionManifest = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(manifest.name, "complete");
        assert_eq!(manifest.domains.len(), 1);
        assert_eq!(manifest.entity_types.len(), 1);
        assert_eq!(manifest.relationship_types.len(), 1);
        assert_eq!(manifest.view_edges.len(), 1);
        assert_eq!(manifest.noise_patterns.len(), 1);
        assert_eq!(manifest.domain_rules.len(), 1);
        assert_eq!(manifest.domain_groups.len(), 1);
        assert_eq!(manifest.alignment_rules.len(), 1);
        assert_eq!(manifest.hooks.len(), 1);
        assert_eq!(manifest.config_schema.len(), 1);
    }
}
