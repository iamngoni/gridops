//! Authenticated host observations use strict shared agent contracts.
//! Successful writes return durable receipts and never grant command authority.

use axum::{Json, extract::State};
use gridops_core::fleet::{
    protocol::{
        browser::ErrorCode,
        inventory::{
            HandshakeRequest, HandshakeResponse, HeartbeatRequest, HeartbeatResponse,
            InventoryRequest, InventoryResponse,
        },
    },
    registry::{InventoryError, RegistryService},
};

use super::{
    auth::FleetAgent,
    boundary::FleetJson,
    error::{FleetApiError, RequestContext},
};
use crate::state::AppState;

pub(super) async fn handshake(
    State(state): State<AppState>,
    context: RequestContext,
    agent: FleetAgent,
    FleetJson(body): FleetJson<HandshakeRequest>,
) -> Result<Json<HandshakeResponse>, FleetApiError> {
    RegistryService::new(state.database, state.vault)
        .handshake(agent.authenticated(), body)
        .await
        .map(Json)
        .map_err(|error| inventory_error(context, &error))
}

pub(super) async fn submit(
    State(state): State<AppState>,
    context: RequestContext,
    agent: FleetAgent,
    FleetJson(body): FleetJson<InventoryRequest>,
) -> Result<Json<InventoryResponse>, FleetApiError> {
    RegistryService::new(state.database, state.vault)
        .submit_inventory(agent.authenticated(), body)
        .await
        .map(Json)
        .map_err(|error| inventory_error(context, &error))
}

pub(super) async fn heartbeat(
    State(state): State<AppState>,
    context: RequestContext,
    agent: FleetAgent,
    FleetJson(body): FleetJson<HeartbeatRequest>,
) -> Result<Json<HeartbeatResponse>, FleetApiError> {
    RegistryService::new(state.database, state.vault)
        .heartbeat(agent.authenticated(), body)
        .await
        .map(Json)
        .map_err(|error| inventory_error(context, &error))
}

fn inventory_error(context: RequestContext, error: &InventoryError) -> FleetApiError {
    context.error(match error {
        InventoryError::Registry(error) => return context.registry_error(error),
        InventoryError::DependencyUnavailable
        | InventoryError::Database(_)
        | InventoryError::Json(_) => {
            tracing::error!(request_id = %context.id(), "fleet inventory dependency failed");
            ErrorCode::DependencyUnavailable
        }
        InventoryError::Payload(_) | InventoryError::DomainDepth | InventoryError::RootConflict => {
            ErrorCode::InvalidRequest
        }
        InventoryError::HandshakeConflict
        | InventoryError::SequenceConflict
        | InventoryError::HeartbeatConflict => ErrorCode::IdempotencyConflict,
        InventoryError::IdentityConflict
        | InventoryError::CredentialConflict
        | InventoryError::SessionConflict
        | InventoryError::MappingConflict => ErrorCode::RevisionConflict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::header, response::IntoResponse};
    use gridops_core::fleet::{
        protocol::{browser::FleetErrorResponse, inventory::InventoryPayloadError},
        registry::RegistryError,
    };

    #[tokio::test]
    async fn observation_errors_use_safe_shared_codes_and_request_identity() -> anyhow::Result<()> {
        for (failure, expected) in [
            (
                InventoryError::DependencyUnavailable,
                ErrorCode::DependencyUnavailable,
            ),
            (
                InventoryError::Database(sqlx::Error::Protocol(
                    "inventory-private-sentinel".into(),
                )),
                ErrorCode::DependencyUnavailable,
            ),
            (
                InventoryError::Payload(InventoryPayloadError::InvalidProtocolRange),
                ErrorCode::InvalidRequest,
            ),
            (InventoryError::RootConflict, ErrorCode::InvalidRequest),
            (InventoryError::DomainDepth, ErrorCode::InvalidRequest),
            (
                InventoryError::HandshakeConflict,
                ErrorCode::IdempotencyConflict,
            ),
            (
                InventoryError::SequenceConflict,
                ErrorCode::IdempotencyConflict,
            ),
            (
                InventoryError::HeartbeatConflict,
                ErrorCode::IdempotencyConflict,
            ),
            (
                InventoryError::IdentityConflict,
                ErrorCode::RevisionConflict,
            ),
            (
                InventoryError::CredentialConflict,
                ErrorCode::RevisionConflict,
            ),
            (InventoryError::SessionConflict, ErrorCode::RevisionConflict),
            (InventoryError::MappingConflict, ErrorCode::RevisionConflict),
            (
                InventoryError::Registry(RegistryError::InvalidCredential),
                ErrorCode::Unauthenticated,
            ),
        ] {
            let context = RequestContext::new();
            let response = inventory_error(context, &failure).into_response();
            assert_eq!(response.status().as_u16(), expected.status());
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let bytes = axum::body::to_bytes(response.into_body(), 1024).await?;
            assert!(!String::from_utf8_lossy(&bytes).contains("inventory-private-sentinel"));
            let body: FleetErrorResponse = serde_json::from_slice(&bytes)?;
            assert_eq!(body.code, expected);
            assert_eq!(body.request_id.as_uuid(), context.id());
            assert!(body.details.is_none());
        }
        Ok(())
    }
}
