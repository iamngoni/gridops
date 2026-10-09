//! Strict host observation contracts for handshake, inventory, and heartbeat.
//!
//! These values describe what an agent observed.  They contain no policy
//! grants, command authority, or physical-host filtering.  Constructors and
//! deserialization share the same count and identity checks so internal calls
//! cannot bypass the wire boundary.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::primitives::{
    BrowserCounter, BrowserUint53, ProtocolVersion, Sha256Digest, WireTimestamp,
};
use crate::fleet::{
    Architecture, BackendReadiness, ExecutionOs, RuntimeKind,
    capabilities::BackendCapabilities,
    ids::{
        AuthoritySessionId, BackendId, ControlPlaneIncarnation, HostEpoch, OperationId,
        ResourceDomainId, SessionId, UpstreamText,
    },
};

pub const MAX_INVENTORY_ITEMS: usize = 64;
pub const MAX_INVENTORY_SAMPLES: usize = 64;
pub const MAX_PROTOCOL_VERSION: u16 = u16::MAX;

/// The range advertised by an agent before negotiation with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawProtocolRange", into = "RawProtocolRange")]
pub struct ProtocolRange {
    min: u16,
    max: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProtocolRange {
    min: u16,
    max: u16,
}

impl TryFrom<RawProtocolRange> for ProtocolRange {
    type Error = InventoryPayloadError;
    fn try_from(raw: RawProtocolRange) -> Result<Self, Self::Error> {
        Self::new(raw.min, raw.max)
    }
}

impl From<ProtocolRange> for RawProtocolRange {
    fn from(value: ProtocolRange) -> Self {
        Self {
            min: value.min,
            max: value.max,
        }
    }
}

impl ProtocolRange {
    pub fn new(min: u16, max: u16) -> Result<Self, InventoryPayloadError> {
        if min == 0 || min > max {
            return Err(InventoryPayloadError::InvalidProtocolRange);
        }
        Ok(Self { min, max })
    }

    pub const fn includes(self, version: u16) -> bool {
        version >= self.min && version <= self.max
    }

    pub const fn min(self) -> u16 {
        self.min
    }
    pub const fn max(self) -> u16 {
        self.max
    }
}

/// A request to establish or replay an observation session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeRequest {
    pub(crate) request_id: OperationId,
    pub(crate) agent_session_id: AuthoritySessionId,
    pub(crate) boot_id: UpstreamText,
    pub(crate) supported_protocol: ProtocolRange,
    pub(crate) agent_version: UpstreamText,
    pub(crate) expected_host_epoch: Option<HostEpoch>,
}

impl HandshakeRequest {
    pub fn new(
        request_id: OperationId,
        agent_session_id: AuthoritySessionId,
        boot_id: UpstreamText,
        supported_protocol: ProtocolRange,
        agent_version: UpstreamText,
        expected_host_epoch: Option<HostEpoch>,
    ) -> Self {
        Self {
            request_id,
            agent_session_id,
            boot_id,
            supported_protocol,
            agent_version,
            expected_host_epoch,
        }
    }
}

/// Negotiation has a stable incompatible outcome instead of a malformed
/// protocol response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProtocolNegotiation {
    Supported { version: ProtocolVersion },
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSessionState {
    Observing,
    Reconciling,
    Ready,
    Conflicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationLease {
    pub expires_at: WireTimestamp,
    pub heartbeat_ms: BrowserUint53,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeResponse {
    pub request_id: OperationId,
    pub host_id: crate::fleet::ids::HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub agent_session_id: AuthoritySessionId,
    pub boot_id: UpstreamText,
    pub negotiated_protocol: ProtocolNegotiation,
    pub state: ObservationSessionState,
    pub lease: Option<ObservationLease>,
    pub received_at: WireTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainKind {
    Physical,
    VirtualMachine,
    Container,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedCapacity {
    pub cpu_millis: Option<BrowserUint53>,
    pub memory_mib: Option<BrowserUint53>,
    pub disk_bytes: Option<BrowserUint53>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedDomain {
    pub local_key: UpstreamText,
    pub known_id: Option<ResourceDomainId>,
    pub parent_local_key: Option<UpstreamText>,
    pub kind: DomainKind,
    pub name: UpstreamText,
    pub runtime_identity: Option<UpstreamText>,
    pub capacity: ObservedCapacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedBackend {
    pub local_key: UpstreamText,
    pub known_id: Option<BackendId>,
    pub domain_local_key: UpstreamText,
    pub runtime_kind: RuntimeKind,
    pub execution_os: ExecutionOs,
    pub architecture: Architecture,
    pub capabilities: BackendCapabilities,
    pub readiness: BackendReadiness,
    pub reason: Option<UpstreamText>,
    pub runtime_identity: Option<UpstreamText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperState {
    Ready,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedInteractiveSession {
    pub local_key: UpstreamText,
    pub known_id: Option<SessionId>,
    pub os_user_id: UpstreamText,
    pub helper_state: HelperState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleCoverage {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedSample {
    pub domain_local_key: UpstreamText,
    pub observed_at: WireTimestamp,
    pub coverage: SampleCoverage,
    pub cpu_used_millis: Option<BrowserUint53>,
    pub memory_used_mib: Option<BrowserUint53>,
    pub disk_free_bytes: Option<BrowserUint53>,
    pub uptime_seconds: Option<BrowserUint53>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawInventoryReport", into = "RawInventoryReport")]
pub struct InventoryReport {
    pub(crate) host_epoch: HostEpoch,
    pub(crate) control_plane_incarnation: ControlPlaneIncarnation,
    pub(crate) authority_session_id: AuthoritySessionId,
    pub(crate) boot_id: UpstreamText,
    pub(crate) inventory_sequence: BrowserCounter,
    pub(crate) observed_at: WireTimestamp,
    pub(crate) domains: Vec<ObservedDomain>,
    pub(crate) backends: Vec<ObservedBackend>,
    pub(crate) interactive_sessions: Vec<ObservedInteractiveSession>,
    pub(crate) samples: Vec<ObservedSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInventoryReport {
    host_epoch: HostEpoch,
    control_plane_incarnation: ControlPlaneIncarnation,
    authority_session_id: AuthoritySessionId,
    boot_id: UpstreamText,
    inventory_sequence: BrowserCounter,
    observed_at: WireTimestamp,
    domains: Vec<ObservedDomain>,
    backends: Vec<ObservedBackend>,
    interactive_sessions: Vec<ObservedInteractiveSession>,
    samples: Vec<ObservedSample>,
}

impl TryFrom<RawInventoryReport> for InventoryReport {
    type Error = InventoryPayloadError;
    fn try_from(raw: RawInventoryReport) -> Result<Self, Self::Error> {
        let report = Self {
            host_epoch: raw.host_epoch,
            control_plane_incarnation: raw.control_plane_incarnation,
            authority_session_id: raw.authority_session_id,
            boot_id: raw.boot_id,
            inventory_sequence: raw.inventory_sequence,
            observed_at: raw.observed_at,
            domains: raw.domains,
            backends: raw.backends,
            interactive_sessions: raw.interactive_sessions,
            samples: raw.samples,
        };
        report.validate_shape()?;
        Ok(report)
    }
}

impl From<InventoryReport> for RawInventoryReport {
    fn from(value: InventoryReport) -> Self {
        Self {
            host_epoch: value.host_epoch,
            control_plane_incarnation: value.control_plane_incarnation,
            authority_session_id: value.authority_session_id,
            boot_id: value.boot_id,
            inventory_sequence: value.inventory_sequence,
            observed_at: value.observed_at,
            domains: value.domains,
            backends: value.backends,
            interactive_sessions: value.interactive_sessions,
            samples: value.samples,
        }
    }
}

impl InventoryReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host_epoch: HostEpoch,
        control_plane_incarnation: ControlPlaneIncarnation,
        authority_session_id: AuthoritySessionId,
        boot_id: UpstreamText,
        inventory_sequence: BrowserCounter,
        observed_at: WireTimestamp,
        domains: Vec<ObservedDomain>,
        backends: Vec<ObservedBackend>,
        interactive_sessions: Vec<ObservedInteractiveSession>,
        samples: Vec<ObservedSample>,
    ) -> Result<Self, InventoryPayloadError> {
        let report = Self {
            host_epoch,
            control_plane_incarnation,
            authority_session_id,
            boot_id,
            inventory_sequence,
            observed_at,
            domains,
            backends,
            interactive_sessions,
            samples,
        };
        report.validate_shape()?;
        Ok(report)
    }

    pub fn validate_shape(&self) -> Result<(), InventoryPayloadError> {
        check_count(self.domains.len())?;
        check_count(self.backends.len())?;
        check_count(self.interactive_sessions.len())?;
        if self.samples.len() > MAX_INVENTORY_SAMPLES {
            return Err(InventoryPayloadError::TooManySamples);
        }
        let domain_keys = self
            .domains
            .iter()
            .map(|item| item.local_key.as_str())
            .collect::<std::collections::HashSet<_>>();
        if domain_keys.len() != self.domains.len() {
            return Err(InventoryPayloadError::DuplicateLocalKey);
        }
        if self
            .backends
            .iter()
            .map(|item| item.local_key.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len()
            != self.backends.len()
            || self
                .interactive_sessions
                .iter()
                .map(|item| item.local_key.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.interactive_sessions.len()
        {
            return Err(InventoryPayloadError::DuplicateLocalKey);
        }
        for domain in &self.domains {
            if let Some(parent) = &domain.parent_local_key {
                if parent.as_str() == domain.local_key.as_str()
                    || !domain_keys.contains(parent.as_str())
                {
                    return Err(InventoryPayloadError::InvalidDomainGraph);
                }
            }
        }
        for backend in &self.backends {
            if !domain_keys.contains(backend.domain_local_key.as_str()) {
                return Err(InventoryPayloadError::InvalidDomainGraph);
            }
            let report = backend.capabilities.report();
            if (
                report.runtime_kind,
                report.execution_os,
                report.architecture,
            ) != (
                backend.runtime_kind,
                backend.execution_os,
                backend.architecture,
            ) {
                return Err(InventoryPayloadError::BackendIdentityMismatch);
            }
        }
        for sample in &self.samples {
            if !domain_keys.contains(sample.domain_local_key.as_str()) {
                return Err(InventoryPayloadError::InvalidDomainGraph);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryRequest {
    pub(crate) request_id: OperationId,
    pub(crate) report: InventoryReport,
}

impl InventoryRequest {
    pub fn new(
        request_id: OperationId,
        report: InventoryReport,
    ) -> Result<Self, InventoryPayloadError> {
        report.validate_shape()?;
        Ok(Self { request_id, report })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawHeartbeatRequest", into = "RawHeartbeatRequest")]
pub struct HeartbeatRequest {
    pub(crate) request_id: OperationId,
    pub(crate) host_epoch: HostEpoch,
    pub(crate) control_plane_incarnation: ControlPlaneIncarnation,
    pub(crate) authority_session_id: AuthoritySessionId,
    pub(crate) boot_id: UpstreamText,
    pub(crate) heartbeat_sequence: BrowserCounter,
    pub(crate) observed_at: WireTimestamp,
    pub(crate) samples: Vec<ObservedSample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHeartbeatRequest {
    request_id: OperationId,
    host_epoch: HostEpoch,
    control_plane_incarnation: ControlPlaneIncarnation,
    authority_session_id: AuthoritySessionId,
    boot_id: UpstreamText,
    heartbeat_sequence: BrowserCounter,
    observed_at: WireTimestamp,
    samples: Vec<ObservedSample>,
}

impl TryFrom<RawHeartbeatRequest> for HeartbeatRequest {
    type Error = InventoryPayloadError;
    fn try_from(raw: RawHeartbeatRequest) -> Result<Self, Self::Error> {
        let request = Self {
            request_id: raw.request_id,
            host_epoch: raw.host_epoch,
            control_plane_incarnation: raw.control_plane_incarnation,
            authority_session_id: raw.authority_session_id,
            boot_id: raw.boot_id,
            heartbeat_sequence: raw.heartbeat_sequence,
            observed_at: raw.observed_at,
            samples: raw.samples,
        };
        request.validate_shape()?;
        Ok(request)
    }
}

impl From<HeartbeatRequest> for RawHeartbeatRequest {
    fn from(value: HeartbeatRequest) -> Self {
        Self {
            request_id: value.request_id,
            host_epoch: value.host_epoch,
            control_plane_incarnation: value.control_plane_incarnation,
            authority_session_id: value.authority_session_id,
            boot_id: value.boot_id,
            heartbeat_sequence: value.heartbeat_sequence,
            observed_at: value.observed_at,
            samples: value.samples,
        }
    }
}

impl HeartbeatRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_id: OperationId,
        host_epoch: HostEpoch,
        control_plane_incarnation: ControlPlaneIncarnation,
        authority_session_id: AuthoritySessionId,
        boot_id: UpstreamText,
        heartbeat_sequence: BrowserCounter,
        observed_at: WireTimestamp,
        samples: Vec<ObservedSample>,
    ) -> Result<Self, InventoryPayloadError> {
        let request = Self {
            request_id,
            host_epoch,
            control_plane_incarnation,
            authority_session_id,
            boot_id,
            heartbeat_sequence,
            observed_at,
            samples,
        };
        request.validate_shape()?;
        Ok(request)
    }

    pub fn validate_shape(&self) -> Result<(), InventoryPayloadError> {
        if self.samples.len() > MAX_INVENTORY_SAMPLES {
            return Err(InventoryPayloadError::TooManySamples);
        }
        let mut keys = std::collections::HashSet::new();
        for sample in &self.samples {
            if !keys.insert(sample.domain_local_key.as_str()) {
                return Err(InventoryPayloadError::DuplicateLocalKey);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryResponse {
    pub request_id: OperationId,
    pub host_id: crate::fleet::ids::HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub inventory_sequence: BrowserCounter,
    pub inventory_revision: BrowserCounter,
    pub digest: Sha256Digest,
    pub domain_ids: Vec<InventoryIdMapping<ResourceDomainId>>,
    pub backend_ids: Vec<InventoryIdMapping<BackendId>>,
    pub interactive_session_ids: Vec<InventoryIdMapping<SessionId>>,
    pub received_at: WireTimestamp,
    pub state: ObservationSessionState,
    pub lease: ObservationLease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryIdMapping<T> {
    pub local_key: UpstreamText,
    pub server_id: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatResponse {
    pub request_id: OperationId,
    pub host_id: crate::fleet::ids::HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub heartbeat_sequence: BrowserCounter,
    pub received_at: WireTimestamp,
    pub state: ObservationSessionState,
    pub lease: ObservationLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InventoryPayloadError {
    #[error("protocol range must be positive and ordered")]
    InvalidProtocolRange,
    #[error("inventory contains too many items")]
    TooManyItems,
    #[error("inventory contains too many samples")]
    TooManySamples,
    #[error("inventory contains duplicate local keys")]
    DuplicateLocalKey,
    #[error("inventory domain graph is invalid")]
    InvalidDomainGraph,
    #[error("backend identity does not match its capability report")]
    BackendIdentityMismatch,
}

fn check_count(count: usize) -> Result<(), InventoryPayloadError> {
    if count > MAX_INVENTORY_ITEMS {
        Err(InventoryPayloadError::TooManyItems)
    } else {
        Ok(())
    }
}
