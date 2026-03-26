//! Extension management DTOs.

use serde::Serialize;
use utoipa::ToSchema;

/// Response from listing loaded extensions.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListExtensionsResponse {
    /// Names of available extensions found on disk.
    pub extensions: Vec<String>,
}

/// Response from reloading extensions.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReloadExtensionsResponse {
    /// Names of extensions that were loaded.
    pub loaded: Vec<String>,
}
