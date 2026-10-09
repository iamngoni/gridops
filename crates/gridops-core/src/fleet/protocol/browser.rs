//! Safe fleet browser projections and mutation requests.
//! These DTOs contain no raw runtime configuration or bootstrap credentials.
//! API handlers must authorize each projection before constructing it.

use super::primitives::{
    BrowserCounter, BrowserUint53, ContractError, MAX_CURSOR_BYTES, WireTimestamp,
};
use crate::fleet::{
    Architecture, BackendReadiness, EnrollmentState, ExecutionOs, Freshness, HostIntent,
    IntegrityState, PressureState, RuntimeKind,
    ids::{
        BackendId, CiTargetId, EventId, HostId, OperationId, PlacementId, ProfileId, UpstreamText,
        WorkloadId,
    },
};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpaqueCursor(String);
impl TryFrom<String> for OpaqueCursor {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > MAX_CURSOR_BYTES
            || !value
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
        {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(value))
    }
}
impl From<OpaqueCursor> for String {
    fn from(value: OpaqueCursor) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceAmountDto {
    pub cpu_millis: BrowserUint53,
    pub memory_mib: BrowserUint53,
    pub disk_bytes: BrowserUint53,
}

impl TryFrom<crate::fleet::ResourceAmount> for ResourceAmountDto {
    type Error = ContractError;
    fn try_from(value: crate::fleet::ResourceAmount) -> Result<Self, Self::Error> {
        Ok(Self {
            cpu_millis: BrowserUint53::new(value.cpu_millis())?,
            memory_mib: BrowserUint53::new(value.memory_mib())?,
            disk_bytes: BrowserUint53::new(value.disk_bytes())?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostSummary {
    pub id: HostId,
    pub name: UpstreamText,
    pub host_os: ExecutionOs,
    pub architecture: Architecture,
    pub enrollment: EnrollmentState,
    pub lifecycle: HostIntent,
    pub integrity: IntegrityState,
    pub freshness: Freshness,
    pub pressure: PressureState,
    pub revision: BrowserCounter,
    pub agent_version: Option<UpstreamText>,
    pub last_heartbeat_at: Option<WireTimestamp>,
    pub backend_count: BrowserUint53,
    pub runner_count: BrowserUint53,
    pub budget: Option<ResourceAmountDto>,
    pub allocated: Option<ResourceAmountDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendSummary {
    pub id: BackendId,
    pub host_id: HostId,
    pub runtime_kind: RuntimeKind,
    pub execution_os: ExecutionOs,
    pub architecture: Architecture,
    pub readiness: BackendReadiness,
    pub reason: Option<RejectionReason>,
    pub revision: BrowserCounter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedState {
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawOperationAccepted")]
pub struct OperationAccepted {
    operation_id: OperationId,
    status: AcceptedState,
    status_url: OperationUrl,
}

impl OperationAccepted {
    pub const fn new(operation_id: OperationId) -> Self {
        Self {
            operation_id,
            status: AcceptedState::Accepted,
            status_url: OperationUrl::new(operation_id),
        }
    }
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawOperationAccepted {
    operation_id: OperationId,
    status: AcceptedState,
    status_url: OperationUrl,
}
impl TryFrom<RawOperationAccepted> for OperationAccepted {
    type Error = ContractError;
    fn try_from(raw: RawOperationAccepted) -> Result<Self, Self::Error> {
        if raw.status_url != OperationUrl::new(raw.operation_id) {
            return Err(ContractError::InvalidText);
        }
        Ok(Self {
            operation_id: raw.operation_id,
            status: raw.status,
            status_url: raw.status_url,
        })
    }
}

/// An operation URL is generated from its identifier, never an arbitrary URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OperationUrl(OperationId);
impl OperationUrl {
    pub const fn new(id: OperationId) -> Self {
        Self(id)
    }
}
impl TryFrom<String> for OperationUrl {
    type Error = ContractError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let id = value
            .strip_prefix("/api/v1/operations/")
            .ok_or(ContractError::InvalidText)?;
        Ok(Self(id.parse().map_err(|_| ContractError::InvalidText)?))
    }
}
impl From<OperationUrl> for String {
    fn from(value: OperationUrl) -> Self {
        format!("/api/v1/operations/{}", value.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationStatus {
    pub operation_id: OperationId,
    pub status: OperationState,
    pub reason: Option<RejectionReason>,
    pub updated_at: WireTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    Forbidden,
    RevokedGrant,
    NativeTrustRequired,
    DockerSocketTrustRequired,
    IncompatibleRuntime,
    IncompatibleOs,
    IncompatibleArchitecture,
    UnsupportedMode,
    CapabilityUnverified,
    StaleHealth,
    StaleInventory,
    Offline,
    PressureUnknown,
    ResourcePressure,
    ResourceUnsupported,
    SessionRequired,
    SessionUnavailable,
    ImageUnavailable,
    PhysicalBudget,
    ChildBudget,
    PoolLimit,
    LifecycleBlocked,
    ProfileRevisionChanged,
    AuthorityUnavailable,
    NoEligibleHost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementCandidate {
    pub host_id: HostId,
    pub backend_id: BackendId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementRejection {
    pub host_id: Option<HostId>,
    pub backend_id: Option<BackendId>,
    pub reason: RejectionReason,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementExplanation {
    pub selected: Option<PlacementCandidate>,
    pub rejections: BoundedItems<PlacementRejection, 1000>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PlacementOutcomeData {
    Queued {
        operation_id: OperationId,
        workload_id: WorkloadId,
        placement_id: (),
        explanation: PlacementExplanation,
    },
    Admitted {
        operation_id: OperationId,
        workload_id: WorkloadId,
        placement_id: PlacementId,
        explanation: PlacementExplanation,
    },
}

/// Dependent fields are validated together: queued work has no selected host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PlacementOutcomeData", into = "PlacementOutcomeData")]
pub struct PlacementOutcome(PlacementOutcomeData);
impl PlacementOutcome {
    pub const fn data(&self) -> &PlacementOutcomeData {
        &self.0
    }
}
impl TryFrom<PlacementOutcomeData> for PlacementOutcome {
    type Error = ContractError;
    fn try_from(data: PlacementOutcomeData) -> Result<Self, Self::Error> {
        match &data {
            PlacementOutcomeData::Queued { explanation, .. } if explanation.selected.is_some() => {
                return Err(ContractError::InvalidText);
            }
            PlacementOutcomeData::Admitted { explanation, .. }
                if explanation.selected.is_none() =>
            {
                return Err(ContractError::InvalidText);
            }
            _ => {}
        }
        Ok(Self(data))
    }
}
impl From<PlacementOutcome> for PlacementOutcomeData {
    fn from(value: PlacementOutcome) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostAction {
    Pause,
    Resume,
    Drain,
    Maintenance,
    Retire,
    Revoke,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostActionRequest {
    pub action: HostAction,
    pub expected_revision: BrowserCounter,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitWorkloadRequest {
    pub pool_id: UpstreamText,
    pub profile_id: ProfileId,
    pub target_id: CiTargetId,
    pub expected_revision: BrowserCounter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BoundedItems<T, const MAX: usize>(Vec<T>);
impl<T, const MAX: usize> BoundedItems<T, MAX> {
    pub fn new(items: Vec<T>) -> Result<Self, ContractError> {
        if items.len() > MAX {
            return Err(ContractError::PayloadTooLarge);
        }
        Ok(Self(items))
    }
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}
impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for BoundedItems<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::<T>::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<T> {
    pub items: BoundedItems<T, 100>,
    pub next_cursor: Option<OpaqueCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FleetEventDto {
    pub id: EventId,
    pub cursor: OpaqueCursor,
    pub host_id: Option<HostId>,
    pub placement_id: Option<PlacementId>,
    pub kind: FleetEventKind,
    pub observed_at: WireTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogChunkDto {
    pub stream_id: super::super::ids::LogStreamId,
    pub placement_id: PlacementId,
    pub text: super::primitives::LogText,
    pub next_cursor: OpaqueCursor,
    pub eof: bool,
    pub dropped_bytes: Option<BrowserCounter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogAvailability {
    Pending,
    Available,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogMetadata {
    pub stream_id: super::super::ids::LogStreamId,
    pub placement_id: PlacementId,
    pub availability: LogAvailability,
    pub start_cursor: Option<OpaqueCursor>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetEventKind {
    HostEnrolled,
    HostApproved,
    HostChanged,
    BackendChanged,
    PlacementQueued,
    PlacementAdmitted,
    CommandChanged,
    LogGap,
    AdoptionChanged,
    UpgradeChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthenticated,
    Forbidden,
    NotFound,
    RevisionConflict,
    IdempotencyConflict,
    EnrollmentExpired,
    GrantExpired,
    CursorExpired,
    CursorGap,
    PayloadTooLarge,
    RateLimited,
    DependencyUnavailable,
    NotReady,
    ContractRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ErrorDetails {
    Revision { current_revision: BrowserCounter },
    InvalidField { field: UpstreamText },
    Limit { maximum: BrowserUint53 },
    Retry { retry_after_ms: BrowserUint53 },
    CursorGap,
    Readiness { reason: RejectionReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FleetErrorResponse {
    pub code: ErrorCode,
    pub message: UpstreamText,
    pub request_id: OperationId,
    pub details: Option<ErrorDetails>,
}

impl ErrorCode {
    pub const fn status(self) -> u16 {
        match self {
            Self::InvalidRequest | Self::ContractRange => 400,
            Self::Unauthenticated => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::RevisionConflict | Self::IdempotencyConflict => 409,
            Self::EnrollmentExpired
            | Self::GrantExpired
            | Self::CursorExpired
            | Self::CursorGap => 410,
            Self::PayloadTooLarge => 413,
            Self::RateLimited => 429,
            Self::DependencyUnavailable | Self::NotReady => 503,
        }
    }
}
