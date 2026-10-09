//! Fleet errors use a bounded public contract and server-owned request IDs.
//! Database, decoder and credential details never cross the HTTP boundary.

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use gridops_core::fleet::protocol::browser::{ErrorCode, ErrorDetails};
use gridops_core::fleet::registry::RegistryError;
use serde::Serialize;
use uuid::Uuid;

/// Created by the fleet middleware, never deserialized from request content.
#[derive(Debug, Clone, Copy)]
pub(super) struct RequestContext {
    id: Uuid,
}

impl RequestContext {
    pub(super) fn new() -> Self {
        Self { id: Uuid::new_v4() }
    }

    pub(super) const fn id(self) -> Uuid {
        self.id
    }

    pub(super) const fn error(self, code: ErrorCode) -> FleetApiError {
        FleetApiError {
            code,
            context: self,
        }
    }

    pub(super) fn registry_error(self, error: &RegistryError) -> FleetApiError {
        self.error(match error {
            RegistryError::Database(_) | RegistryError::Json(_) => {
                tracing::error!(request_id = %self.id, "fleet registry dependency failed");
                ErrorCode::DependencyUnavailable
            }
            RegistryError::Forbidden | RegistryError::TargetScopeForbidden => ErrorCode::Forbidden,
            RegistryError::NotFound => ErrorCode::NotFound,
            RegistryError::EnrollmentGone => ErrorCode::EnrollmentExpired,
            RegistryError::InvalidCredential => ErrorCode::Unauthenticated,
            RegistryError::StaleRevision | RegistryError::StaleGeneration => {
                ErrorCode::RevisionConflict
            }
            RegistryError::RotationConflict | RegistryError::IdempotencyConflict => {
                ErrorCode::IdempotencyConflict
            }
            RegistryError::RotationGone => ErrorCode::GrantExpired,
            RegistryError::EnrollmentScopeMismatch
            | RegistryError::InvalidRequest(_)
            | RegistryError::InvalidIdentifier(_)
            | RegistryError::Credential(_)
            | RegistryError::Input(_) => ErrorCode::InvalidRequest,
        })
    }
}

#[derive(Debug)]
pub(super) struct FleetApiError {
    code: ErrorCode,
    context: RequestContext,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    code: ErrorCode,
    message: &'static str,
    request_id: Uuid,
    details: Option<ErrorDetails>,
}

impl IntoResponse for FleetApiError {
    fn into_response(self) -> Response {
        let status = match self.code.status() {
            400 => StatusCode::BAD_REQUEST,
            401 => StatusCode::UNAUTHORIZED,
            403 => StatusCode::FORBIDDEN,
            404 => StatusCode::NOT_FOUND,
            409 => StatusCode::CONFLICT,
            410 => StatusCode::GONE,
            413 => StatusCode::PAYLOAD_TOO_LARGE,
            429 => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        let message = match self.code {
            ErrorCode::InvalidRequest => "Invalid fleet request",
            ErrorCode::Unauthenticated => "Authentication required",
            ErrorCode::Forbidden => "Fleet action is not authorized",
            ErrorCode::NotFound => "Fleet resource was not found",
            ErrorCode::RevisionConflict => "Fleet revision has changed",
            ErrorCode::IdempotencyConflict => "Request conflicts with an earlier operation",
            ErrorCode::EnrollmentExpired => "Enrollment code is expired or already used",
            ErrorCode::GrantExpired => "Fleet authority has expired",
            ErrorCode::CursorExpired => "Fleet cursor has expired",
            ErrorCode::CursorGap => "Fleet event history is no longer available",
            ErrorCode::PayloadTooLarge => "Fleet request exceeds the size limit",
            ErrorCode::RateLimited => "Fleet request rate limit reached",
            ErrorCode::DependencyUnavailable => "Fleet service is temporarily unavailable",
            ErrorCode::NotReady => "Fleet is not ready for this operation",
            ErrorCode::ContractRange => "Fleet value is outside the supported range",
        };
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(ErrorBody {
                code: self.code,
                message,
                request_id: self.context.id,
                details: None,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridops_core::fleet::protocol::browser::FleetErrorResponse;

    #[tokio::test]
    async fn every_http_error_matches_the_shared_contract() -> anyhow::Result<()> {
        for code in [
            ErrorCode::InvalidRequest,
            ErrorCode::Unauthenticated,
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::RevisionConflict,
            ErrorCode::IdempotencyConflict,
            ErrorCode::EnrollmentExpired,
            ErrorCode::GrantExpired,
            ErrorCode::CursorExpired,
            ErrorCode::CursorGap,
            ErrorCode::PayloadTooLarge,
            ErrorCode::RateLimited,
            ErrorCode::DependencyUnavailable,
            ErrorCode::NotReady,
            ErrorCode::ContractRange,
        ] {
            let context = RequestContext::new();
            let response = context.error(code).into_response();
            assert_eq!(response.status().as_u16(), code.status());
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let bytes = axum::body::to_bytes(response.into_body(), 1024).await?;
            let body: FleetErrorResponse = serde_json::from_slice(&bytes)?;
            assert_eq!(body.code, code);
            assert_eq!(body.request_id.as_uuid(), context.id());
            assert!(body.details.is_none());
        }
        Ok(())
    }
}
