//! HTTP extractors — shared across all route handlers.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// Namespace extracted from the `X-Namespace` request header.
///
/// Defaults to `"default"` when the header is absent or its value is empty /
/// non-UTF-8.  Callers can discriminate tenants by sending a custom header:
///
/// ```http
/// POST /sources HTTP/1.1
/// X-Namespace: my-project
/// ```
///
/// # Usage in handlers
///
/// ```rust,ignore
/// async fn my_handler(
///     State(state): State<AppState>,
///     Namespace(ns): Namespace,
///     Json(req): Json<MyRequest>,
/// ) -> impl IntoResponse {
///     let svc = MyService::new(state.pool).with_namespace(ns);
///     …
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Namespace(pub String);

impl<S: Send + Sync> FromRequestParts<S> for Namespace {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ns = parts
            .headers
            .get("x-namespace")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string();
        Ok(Namespace(ns))
    }
}
