//! Credential rotation transports metadata and agent-generated replacement secrets.
//! Browser operators receive only an operation ID; old credentials have a fixed overlap.

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use gridops_core::fleet::{
    Generation, HostId, NonNilUuid, OperationId,
    protocol::{
        browser::{
            ErrorCode, OperationAccepted, OperationState, OperationStatus, OperationUrl,
            RejectionReason,
        },
        primitives::{BrowserCounter, WireTimestamp},
    },
    registry::{RegistryService, RequestRotation, RotationOperationState},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    auth::FleetAgent,
    boundary::{FleetJson, FleetPath, FleetUser, FleetWriteUser},
    enrollment::{RequestKey, SecretInput},
    error::{FleetApiError, RequestContext},
};
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RotateBody {
    expected_generation: BrowserCounter,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExchangeBody {
    operation_id: NonNilUuid,
    next_secret: SecretInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AckBody {
    operation_id: NonNilUuid,
}

#[derive(Serialize)]
struct ExchangeReceipt {
    operation_id: Uuid,
    credential_id: Uuid,
    generation: Generation,
    overlap_expires_at: WireTimestamp,
}

#[derive(Serialize)]
struct AckReceipt {
    operation_id: Uuid,
    host_id: HostId,
    generation: Generation,
    acknowledged_at: WireTimestamp,
}

pub(super) async fn request(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetWriteUser,
    key: RequestKey,
    FleetPath(host_id): FleetPath<HostId>,
    FleetJson(body): FleetJson<RotateBody>,
) -> Result<Response, FleetApiError> {
    let receipt = RegistryService::new(state.database, state.vault)
        .request_rotation(
            user.principal(),
            RequestRotation {
                host_id,
                expected_generation: Generation::new(body.expected_generation.get())
                    .map_err(|_| context.error(ErrorCode::InvalidRequest))?,
                idempotency_key: key.into_string(),
            },
        )
        .await
        .map_err(|error| context.registry_error(&error))?;
    let id = OperationId::try_new(receipt.operation_id)
        .map_err(|_| context.error(ErrorCode::ContractRange))?;
    let location: String = OperationUrl::new(id).into();
    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, location)],
        Json(OperationAccepted::new(id)),
    )
        .into_response())
}

pub(super) async fn status(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetUser,
    FleetPath(id): FleetPath<OperationId>,
) -> Result<Response, FleetApiError> {
    let receipt = RegistryService::new(state.database, state.vault)
        .rotation_status(user.principal(), id.as_uuid())
        .await
        .map_err(|error| context.registry_error(&error))?;
    let (status, reason) = match receipt.state {
        RotationOperationState::Requested => (OperationState::Queued, None),
        RotationOperationState::Exchanged => (OperationState::Running, None),
        RotationOperationState::Acknowledged => (OperationState::Succeeded, None),
        RotationOperationState::Revoked => (OperationState::Cancelled, None),
        RotationOperationState::Expired => (
            OperationState::Blocked,
            Some(RejectionReason::AuthorityUnavailable),
        ),
    };
    Ok(Json(OperationStatus {
        operation_id: id,
        status,
        reason,
        updated_at: WireTimestamp::from_millis(receipt.updated_at)
            .map_err(|_| context.error(ErrorCode::ContractRange))?,
    })
    .into_response())
}

pub(super) async fn exchange(
    State(state): State<AppState>,
    context: RequestContext,
    agent: FleetAgent,
    FleetJson(body): FleetJson<ExchangeBody>,
) -> Result<Response, FleetApiError> {
    let receipt = RegistryService::new(state.database, state.vault)
        .exchange_rotation(
            agent.authenticated(),
            body.operation_id.as_uuid(),
            body.next_secret.expose(),
        )
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(Json(ExchangeReceipt {
        operation_id: receipt.operation_id,
        credential_id: receipt.credential_id,
        generation: receipt.generation,
        overlap_expires_at: WireTimestamp::from_millis(receipt.overlap_expires_at)
            .map_err(|_| context.error(ErrorCode::ContractRange))?,
    })
    .into_response())
}

pub(super) async fn acknowledge(
    State(state): State<AppState>,
    context: RequestContext,
    agent: FleetAgent,
    FleetJson(body): FleetJson<AckBody>,
) -> Result<Response, FleetApiError> {
    let receipt = RegistryService::new(state.database, state.vault)
        .acknowledge_rotation(agent.authenticated(), body.operation_id.as_uuid())
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(Json(AckReceipt {
        operation_id: receipt.operation_id,
        host_id: receipt.host_id,
        generation: receipt.generation,
        acknowledged_at: WireTimestamp::from_millis(receipt.acknowledged_at)
            .map_err(|_| context.error(ErrorCode::ContractRange))?,
    })
    .into_response())
}
