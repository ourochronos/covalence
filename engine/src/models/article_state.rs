//! Typestate pattern for article lifecycle state transitions (covalence#93).
//!
//! Articles move through a defined lifecycle:
//!
//! ```text
//! Active ──archive()──► Archived
//!                            │
//!                     reactivate() (explicit, documented)
//!                            │
//!                            ▼
//!                         Active
//! ```
//!
//! Using Rust's type system, each lifecycle state is encoded as a distinct type
//! parameter on [`TypedArticle<S>`].  Transition methods are implemented **only**
//! on the relevant state, so invalid transitions are **compile errors**, not
//! runtime bugs.
//!
//! # Quick Example
//!
//! ```rust,no_run
//! use covalence_engine::models::article_state::{Active, Archived, TypedArticle};
//! use covalence_engine::services::article_service::ArticleResponse;
//!
//! fn make_active_response() -> ArticleResponse { todo!() }
//!
//! let data: ArticleResponse = make_active_response(); // status == "active"
//!
//! // Wrap in a compile-time typed handle.
//! let active: TypedArticle<Active> = TypedArticle::<Active>::from_response(data).unwrap();
//!
//! // Transition to Archived — consumes the Active handle.
//! let archived: TypedArticle<Archived> = active.archive();
//!
//! // ✗ The following line does NOT compile because TypedArticle<Archived>
//! //   has no `.archive()` method:
//! //
//! // let _again = archived.archive(); // error[E0599]: no method named `archive`
//! ```

use std::marker::PhantomData;

use crate::services::article_service::ArticleResponse;

// ---------------------------------------------------------------------------
// Sealed-trait infrastructure
// ---------------------------------------------------------------------------

/// Private module that seals the [`ArticleState`] trait.
/// Only types defined in *this* crate can implement [`ArticleState`].
mod sealed {
    pub trait Sealed {}
}

/// Marker trait implemented by valid article lifecycle states.
///
/// This trait is *sealed* — it cannot be implemented outside this crate.
/// That prevents callers from inventing new state types and bypassing the
/// transition rules.
pub trait ArticleState: sealed::Sealed {
    /// The lowercase status string stored in the database / returned in API
    /// responses.
    fn status_str() -> &'static str;
}

// ---------------------------------------------------------------------------
// State marker types
// ---------------------------------------------------------------------------

/// Marker type for the **Active** lifecycle state.
///
/// An article in this state is live and visible in normal queries.
/// It can be archived via [`TypedArticle::<Active>::archive`].
pub struct Active;

/// Marker type for the **Archived** lifecycle state.
///
/// An article in this state has been soft-deleted.  It is retained for
/// provenance history but excluded from normal queries.  It can be explicitly
/// reactivated via [`TypedArticle::<Archived>::reactivate`].
pub struct Archived;

impl sealed::Sealed for Active {}
impl sealed::Sealed for Archived {}

impl ArticleState for Active {
    fn status_str() -> &'static str {
        "active"
    }
}

impl ArticleState for Archived {
    fn status_str() -> &'static str {
        "archived"
    }
}

// ---------------------------------------------------------------------------
// TypedArticle<S> — the typestate wrapper
// ---------------------------------------------------------------------------

/// An article handle whose **lifecycle state is encoded in the type parameter
/// `S`**.
///
/// `TypedArticle<Active>` and `TypedArticle<Archived>` are distinct types.
/// The compiler rejects any code that attempts a transition not defined for
/// the current state (e.g. calling `.archive()` on an already-archived
/// article).
///
/// # Database Interaction
///
/// `TypedArticle<S>` is a **pure type-level wrapper** around [`ArticleResponse`].
/// It does *not* perform database writes.  Use it alongside
/// [`ArticleService`](crate::services::article_service::ArticleService) to
/// validate state before issuing the corresponding SQL update.
///
/// See [`ArticleService::typed_archive`] for the canonical end-to-end
/// archive path.
pub struct TypedArticle<S: ArticleState> {
    /// The underlying article data returned from / to be persisted in the DB.
    pub data: ArticleResponse,
    _state: PhantomData<fn() -> S>,
}

// Shared methods available regardless of state.
impl<S: ArticleState> TypedArticle<S> {
    /// Borrow the underlying [`ArticleResponse`].
    pub fn as_data(&self) -> &ArticleResponse {
        &self.data
    }

    /// Consume the typed handle and return the inner [`ArticleResponse`].
    pub fn into_data(self) -> ArticleResponse {
        self.data
    }

    /// Return the compile-time database status string for state `S`.
    pub fn state_str() -> &'static str {
        S::status_str()
    }
}

// ---------------------------------------------------------------------------
// Active-state methods
// ---------------------------------------------------------------------------

impl TypedArticle<Active> {
    /// Construct a typed handle for an **Active** article.
    ///
    /// Returns `None` when the article's `status` field is not `"active"`,
    /// allowing safe construction at runtime boundaries (e.g., from a DB row).
    ///
    /// # Example
    /// ```ignore
    /// let article = service.get(id).await?;
    /// let active = TypedArticle::<Active>::from_response(article)
    ///     .ok_or_else(|| AppError::Conflict("article is not active".into()))?;
    /// ```
    pub fn from_response(data: ArticleResponse) -> Option<Self> {
        if data.status == "active" {
            Some(Self {
                data,
                _state: PhantomData,
            })
        } else {
            None
        }
    }

    /// **Lifecycle transition: Active → Archived.**
    ///
    /// Consumes `self` (preventing double-archiving at the type level) and
    /// returns a `TypedArticle<Archived>` whose `data.status` is set to
    /// `"archived"`.
    ///
    /// The caller is responsible for persisting the new state to the database
    /// (e.g. via [`ArticleService::delete`] /
    /// [`ArticleService::typed_archive`]).
    ///
    /// ```ignore
    /// // Compile-time guarantee: only Active articles can be archived.
    /// let archived: TypedArticle<Archived> = active_article.archive();
    ///
    /// // The original `active_article` has been consumed — it cannot be used
    /// // again, preventing use-after-archive bugs.
    /// ```
    pub fn archive(self) -> TypedArticle<Archived> {
        let mut data = self.data;
        data.status = "archived".to_string();
        TypedArticle {
            data,
            _state: PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// Archived-state methods
// ---------------------------------------------------------------------------

impl TypedArticle<Archived> {
    /// Construct a typed handle for an **Archived** article.
    ///
    /// Returns `None` when the article's `status` field is not `"archived"`.
    pub fn from_response(data: ArticleResponse) -> Option<Self> {
        if data.status == "archived" {
            Some(Self {
                data,
                _state: PhantomData,
            })
        } else {
            None
        }
    }

    /// **Explicit reactivation: Archived → Active.**
    ///
    /// Reactivation is an *intentional*, documented lifecycle event — not a
    /// normal state transition.  It must be called deliberately and requires
    /// a corresponding database update by the caller.
    ///
    /// Unlike `archive()`, reactivation is expected to be rare and should be
    /// accompanied by an audit trail entry.
    ///
    /// ```ignore
    /// let active: TypedArticle<Active> = archived_article.reactivate();
    /// // Persist: UPDATE nodes SET status = 'active' WHERE id = $1
    /// ```
    pub fn reactivate(self) -> TypedArticle<Active> {
        let mut data = self.data;
        data.status = "active".to_string();
        TypedArticle {
            data,
            _state: PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime dispatch helper
// ---------------------------------------------------------------------------

/// Runtime-dispatched lifecycle variant, for use at API boundaries where the
/// article state is not known until a database row is decoded.
///
/// Prefer the compile-time [`TypedArticle<S>`] form whenever the state is
/// known ahead of time.
pub enum ArticleLifecycleState {
    /// Article is live and queryable.
    Active(TypedArticle<Active>),
    /// Article is soft-deleted / archived.
    Archived(TypedArticle<Archived>),
    /// Status string was not recognised by this version of the library
    /// (e.g. `"tombstone"` written by a newer engine release).
    Unknown(ArticleResponse),
}

impl ArticleLifecycleState {
    /// Parse an [`ArticleResponse`] into the appropriate lifecycle variant.
    pub fn from_response(data: ArticleResponse) -> Self {
        match data.status.as_str() {
            "active" => ArticleLifecycleState::Active(TypedArticle {
                data,
                _state: PhantomData,
            }),
            "archived" => ArticleLifecycleState::Archived(TypedArticle {
                data,
                _state: PhantomData,
            }),
            _ => ArticleLifecycleState::Unknown(data),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_response(status: &str) -> ArticleResponse {
        ArticleResponse {
            id: Uuid::new_v4(),
            node_type: "article".into(),
            title: Some("Test Article".into()),
            content: Some("Hello, world.".into()),
            status: status.to_string(),
            confidence: 0.5,
            epistemic_type: None,
            domain_path: vec![],
            metadata: serde_json::json!({}),
            version: 1,
            pinned: false,
            usage_score: 0.0,
            contention_count: 0,
            content_hash: None,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            stale: None,
            facet_function: None,
            facet_scope: None,
        }
    }

    // -----------------------------------------------------------------------
    // AC1 — Active → Archived transition succeeds
    // -----------------------------------------------------------------------

    /// AC1: An Active article can be transitioned to Archived via the type system.
    #[test]
    fn test_active_to_archived_transition_succeeds() {
        let data = make_response("active");
        let article_id = data.id;

        // Construct the typed handle — must succeed for "active" status.
        let active = TypedArticle::<Active>::from_response(data)
            .expect("should construct TypedArticle<Active> from active status");

        assert_eq!(TypedArticle::<Active>::state_str(), "active");

        // Perform the type-safe transition.
        let archived: TypedArticle<Archived> = active.archive();

        // The status field must now reflect the new state.
        assert_eq!(archived.data.status, "archived");
        assert_eq!(archived.data.id, article_id, "id must be preserved");
        assert_eq!(TypedArticle::<Archived>::state_str(), "archived");
    }

    // -----------------------------------------------------------------------
    // AC1 — valid transition preserves article data
    // -----------------------------------------------------------------------

    /// Data integrity check: all fields other than `status` survive the
    /// `archive()` transition unchanged.
    #[test]
    fn test_archive_transition_preserves_data() {
        let mut data = make_response("active");
        data.title = Some("Important Article".into());
        data.version = 7;
        data.pinned = true;
        let original_id = data.id;

        let archived = TypedArticle::<Active>::from_response(data)
            .unwrap()
            .archive();

        assert_eq!(archived.data.id, original_id);
        assert_eq!(archived.data.title.as_deref(), Some("Important Article"));
        assert_eq!(archived.data.version, 7);
        assert!(archived.data.pinned);
        assert_eq!(archived.data.status, "archived");
    }

    // -----------------------------------------------------------------------
    // AC2 — Archived → Active via explicit reactivation
    // -----------------------------------------------------------------------

    /// AC2: Archived → Active is an *explicit* reactivation path, not a
    /// normal lifecycle transition.  This test documents the path and verifies
    /// that `reactivate()` produces a valid `TypedArticle<Active>`.
    #[test]
    fn test_archived_to_active_explicit_reactivation() {
        let data = make_response("archived");

        let archived = TypedArticle::<Archived>::from_response(data)
            .expect("should construct TypedArticle<Archived> from archived status");

        // Explicit, documented reactivation — not a normal lifecycle step.
        let reactivated: TypedArticle<Active> = archived.reactivate();

        assert_eq!(reactivated.data.status, "active");
    }

    // -----------------------------------------------------------------------
    // Construction guards
    // -----------------------------------------------------------------------

    #[test]
    fn test_active_from_response_rejects_archived_status() {
        let data = make_response("archived");
        let result = TypedArticle::<Active>::from_response(data);
        assert!(
            result.is_none(),
            "TypedArticle::<Active>::from_response should return None for archived status"
        );
    }

    #[test]
    fn test_archived_from_response_rejects_active_status() {
        let data = make_response("active");
        let result = TypedArticle::<Archived>::from_response(data);
        assert!(
            result.is_none(),
            "TypedArticle::<Archived>::from_response should return None for active status"
        );
    }

    // -----------------------------------------------------------------------
    // Round-trip: Active → Archived → Active
    // -----------------------------------------------------------------------

    #[test]
    fn test_round_trip_active_archived_active() {
        let data = make_response("active");
        let id = data.id;

        let reactivated = TypedArticle::<Active>::from_response(data)
            .unwrap()
            .archive() // Active → Archived
            .reactivate(); // Archived → Active (explicit)

        assert_eq!(reactivated.data.status, "active");
        assert_eq!(reactivated.data.id, id);
    }

    // -----------------------------------------------------------------------
    // Runtime dispatch helper
    // -----------------------------------------------------------------------

    #[test]
    fn test_article_lifecycle_state_from_response_active() {
        let data = make_response("active");
        match ArticleLifecycleState::from_response(data) {
            ArticleLifecycleState::Active(a) => assert_eq!(a.data.status, "active"),
            other => panic!("expected Active variant, got something else: status unknown"),
        }
    }

    #[test]
    fn test_article_lifecycle_state_from_response_archived() {
        let data = make_response("archived");
        match ArticleLifecycleState::from_response(data) {
            ArticleLifecycleState::Archived(a) => assert_eq!(a.data.status, "archived"),
            _ => panic!("expected Archived variant"),
        }
    }

    #[test]
    fn test_article_lifecycle_state_from_response_unknown() {
        let data = make_response("tombstone");
        match ArticleLifecycleState::from_response(data) {
            ArticleLifecycleState::Unknown(d) => assert_eq!(d.status, "tombstone"),
            _ => panic!("expected Unknown variant for unrecognised status"),
        }
    }

    // -----------------------------------------------------------------------
    // Compile-time guarantee (documented as a comment, not a runnable test —
    // the Rust compiler enforces this; no runtime check is possible/needed).
    // -----------------------------------------------------------------------

    /// The following code does NOT compile, demonstrating the typestate
    /// guarantee that `TypedArticle<Archived>` has no `.archive()` method:
    ///
    /// ```compile_fail
    /// use covalence_engine::models::article_state::{Active, Archived, TypedArticle};
    /// // … build a TypedArticle<Archived> somehow …
    /// # let archived: TypedArticle<Archived> = unimplemented!();
    /// let _oops = archived.archive(); // error[E0599]: no method named `archive` found
    /// ```
    ///
    /// Similarly, calling `.reactivate()` on an Active article is rejected:
    ///
    /// ```compile_fail
    /// use covalence_engine::models::article_state::{Active, TypedArticle};
    /// # let active: TypedArticle<Active> = unimplemented!();
    /// let _oops = active.reactivate(); // error[E0599]: no method named `reactivate` found
    /// ```
    #[allow(dead_code)]
    fn _compile_time_guarantee_documentation() {}
}
