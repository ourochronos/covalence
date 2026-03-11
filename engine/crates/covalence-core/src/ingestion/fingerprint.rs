//! Pipeline fingerprinting — records the pipeline configuration
//! that produced each ingestion run.
//!
//! Each stage hash captures the config parameters that affect that
//! stage's output. When fingerprints differ between runs, the
//! per-stage hashes identify which stages changed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::config::PipelineConfig;

/// Fingerprint of the pipeline configuration used for an ingestion
/// run.
///
/// Each stage hash captures the config parameters that affect that
/// stage's output. When fingerprints differ between ingestion runs,
/// the per-stage hashes identify which stages changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipelineFingerprint {
    /// Hash of conversion config (converter type, sidecar URLs).
    pub conversion_hash: u64,
    /// Hash of preprocessing config (normalize, coref).
    pub preprocessing_hash: u64,
    /// Hash of chunking params (chunk_size, chunk_overlap).
    pub chunking_hash: u64,
    /// Hash of extraction config (backend, model, thresholds).
    pub extraction_hash: u64,
    /// Hash of resolution config (thresholds, embed model).
    pub resolution_hash: u64,
    /// Combined hash of all stages.
    pub combined_hash: u64,
}

/// Per-stage drift report between two pipeline fingerprints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FingerprintDrift {
    /// Whether conversion config changed.
    pub conversion_changed: bool,
    /// Whether preprocessing config changed.
    pub preprocessing_changed: bool,
    /// Whether chunking params changed.
    pub chunking_changed: bool,
    /// Whether extraction config changed.
    pub extraction_changed: bool,
    /// Whether resolution config changed.
    pub resolution_changed: bool,
}

/// Config parameters needed to compute a pipeline fingerprint.
///
/// These are the subset of application config that affects
/// ingestion output. Collected from `Config` and `SourceService`
/// fields at service construction time.
#[derive(Debug, Clone)]
pub struct FingerprintConfig {
    /// Whether format conversion is enabled.
    pub convert_enabled: bool,
    /// Whether a ReaderLM sidecar is configured.
    pub has_readerlm: bool,
    /// Whether a PDF sidecar is configured.
    pub has_pdf_sidecar: bool,
    /// Whether text normalization is enabled.
    pub normalize_enabled: bool,
    /// Whether coreference resolution is enabled.
    pub coref_enabled: bool,
    /// Whether a coref sidecar URL is configured.
    pub has_coref_url: bool,
    /// Chunk size in bytes.
    pub chunk_size: usize,
    /// Chunk overlap in characters.
    pub chunk_overlap: usize,
    /// Entity extractor backend name (e.g., "llm", "gliner2").
    pub entity_extractor: String,
    /// Chat model used for LLM extraction.
    pub chat_model: String,
    /// Minimum token count for extraction.
    pub min_extract_tokens: usize,
    /// Token budget for extraction batching.
    pub extract_batch_tokens: usize,
    /// Whether landscape gating is enabled.
    pub landscape_enabled: bool,
    /// Whether entity resolution is enabled.
    pub resolve_enabled: bool,
    /// Trigram similarity threshold for resolution.
    pub trigram_threshold: f32,
    /// Vector cosine threshold for resolution.
    pub vector_threshold: f32,
    /// Embedding model name.
    pub embed_model: String,
}

impl FingerprintDrift {
    /// Returns `true` if any stage changed.
    pub fn has_drift(&self) -> bool {
        self.conversion_changed
            || self.preprocessing_changed
            || self.chunking_changed
            || self.extraction_changed
            || self.resolution_changed
    }

    /// Returns the names of stages that changed.
    pub fn changed_stages(&self) -> Vec<&'static str> {
        let mut stages = Vec::new();
        if self.conversion_changed {
            stages.push("conversion");
        }
        if self.preprocessing_changed {
            stages.push("preprocessing");
        }
        if self.chunking_changed {
            stages.push("chunking");
        }
        if self.extraction_changed {
            stages.push("extraction");
        }
        if self.resolution_changed {
            stages.push("resolution");
        }
        stages
    }
}

impl PipelineFingerprint {
    /// Compute a pipeline fingerprint from the current config.
    pub fn compute(cfg: &FingerprintConfig) -> Self {
        let conversion_hash = hash_conversion(cfg);
        let preprocessing_hash = hash_preprocessing(cfg);
        let chunking_hash = hash_chunking(cfg);
        let extraction_hash = hash_extraction(cfg);
        let resolution_hash = hash_resolution(cfg);

        let combined_hash = {
            let mut h = DefaultHasher::new();
            conversion_hash.hash(&mut h);
            preprocessing_hash.hash(&mut h);
            chunking_hash.hash(&mut h);
            extraction_hash.hash(&mut h);
            resolution_hash.hash(&mut h);
            h.finish()
        };

        Self {
            conversion_hash,
            preprocessing_hash,
            chunking_hash,
            extraction_hash,
            resolution_hash,
            combined_hash,
        }
    }

    /// Compare this fingerprint against another, returning a
    /// per-stage drift report.
    pub fn compare(&self, other: &Self) -> FingerprintDrift {
        FingerprintDrift {
            conversion_changed: self.conversion_hash != other.conversion_hash,
            preprocessing_changed: self.preprocessing_hash != other.preprocessing_hash,
            chunking_changed: self.chunking_hash != other.chunking_hash,
            extraction_changed: self.extraction_hash != other.extraction_hash,
            resolution_changed: self.resolution_hash != other.resolution_hash,
        }
    }

    /// Serialize this fingerprint as a `serde_json::Value` suitable
    /// for embedding in source metadata.
    pub fn to_json(&self) -> serde_json::Value {
        // serde_json::to_value cannot fail for this type since all
        // fields are simple primitives.
        serde_json::to_value(self).unwrap_or_default()
    }

    /// Deserialize a fingerprint from a `serde_json::Value`
    /// previously stored in source metadata.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

/// Build a [`FingerprintConfig`] from the top-level application
/// config.
#[allow(clippy::too_many_arguments)]
pub fn fingerprint_config_from(
    pipeline: &PipelineConfig,
    chunk_size: usize,
    chunk_overlap: usize,
    entity_extractor: &str,
    chat_model: &str,
    min_extract_tokens: usize,
    extract_batch_tokens: usize,
    resolve_trigram_threshold: f32,
    resolve_vector_threshold: f32,
    embed_model: &str,
    has_readerlm: bool,
    has_pdf_sidecar: bool,
    has_coref_url: bool,
) -> FingerprintConfig {
    FingerprintConfig {
        convert_enabled: pipeline.convert_enabled,
        has_readerlm,
        has_pdf_sidecar,
        normalize_enabled: pipeline.normalize_enabled,
        coref_enabled: pipeline.coref_enabled,
        has_coref_url,
        chunk_size,
        chunk_overlap,
        entity_extractor: entity_extractor.to_string(),
        chat_model: chat_model.to_string(),
        min_extract_tokens,
        extract_batch_tokens,
        landscape_enabled: pipeline.landscape_enabled,
        resolve_enabled: pipeline.resolve_enabled,
        trigram_threshold: resolve_trigram_threshold,
        vector_threshold: resolve_vector_threshold,
        embed_model: embed_model.to_string(),
    }
}

// --- per-stage hash functions ---

/// Hash conversion stage config.
fn hash_conversion(cfg: &FingerprintConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.convert_enabled.hash(&mut h);
    cfg.has_readerlm.hash(&mut h);
    cfg.has_pdf_sidecar.hash(&mut h);
    h.finish()
}

/// Hash preprocessing stage config.
fn hash_preprocessing(cfg: &FingerprintConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.normalize_enabled.hash(&mut h);
    cfg.coref_enabled.hash(&mut h);
    cfg.has_coref_url.hash(&mut h);
    h.finish()
}

/// Hash chunking stage config.
fn hash_chunking(cfg: &FingerprintConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.chunk_size.hash(&mut h);
    cfg.chunk_overlap.hash(&mut h);
    h.finish()
}

/// Hash extraction stage config.
fn hash_extraction(cfg: &FingerprintConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.entity_extractor.hash(&mut h);
    cfg.chat_model.hash(&mut h);
    cfg.min_extract_tokens.hash(&mut h);
    cfg.extract_batch_tokens.hash(&mut h);
    cfg.landscape_enabled.hash(&mut h);
    h.finish()
}

/// Hash resolution stage config.
fn hash_resolution(cfg: &FingerprintConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.resolve_enabled.hash(&mut h);
    // Hash f32 thresholds via their bit patterns to avoid
    // floating-point comparison issues.
    cfg.trigram_threshold.to_bits().hash(&mut h);
    cfg.vector_threshold.to_bits().hash(&mut h);
    cfg.embed_model.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a default FingerprintConfig for testing.
    fn default_config() -> FingerprintConfig {
        FingerprintConfig {
            convert_enabled: true,
            has_readerlm: false,
            has_pdf_sidecar: false,
            normalize_enabled: true,
            coref_enabled: true,
            has_coref_url: false,
            chunk_size: 1000,
            chunk_overlap: 200,
            entity_extractor: "llm".to_string(),
            chat_model: "gpt-4o".to_string(),
            min_extract_tokens: 30,
            extract_batch_tokens: 2000,
            landscape_enabled: true,
            resolve_enabled: true,
            trigram_threshold: 0.4,
            vector_threshold: 0.85,
            embed_model: "text-embedding-3-large".to_string(),
        }
    }

    #[test]
    fn compute_is_deterministic() {
        let cfg = default_config();
        let a = PipelineFingerprint::compute(&cfg);
        let b = PipelineFingerprint::compute(&cfg);
        assert_eq!(a, b);
    }

    #[test]
    fn identical_configs_produce_no_drift() {
        let cfg = default_config();
        let a = PipelineFingerprint::compute(&cfg);
        let b = PipelineFingerprint::compute(&cfg);
        let drift = a.compare(&b);
        assert!(!drift.has_drift());
        assert!(drift.changed_stages().is_empty());
    }

    #[test]
    fn changing_chunk_size_only_affects_chunking_stage() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.chunk_size = 2000;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.chunking_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert!(!drift.extraction_changed);
        assert!(!drift.resolution_changed);
        assert_eq!(drift.changed_stages(), vec!["chunking"]);
    }

    #[test]
    fn changing_chunk_overlap_only_affects_chunking_stage() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.chunk_overlap = 400;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.chunking_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert!(!drift.extraction_changed);
        assert!(!drift.resolution_changed);
    }

    #[test]
    fn changing_extractor_only_affects_extraction_stage() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.entity_extractor = "gliner2".to_string();

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.extraction_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert!(!drift.chunking_changed);
        assert!(!drift.resolution_changed);
        assert_eq!(drift.changed_stages(), vec!["extraction"]);
    }

    #[test]
    fn changing_chat_model_only_affects_extraction_stage() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.chat_model = "gpt-4o-mini".to_string();

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.extraction_changed);
        assert!(!drift.chunking_changed);
        assert!(!drift.resolution_changed);
    }

    #[test]
    fn changing_landscape_only_affects_extraction_stage() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.landscape_enabled = false;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.extraction_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert!(!drift.chunking_changed);
        assert!(!drift.resolution_changed);
    }

    #[test]
    fn changing_resolve_threshold_only_affects_resolution() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.trigram_threshold = 0.5;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.resolution_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert!(!drift.chunking_changed);
        assert!(!drift.extraction_changed);
        assert_eq!(drift.changed_stages(), vec!["resolution"]);
    }

    #[test]
    fn changing_vector_threshold_only_affects_resolution() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.vector_threshold = 0.90;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.resolution_changed);
        assert!(!drift.extraction_changed);
    }

    #[test]
    fn changing_embed_model_only_affects_resolution() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.embed_model = "voyage-context-3".to_string();

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.resolution_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.extraction_changed);
    }

    #[test]
    fn changing_convert_enabled_only_affects_conversion() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.convert_enabled = false;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert!(!drift.chunking_changed);
        assert!(!drift.extraction_changed);
        assert!(!drift.resolution_changed);
        assert_eq!(drift.changed_stages(), vec!["conversion"]);
    }

    #[test]
    fn enabling_readerlm_only_affects_conversion() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.has_readerlm = true;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
    }

    #[test]
    fn enabling_pdf_sidecar_only_affects_conversion() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.has_pdf_sidecar = true;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
    }

    #[test]
    fn changing_normalize_only_affects_preprocessing() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.normalize_enabled = false;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.preprocessing_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.chunking_changed);
        assert_eq!(drift.changed_stages(), vec!["preprocessing"]);
    }

    #[test]
    fn changing_coref_only_affects_preprocessing() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.coref_enabled = false;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.preprocessing_changed);
        assert!(!drift.conversion_changed);
    }

    #[test]
    fn enabling_coref_url_only_affects_preprocessing() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.has_coref_url = true;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.preprocessing_changed);
        assert!(!drift.conversion_changed);
    }

    #[test]
    fn changing_resolve_enabled_only_affects_resolution() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.resolve_enabled = false;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.resolution_changed);
        assert!(!drift.extraction_changed);
    }

    #[test]
    fn changing_min_extract_tokens_only_affects_extraction() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.min_extract_tokens = 50;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.extraction_changed);
        assert!(!drift.chunking_changed);
        assert!(!drift.resolution_changed);
    }

    #[test]
    fn changing_batch_tokens_only_affects_extraction() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.extract_batch_tokens = 4000;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.extraction_changed);
        assert!(!drift.chunking_changed);
    }

    #[test]
    fn multiple_stages_can_drift_at_once() {
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        cfg_b.chunk_size = 500;
        cfg_b.entity_extractor = "sidecar".to_string();
        cfg_b.trigram_threshold = 0.6;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert!(drift.has_drift());
        assert!(drift.chunking_changed);
        assert!(drift.extraction_changed);
        assert!(drift.resolution_changed);
        assert!(!drift.conversion_changed);
        assert!(!drift.preprocessing_changed);
        assert_eq!(
            drift.changed_stages(),
            vec!["chunking", "extraction", "resolution"]
        );
    }

    #[test]
    fn combined_hash_changes_when_any_stage_changes() {
        let cfg_a = default_config();
        let fp_base = PipelineFingerprint::compute(&cfg_a);

        // Change only one stage at a time and verify combined
        // changes.
        let mut cfg_b = default_config();
        cfg_b.chunk_size = 2000;
        let fp_chunk = PipelineFingerprint::compute(&cfg_b);
        assert_ne!(fp_base.combined_hash, fp_chunk.combined_hash);

        let mut cfg_c = default_config();
        cfg_c.entity_extractor = "gliner2".to_string();
        let fp_extract = PipelineFingerprint::compute(&cfg_c);
        assert_ne!(fp_base.combined_hash, fp_extract.combined_hash);
    }

    #[test]
    fn json_round_trip() {
        let cfg = default_config();
        let fp = PipelineFingerprint::compute(&cfg);
        let json = fp.to_json();
        let restored = PipelineFingerprint::from_json(&json);
        assert_eq!(Some(fp), restored);
    }

    #[test]
    fn from_json_returns_none_for_invalid_data() {
        let bad = serde_json::json!({"not": "a fingerprint"});
        assert!(PipelineFingerprint::from_json(&bad).is_none());
    }

    #[test]
    fn from_json_returns_none_for_null() {
        let null = serde_json::Value::Null;
        assert!(PipelineFingerprint::from_json(&null).is_none());
    }

    #[test]
    fn fingerprint_config_from_helper() {
        let pipeline = PipelineConfig::default();
        let cfg = fingerprint_config_from(
            &pipeline,
            1000,
            200,
            "llm",
            "gpt-4o",
            30,
            2000,
            0.4,
            0.85,
            "text-embedding-3-large",
            false,
            false,
            false,
        );
        assert_eq!(cfg.chunk_size, 1000);
        assert_eq!(cfg.chunk_overlap, 200);
        assert_eq!(cfg.entity_extractor, "llm");
        assert!(cfg.convert_enabled);
        assert!(cfg.normalize_enabled);
    }

    #[test]
    fn drift_changed_stages_order_is_stable() {
        // Verify the order matches the pipeline order:
        // conversion, preprocessing, chunking, extraction,
        // resolution.
        let cfg_a = default_config();
        let mut cfg_b = default_config();
        // Change all stages.
        cfg_b.convert_enabled = false;
        cfg_b.normalize_enabled = false;
        cfg_b.chunk_size = 500;
        cfg_b.entity_extractor = "gliner2".to_string();
        cfg_b.trigram_threshold = 0.9;

        let a = PipelineFingerprint::compute(&cfg_a);
        let b = PipelineFingerprint::compute(&cfg_b);
        let drift = a.compare(&b);

        assert_eq!(
            drift.changed_stages(),
            vec![
                "conversion",
                "preprocessing",
                "chunking",
                "extraction",
                "resolution",
            ]
        );
    }
}
