//! Narrow fleet HTTP parsing and cookie authentication boundaries.
//! Cookie writes require an exact Origin; agent bearer requests use a separate extractor.

use std::{panic::AssertUnwindSafe, time::Duration};

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Request},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::FutureExt as _;
use gridops_core::fleet::{authorization::Principal, protocol::browser::ErrorCode};
use serde::de::DeserializeOwned;

use super::error::{FleetApiError, RequestContext};
use crate::{auth::AuthUser, error::ApiError, state::AppState};

pub(super) fn context(parts: &Parts) -> RequestContext {
    parts
        .extensions
        .get::<RequestContext>()
        .copied()
        .unwrap_or_else(RequestContext::new)
}

/// Replaces untrusted request IDs and normalizes router-level failures.
pub(super) async fn envelope(request: Request, next: Next) -> Response {
    // Finish before the legacy 30-second outer timeout so fleet failures retain
    // their typed envelope. The 25-second agent poll fits inside this deadline.
    envelope_with_deadline(request, next, Duration::from_secs(29)).await
}

async fn envelope_with_deadline(mut request: Request, next: Next, deadline: Duration) -> Response {
    let context = RequestContext::new();
    request.extensions_mut().insert(context);
    let mut response =
        match tokio::time::timeout(deadline, AssertUnwindSafe(next.run(request)).catch_unwind())
            .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                // Never format a panic payload: it may contain request data.
                tracing::error!(request_id = %context.id(), "fleet handler failed unexpectedly");
                context
                    .error(ErrorCode::DependencyUnavailable)
                    .into_response()
            }
            Err(_) => {
                tracing::warn!(request_id = %context.id(), "fleet request deadline exceeded");
                context
                    .error(ErrorCode::DependencyUnavailable)
                    .into_response()
            }
        };
    // Handler errors carry the typed body. Method/fallback rejection does not.
    if response.status() == StatusCode::METHOD_NOT_ALLOWED {
        response = context.error(ErrorCode::InvalidRequest).into_response();
    }
    if let Ok(header) = HeaderValue::from_str(&context.id().to_string()) {
        response.headers_mut().insert("x-request-id", header);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

impl FromRequestParts<AppState> for RequestContext {
    type Rejection = FleetApiError;

    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        Ok(context(parts))
    }
}

pub(super) struct FleetUser {
    principal: Principal,
}

impl FleetUser {
    pub(super) fn principal(&self) -> &Principal {
        &self.principal
    }
}

impl FromRequestParts<AppState> for FleetUser {
    type Rejection = FleetApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let context = context(parts);
        if parts.headers.contains_key(header::AUTHORIZATION) {
            return Err(context.error(ErrorCode::Unauthenticated));
        }
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(|error| {
                context.error(match error {
                    ApiError::Unauthorized => ErrorCode::Unauthenticated,
                    ApiError::Forbidden => ErrorCode::Forbidden,
                    _ => ErrorCode::DependencyUnavailable,
                })
            })?;
        Ok(Self {
            principal: Principal::AuthenticatedUser { user_id: user.id },
        })
    }
}

/// Proves cookie authentication plus Origin validation, not administration.
/// The core write transaction still reloads the current user's permissions.
pub(super) struct FleetWriteUser(FleetUser);

impl FleetWriteUser {
    pub(super) fn principal(&self) -> &Principal {
        self.0.principal()
    }
}

impl FromRequestParts<AppState> for FleetWriteUser {
    type Rejection = FleetApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let context = context(parts);
        let expected = state.config.base_url().origin().ascii_serialization();
        if !same_origin(&parts.headers, &expected) {
            return Err(context.error(ErrorCode::Forbidden));
        }
        Ok(Self(FleetUser::from_request_parts(parts, state).await?))
    }
}

fn same_origin(headers: &HeaderMap, expected: &str) -> bool {
    let mut values = headers.get_all(header::ORIGIN).iter();
    let origin = values.next();
    values.next().is_none() && origin.is_some_and(|value| value.as_bytes() == expected.as_bytes())
}

pub(super) struct FleetJson<T>(pub T);

impl<T: DeserializeOwned + Send> FromRequest<AppState> for FleetJson<T> {
    type Rejection = FleetApiError;

    async fn from_request(request: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let context = request
            .extensions()
            .get::<RequestContext>()
            .copied()
            .unwrap_or_else(RequestContext::new);
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|error| {
                context.error(if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    ErrorCode::PayloadTooLarge
                } else {
                    ErrorCode::InvalidRequest
                })
            })
    }
}

pub(super) struct FleetPath<T>(pub T);

impl<T: DeserializeOwned + Send> FromRequestParts<AppState> for FleetPath<T> {
    type Rejection = FleetApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let context = context(parts);
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| context.error(ErrorCode::InvalidRequest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn panic_keeps_the_fleet_error_envelope_without_exposing_its_payload()
    -> anyhow::Result<()> {
        use axum::{Router, body::Body, middleware, routing::get};
        use gridops_core::fleet::protocol::browser::FleetErrorResponse;
        use tower::ServiceExt as _;

        async fn fault() -> Response {
            std::panic::resume_unwind(Box::new("private-panic-sentinel"));
        }
        let router = Router::new()
            .route("/fault", get(fault))
            .layer(middleware::from_fn(envelope));
        let response = router
            .oneshot(Request::builder().uri("/fault").body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let id = response.headers()["x-request-id"].to_str()?.to_owned();
        let body = axum::body::to_bytes(response.into_body(), 1024).await?;
        assert!(!String::from_utf8_lossy(&body).contains("private-panic-sentinel"));
        let error: FleetErrorResponse = serde_json::from_slice(&body)?;
        assert_eq!(error.code, ErrorCode::DependencyUnavailable);
        assert_eq!(error.request_id.to_string(), id);
        Ok(())
    }

    #[tokio::test]
    async fn deadline_keeps_the_fleet_error_envelope_and_server_request_id() -> anyhow::Result<()> {
        use axum::{Router, body::Body, middleware, routing::get};
        use gridops_core::fleet::protocol::browser::FleetErrorResponse;
        use tower::ServiceExt as _;

        let router = Router::new()
            .route("/slow", get(std::future::pending::<Response>))
            .layer(middleware::from_fn(|request, next| {
                envelope_with_deadline(request, next, Duration::from_millis(1))
            }));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/slow")
                    .header("x-request-id", "untrusted-request-id")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let id = response.headers()["x-request-id"].to_str()?.to_owned();
        assert_ne!(id, "untrusted-request-id");
        let body = axum::body::to_bytes(response.into_body(), 1024).await?;
        let error: FleetErrorResponse = serde_json::from_slice(&body)?;
        assert_eq!(error.code, ErrorCode::DependencyUnavailable);
        assert_eq!(error.request_id.to_string(), id);
        Ok(())
    }

    #[test]
    fn cookie_origin_requires_one_exact_configured_origin() -> anyhow::Result<()> {
        let expected = "https://gridops.example";
        let mut headers = HeaderMap::new();
        assert!(!same_origin(&headers, expected));
        for value in [
            "null",
            "http://gridops.example",
            "https://other.example",
            "https://gridops.example/",
            "https://gridops.example https://other.example",
        ] {
            headers.insert(header::ORIGIN, value.parse()?);
            assert!(!same_origin(&headers, expected));
        }
        headers.insert(header::ORIGIN, expected.parse()?);
        assert!(same_origin(&headers, expected));
        headers.append(header::ORIGIN, expected.parse()?);
        assert!(!same_origin(&headers, expected));
        Ok(())
    }
}
