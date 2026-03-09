//! Stage 1: Accept source.
//!
//! Computes content hash for dedup, creates source record,
//! routes to update handling if source already exists.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::ids::SourceId;

/// Result of the accept stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AcceptResult {
    /// Brand-new content never seen before.
    New {
        /// Assigned source identifier.
        source_id: SourceId,
        /// SHA-256 hash of the content.
        hash: Vec<u8>,
    },
    /// Content hash matches an existing source exactly.
    Duplicate {
        /// Identifier of the existing source.
        existing_id: SourceId,
    },
    /// Same URI but content has changed.
    Update {
        /// Identifier of the source being updated.
        source_id: SourceId,
        /// Hash of the previous version's content.
        previous_hash: Vec<u8>,
    },
}

/// Compute the SHA-256 hash of raw content bytes.
pub fn compute_content_hash(content: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        let a = compute_content_hash(b"hello world");
        let b = compute_content_hash(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_differs_for_different_input() {
        let a = compute_content_hash(b"hello");
        let b = compute_content_hash(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_32_bytes() {
        let h = compute_content_hash(b"test");
        assert_eq!(h.len(), 32);
    }

    #[test]
    fn empty_input_produces_valid_hash() {
        let h = compute_content_hash(b"");
        assert_eq!(h.len(), 32);
    }
}
