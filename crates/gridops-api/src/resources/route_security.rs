//! Shared request-part extractors for resource-route security boundaries.
//!
//! The extractor delegates to the existing origin policy so resource handlers retain the
//! established missing-origin, invalid-origin, and forbidden-origin semantics.

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{auth::assert_same_origin, error::ApiError, state::AppState};

/// Proves that a resource request passed `GridOps`' existing same-origin check.
pub struct SameOrigin;

impl FromRequestParts<AppState> for SameOrigin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        assert_same_origin(state, &parts.headers)?;
        Ok(Self)
    }
}
