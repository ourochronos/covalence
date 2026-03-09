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
