//! Newtype wrappers for domain identifiers.
//!
//! Prevents accidental mixing of UUIDs from different domains.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Create a new random ID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Create from a raw UUID.
            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// Get the inner UUID.
            pub fn into_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl sqlx::Type<sqlx::Postgres> for $name {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <Uuid as sqlx::Type<sqlx::Postgres>>::type_info()
            }

            fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
                <Uuid as sqlx::Type<sqlx::Postgres>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <Uuid as sqlx::Encode<'q, sqlx::Postgres>>::encode_by_ref(
                    &self.0,
                    buf,
                )
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $name {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let uuid = <Uuid as sqlx::Decode<'r, sqlx::Postgres>>::decode(value)?;
                Ok(Self(uuid))
            }
        }
    };
}

define_id!(
    /// Unique identifier for a source record.
    SourceId
);

define_id!(
    /// Unique identifier for a text chunk.
    ChunkId
);

define_id!(
    /// Unique identifier for a graph node.
    NodeId
);

define_id!(
    /// Unique identifier for a graph edge.
    EdgeId
);

define_id!(
    /// Unique identifier for a compiled article.
    ArticleId
);

define_id!(
    /// Unique identifier for an extraction record.
    ExtractionId
);

define_id!(
    /// Unique identifier for a node alias.
    AliasId
);

define_id!(
    /// Unique identifier for an audit log entry.
    AuditLogId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_generates_unique_ids() {
        let a = NodeId::new();
        let b = NodeId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn from_uuid_roundtrip() {
        let uuid = Uuid::new_v4();
        let id = SourceId::from_uuid(uuid);
        assert_eq!(id.into_uuid(), uuid);
    }

    #[test]
    fn from_into_uuid() {
        let uuid = Uuid::new_v4();
        let id: EdgeId = uuid.into();
        let back: Uuid = id.into();
        assert_eq!(uuid, back);
    }

    #[test]
    fn display_matches_inner_uuid() {
        let uuid = Uuid::new_v4();
        let id = ChunkId::from_uuid(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn serde_roundtrip() {
        let id = ArticleId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: ArticleId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn hash_eq_consistency() {
        use std::collections::HashSet;
        let uuid = Uuid::new_v4();
        let a = NodeId::from_uuid(uuid);
        let b = NodeId::from_uuid(uuid);
        assert_eq!(a, b);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }
}
