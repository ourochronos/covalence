//! Shared DTOs used across multiple handlers.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Common pagination query parameters.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct PaginationParams {
    /// Maximum number of results (default 20).
    pub limit: Option<i64>,
    /// Offset for pagination (default 0).
    pub offset: Option<i64>,
}

/// Maximum allowed pagination limit.
const MAX_PAGINATION_LIMIT: i64 = 1000;

impl PaginationParams {
    /// Get limit with default, capped at 1000.
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(20).clamp(1, MAX_PAGINATION_LIMIT)
    }

    /// Get offset with default, floored at 0.
    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }
}

/// Generic response for curation operations.
#[derive(Debug, Serialize, ToSchema)]
pub struct CurationResponse {
    /// Whether the operation succeeded.
    pub success: bool,
    /// The audit log entry ID for the operation.
    pub audit_log_id: Uuid,
}

/// Generic response for feedback submission.
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedbackResponse {
    /// Whether the feedback was recorded.
    pub recorded: bool,
}

/// Response for an audit log entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditLogResponse {
    pub id: Uuid,
    pub action: String,
    pub actor: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_limit_capped_at_max() {
        let params = PaginationParams {
            limit: Some(999_999),
            offset: Some(0),
        };
        assert_eq!(params.limit(), MAX_PAGINATION_LIMIT);
    }

    #[test]
    fn pagination_negative_offset_clamped() {
        let params = PaginationParams {
            limit: None,
            offset: Some(-5),
        };
        assert_eq!(params.offset(), 0);
    }

    #[test]
    fn pagination_zero_limit_clamped_to_one() {
        let params = PaginationParams {
            limit: Some(0),
            offset: None,
        };
        assert_eq!(params.limit(), 1);
    }
}
