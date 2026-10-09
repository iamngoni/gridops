//! Enrollment HTTP routes expose each plaintext code or credential once.
//! Retries and status reads contain metadata only; writes delegate atomic policy checks to core.

use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use gridops_core::fleet::{
    Architecture, CiTargetId, ExecutionOs, Generation, HostId, NonNilUuid, Revision,
    protocol::{
        browser::{BoundedItems, ErrorCode},
        primitives::{BrowserCounter, IdempotencyKey, Sha256Digest, WireTimestamp},
    },
    registry::{
        ApproveHost, EnrollmentIssue, EnrollmentMetadata, EnrollmentScope, HostRegistration,
        IssueEnrollment, OneTimeSecret, RecoverHost, RecoveryIssue, RegistryService,
    },
};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use super::{
    boundary::{FleetJson, FleetPath, FleetUser, FleetWriteUser, context},
    error::{FleetApiError, RequestContext},
};
use crate::state::AppState;

/// Checked HTTP header; it cannot include credentials or arbitrary control bytes.
pub(super) struct RequestKey(IdempotencyKey);

impl RequestKey {
    pub(super) fn into_string(self) -> String {
        self.0.as_str().to_owned()
    }
}

impl FromRequestParts<AppState> for RequestKey {
    type Rejection = FleetApiError;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        let context = context(parts);
        let mut headers = parts.headers.get_all("idempotency-key").iter();
        let value = headers
            .next()
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| context.error(ErrorCode::InvalidRequest))?;
        if headers.next().is_some() {
            return Err(context.error(ErrorCode::InvalidRequest));
        }
        IdempotencyKey::parse(value)
            .map(Self)
            .map_err(|_| context.error(ErrorCode::InvalidRequest))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct IssueBody {
    target_ids: BoundedItems<CiTargetId, 64>,
    intended_host_id: Option<HostId>,
    lifetime_minutes: Option<BrowserCounter>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EnrollBody {
    code: SecretInput,
    name: String,
    host_os: ExecutionOs,
    architecture: Architecture,
    hardware_fingerprint: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecoverBody {
    expected_revision: BrowserCounter,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ApproveBody {
    expected_revision: BrowserCounter,
    expected_inventory_revision: BrowserCounter,
    expected_inventory_digest: Sha256Digest,
    target_ids: BoundedItems<CiTargetId, 64>,
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum RecoveryResponse {
    Created {
        enrollment_id: Uuid,
        host_id: HostId,
        code: ReceiptSecret,
        expires_at: WireTimestamp,
    },
    Replay {
        enrollment: EnrollmentStatus,
    },
}

/// Plaintext input is never Debug/Serialize and is zeroized on drop.
pub(super) struct SecretInput(SecretString);
impl SecretInput {
    pub(super) fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}
impl<'de> Deserialize<'de> for SecretInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = SecretString::from(String::deserialize(deserializer)?);
        if value.expose_secret().len() != 43
            || !value
                .expose_secret()
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch))
        {
            return Err(serde::de::Error::custom("invalid credential encoding"));
        }
        Ok(Self(value))
    }
}

/// This wrapper is confined to the noncacheable one-time receipt response.
struct ReceiptSecret(OneTimeSecret);
impl Serialize for ReceiptSecret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.expose())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollmentStatus {
    enrollment_id: Uuid,
    expires_at: WireTimestamp,
    consumed_at: Option<WireTimestamp>,
    revoked_at: Option<WireTimestamp>,
    consumed_host_id: Option<HostId>,
}

impl EnrollmentStatus {
    fn from_metadata(
        value: &EnrollmentMetadata,
        context: RequestContext,
    ) -> Result<Self, FleetApiError> {
        Ok(Self {
            enrollment_id: value.id,
            expires_at: WireTimestamp::from_millis(value.expires_at)
                .map_err(|_| context.error(ErrorCode::ContractRange))?,
            consumed_at: value
                .consumed_at
                .map(WireTimestamp::from_millis)
                .transpose()
                .map_err(|_| context.error(ErrorCode::ContractRange))?,
            revoked_at: value
                .revoked_at
                .map(WireTimestamp::from_millis)
                .transpose()
                .map_err(|_| context.error(ErrorCode::ContractRange))?,
            consumed_host_id: value.consumed_host_id,
        })
    }
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum IssueResponse {
    Created {
        enrollment_id: Uuid,
        code: ReceiptSecret,
        expires_at: WireTimestamp,
    },
    Replay {
        enrollment: EnrollmentStatus,
    },
}

#[derive(Serialize)]
struct EnrollmentResponse {
    host_id: HostId,
    credential_id: Uuid,
    credential_generation: Generation,
    credential: ReceiptSecret,
}

pub(super) async fn issue(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetWriteUser,
    key: RequestKey,
    FleetJson(body): FleetJson<IssueBody>,
) -> Result<Response, FleetApiError> {
    let scope = EnrollmentScope::new(body.target_ids.as_slice().iter().copied())
        .map_err(|_| context.error(ErrorCode::InvalidRequest))?;
    let ttl_millis = body
        .lifetime_minutes
        .map(|minutes| {
            let value = minutes.get();
            if value > 60 {
                return Err(context.error(ErrorCode::InvalidRequest));
            }
            i64::try_from(value * 60_000).map_err(|_| context.error(ErrorCode::InvalidRequest))
        })
        .transpose()?;
    let result = RegistryService::new(state.database, state.vault)
        .issue_enrollment(
            user.principal(),
            IssueEnrollment {
                idempotency_key: key.into_string(),
                scope,
                intended_host_id: body.intended_host_id,
                ttl_millis,
            },
        )
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(match result {
        EnrollmentIssue::Created(value) => (
            StatusCode::CREATED,
            Json(IssueResponse::Created {
                enrollment_id: value.id,
                code: ReceiptSecret(value.code),
                expires_at: WireTimestamp::from_millis(value.expires_at)
                    .map_err(|_| context.error(ErrorCode::ContractRange))?,
            }),
        )
            .into_response(),
        EnrollmentIssue::Replay(value) => Json(IssueResponse::Replay {
            enrollment: EnrollmentStatus::from_metadata(&value, context)?,
        })
        .into_response(),
    })
}

pub(super) async fn status(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetUser,
    FleetPath(id): FleetPath<NonNilUuid>,
) -> Result<Response, FleetApiError> {
    let value = RegistryService::new(state.database, state.vault)
        .enrollment_status(user.principal(), id.as_uuid())
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(Json(EnrollmentStatus::from_metadata(&value, context)?).into_response())
}

pub(super) async fn revoke(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetWriteUser,
    FleetPath(id): FleetPath<NonNilUuid>,
) -> Result<Response, FleetApiError> {
    RegistryService::new(state.database, state.vault)
        .revoke_enrollment(user.principal(), id.as_uuid())
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(StatusCode::OK.into_response())
}

pub(super) async fn consume(
    State(state): State<AppState>,
    context: RequestContext,
    FleetJson(body): FleetJson<EnrollBody>,
) -> Result<Response, FleetApiError> {
    let registration = HostRegistration::new(
        body.name,
        body.host_os,
        body.architecture,
        body.hardware_fingerprint,
    )
    .map_err(|_| context.error(ErrorCode::InvalidRequest))?;
    let value = RegistryService::new(state.database, state.vault)
        .consume_enrollment(body.code.expose(), registration)
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok((
        StatusCode::CREATED,
        Json(EnrollmentResponse {
            host_id: value.host_id,
            credential_id: value.credential_id,
            credential_generation: Generation::new(value.credential_generation)
                .map_err(|_| context.error(ErrorCode::ContractRange))?,
            credential: ReceiptSecret(value.credential),
        }),
    )
        .into_response())
}

pub(super) async fn recover(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetWriteUser,
    key: RequestKey,
    FleetPath(host_id): FleetPath<HostId>,
    FleetJson(body): FleetJson<RecoverBody>,
) -> Result<Response, FleetApiError> {
    let result = RegistryService::new(state.database, state.vault)
        .recover_host(
            user.principal(),
            RecoverHost {
                host_id,
                expected_revision: Revision::new(body.expected_revision.get())
                    .map_err(|_| context.error(ErrorCode::InvalidRequest))?,
                idempotency_key: key.into_string(),
            },
        )
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(match result {
        RecoveryIssue::Created(value) => (
            StatusCode::CREATED,
            Json(RecoveryResponse::Created {
                enrollment_id: value.id,
                host_id: value.host_id,
                code: ReceiptSecret(value.code),
                expires_at: WireTimestamp::from_millis(value.expires_at)
                    .map_err(|_| context.error(ErrorCode::ContractRange))?,
            }),
        )
            .into_response(),
        RecoveryIssue::Replay(value) => Json(RecoveryResponse::Replay {
            enrollment: EnrollmentStatus::from_metadata(&value, context)?,
        })
        .into_response(),
    })
}

/// Approval records enrollment scope while leaving execution policy and authority separate.
pub(super) async fn approve(
    State(state): State<AppState>,
    context: RequestContext,
    user: FleetWriteUser,
    FleetPath(host_id): FleetPath<HostId>,
    FleetJson(body): FleetJson<ApproveBody>,
) -> Result<Response, FleetApiError> {
    let request = ApproveHost {
        host_id,
        expected_revision: Revision::new(body.expected_revision.get())
            .map_err(|_| context.error(ErrorCode::InvalidRequest))?,
        expected_inventory_revision: body.expected_inventory_revision.get(),
        expected_inventory_digest: body.expected_inventory_digest.into(),
        scope: EnrollmentScope::new(body.target_ids.as_slice().iter().copied())
            .map_err(|_| context.error(ErrorCode::InvalidRequest))?,
    };
    RegistryService::new(state.database, state.vault)
        .approve_host(user.principal(), request)
        .await
        .map_err(|error| context.registry_error(&error))?;
    Ok(StatusCode::OK.into_response())
}
