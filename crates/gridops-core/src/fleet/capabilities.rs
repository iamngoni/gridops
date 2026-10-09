//! Observed execution capabilities and pure admission compatibility.
//! Reports never grant trust, enable a backend, or raise its budget. Physical
//! host OS is deliberately absent: eligibility follows the execution backend.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

use super::{
    Architecture, CiTarget, ExecutionOs, ResourceRequest, RuntimeKind,
    ids::{
        AuthoritySessionId, BackendId, ControlPlaneIncarnation, HostEpoch, HostId, UpstreamText,
    },
    protocol::{
        agent::RunnerMode,
        primitives::{ContractError, ImageReference, ProtocolVersion, WireTimestamp},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    GithubRepository,
    GithubOrganization,
    BitbucketWorkspace,
    BitbucketRepository,
}
impl From<&CiTarget> for TargetKind {
    fn from(target: &CiTarget) -> Self {
        match target {
            CiTarget::GithubRepository { .. } => Self::GithubRepository,
            CiTarget::GithubOrganization { .. } => Self::GithubOrganization,
            CiTarget::BitbucketWorkspace { .. } => Self::BitbucketWorkspace,
            CiTarget::BitbucketRepository { .. } => Self::BitbucketRepository,
        }
    }
}
impl TargetKind {
    pub const fn is_bitbucket(self) -> bool {
        matches!(self, Self::BitbucketWorkspace | Self::BitbucketRepository)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportedRunner {
    pub target_kind: TargetKind,
    pub mode: RunnerMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentReadiness {
    Ready,
    Preparable,
    Unavailable,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentCapability {
    pub reference: ImageReference,
    pub readiness: EnvironmentReadiness,
    pub version: Option<UpstreamText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuMemoryEnforcement {
    Reservation,
    HardLimit,
    Unsupported,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskEnforcement {
    Estimate,
    Quota,
    Unsupported,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceEnforcement {
    pub cpu: CpuMemoryEnforcement,
    pub memory: CpuMemoryEnforcement,
    pub disk: DiskEnforcement,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveCapability {
    Unsupported,
    RequiresAuthorizedHelper,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerSocketCapability {
    Unsupported,
    RequiresExplicitGrant,
}

/// Raw observed data, including explicit unknown/unsupported values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityReport {
    pub schema_version: ProtocolVersion,
    pub runtime_kind: RuntimeKind,
    pub execution_os: ExecutionOs,
    pub architecture: Architecture,
    pub supported_runners: Vec<SupportedRunner>,
    pub environments: Vec<EnvironmentCapability>,
    pub resources: ResourceEnforcement,
    pub interactive: InteractiveCapability,
    pub docker_socket: DockerSocketCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CapabilityReport", into = "CapabilityReport")]
pub struct BackendCapabilities(CapabilityReport);
impl TryFrom<CapabilityReport> for BackendCapabilities {
    type Error = ContractError;
    fn try_from(report: CapabilityReport) -> Result<Self, Self::Error> {
        if report.supported_runners.len() > 32 || report.environments.len() > 128 {
            return Err(ContractError::PayloadTooLarge);
        }
        if report
            .supported_runners
            .iter()
            .collect::<HashSet<_>>()
            .len()
            != report.supported_runners.len()
            || report
                .environments
                .iter()
                .map(|environment| environment.reference.as_str())
                .collect::<HashSet<_>>()
                .len()
                != report.environments.len()
        {
            return Err(ContractError::InvalidText);
        }
        Ok(Self(report))
    }
}
impl From<BackendCapabilities> for CapabilityReport {
    fn from(value: BackendCapabilities) -> Self {
        value.0
    }
}

/// Loaded from relational policy and a typed CI target. Never deserialize this
/// from the host report or permit configuration JSON to supply these flags.
#[derive(Debug, Clone)]
pub struct ExecutionRequirement {
    pub runtime_kind: RuntimeKind,
    pub execution_os: ExecutionOs,
    pub architecture: Architecture,
    pub runner: SupportedRunner,
    pub environment: ImageReference,
    pub resources: ResourceRequest,
    pub requires_interactive: bool,
    pub requires_docker_socket: bool,
}

/// Server-owned identity of the backend record being considered.
#[derive(Debug, Clone)]
pub struct BackendIdentity {
    pub host_id: HostId,
    pub backend_id: BackendId,
    pub runtime_kind: RuntimeKind,
    pub execution_os: ExecutionOs,
    pub architecture: Architecture,
}
/// Server-recorded receipt provenance, never taken from agent JSON claims.
#[derive(Debug, Clone)]
pub struct CurrentProbe {
    pub host_id: HostId,
    pub backend_id: BackendId,
    pub received_at: WireTimestamp,
    pub host_epoch: HostEpoch,
    pub authority_session_id: AuthoritySessionId,
    pub boot_id: UpstreamText,
    pub control_plane_incarnation: ControlPlaneIncarnation,
}
#[derive(Debug, Clone)]
pub struct CurrentAuthority {
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub authority_session_id: AuthoritySessionId,
    pub boot_id: UpstreamText,
    pub control_plane_incarnation: ControlPlaneIncarnation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CapabilityRejection {
    #[error("capability report does not match backend identity")]
    IdentityMismatch,
    #[error("backend probe belongs to another authority session")]
    AuthorityMismatch,
    #[error("backend probe is stale, future or missing")]
    StaleProbe,
    #[error("runtime is incompatible")]
    IncompatibleRuntime,
    #[error("execution OS is incompatible")]
    IncompatibleOs,
    #[error("execution architecture is incompatible")]
    IncompatibleArchitecture,
    #[error("target or runner mode is unsupported")]
    UnsupportedMode,
    #[error("image or release is unavailable")]
    ImageUnavailable,
    #[error("requested resource accounting is unsupported")]
    ResourceUnsupported,
    #[error("interactive helper capability is unsupported")]
    InteractiveUnsupported,
    #[error("Docker socket capability is unsupported")]
    DockerSocketUnsupported,
}
impl CapabilityRejection {
    pub const fn reason_code(self) -> &'static str {
        match self {
            Self::IdentityMismatch => "capability_unverified",
            Self::AuthorityMismatch => "stale_inventory",
            Self::StaleProbe => "stale_health",
            Self::IncompatibleRuntime => "incompatible_runtime",
            Self::IncompatibleOs => "incompatible_os",
            Self::IncompatibleArchitecture => "incompatible_architecture",
            Self::UnsupportedMode => "unsupported_mode",
            Self::ImageUnavailable => "image_unavailable",
            Self::ResourceUnsupported => "resource_unsupported",
            Self::InteractiveUnsupported => "session_unavailable",
            Self::DockerSocketUnsupported => "capability_unverified",
        }
    }
}

/// Compatibility evidence only. Admission still requires current grants,
/// selected interactive helper, lifecycle, slots and physical/child budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibleBackend {
    environment_readiness: EnvironmentReadiness,
}
impl CompatibleBackend {
    pub const fn environment_readiness(self) -> EnvironmentReadiness {
        self.environment_readiness
    }
}

impl BackendCapabilities {
    /// Bound persisted/untrusted reports before decoding their collections.
    pub fn parse(bytes: &[u8]) -> Result<Self, ContractError> {
        super::protocol::primitives::parse_json(bytes, super::protocol::primitives::MAX_BATCH_BYTES)
    }
    pub const fn report(&self) -> &CapabilityReport {
        &self.0
    }

    pub fn evaluate(
        &self,
        identity: &BackendIdentity,
        requirement: &ExecutionRequirement,
        probe: Option<&CurrentProbe>,
        authority: &CurrentAuthority,
        now: WireTimestamp,
    ) -> Result<CompatibleBackend, CapabilityRejection> {
        let report = &self.0;
        if (
            report.runtime_kind,
            report.execution_os,
            report.architecture,
        ) != (
            identity.runtime_kind,
            identity.execution_os,
            identity.architecture,
        ) {
            return Err(CapabilityRejection::IdentityMismatch);
        }
        let probe = probe.ok_or(CapabilityRejection::StaleProbe)?;
        if probe.host_id != identity.host_id
            || probe.backend_id != identity.backend_id
            || authority.host_id != identity.host_id
            || probe.host_epoch != authority.host_epoch
            || probe.authority_session_id != authority.authority_session_id
            || probe.boot_id != authority.boot_id
            || probe.control_plane_incarnation != authority.control_plane_incarnation
        {
            return Err(CapabilityRejection::AuthorityMismatch);
        }
        let age = now
            .millis()
            .checked_sub(probe.received_at.millis())
            .ok_or(CapabilityRejection::StaleProbe)?;
        if !(0..45_000).contains(&age) {
            return Err(CapabilityRejection::StaleProbe);
        }
        if report.runtime_kind != requirement.runtime_kind {
            return Err(CapabilityRejection::IncompatibleRuntime);
        }
        if report.execution_os != requirement.execution_os {
            return Err(CapabilityRejection::IncompatibleOs);
        }
        if report.architecture != requirement.architecture {
            return Err(CapabilityRejection::IncompatibleArchitecture);
        }
        static_compatibility(requirement)?;
        if !report.supported_runners.contains(&requirement.runner) {
            return Err(CapabilityRejection::UnsupportedMode);
        }
        let environment = report
            .environments
            .iter()
            .find(|environment| {
                environment.reference == requirement.environment
                    && matches!(
                        environment.readiness,
                        EnvironmentReadiness::Ready | EnvironmentReadiness::Preparable
                    )
            })
            .ok_or(CapabilityRejection::ImageUnavailable)?;
        if (requirement.resources.cpu_millis() > 0
            && report.resources.cpu == CpuMemoryEnforcement::Unsupported)
            || (requirement.resources.memory_mib() > 0
                && report.resources.memory == CpuMemoryEnforcement::Unsupported)
            || (requirement.resources.disk_bytes() > 0
                && report.resources.disk == DiskEnforcement::Unsupported)
        {
            return Err(CapabilityRejection::ResourceUnsupported);
        }
        if requirement.requires_interactive
            && report.interactive != InteractiveCapability::RequiresAuthorizedHelper
        {
            return Err(CapabilityRejection::InteractiveUnsupported);
        }
        if requirement.requires_docker_socket
            && report.docker_socket != DockerSocketCapability::RequiresExplicitGrant
        {
            return Err(CapabilityRejection::DockerSocketUnsupported);
        }
        Ok(CompatibleBackend {
            environment_readiness: environment.readiness,
        })
    }
}

/// Upstream/runtime ceilings apply even to an agent claiming broader support.
pub fn static_compatibility(requirement: &ExecutionRequirement) -> Result<(), CapabilityRejection> {
    if requirement.runtime_kind == RuntimeKind::Docker
        && requirement.execution_os != ExecutionOs::Linux
    {
        return Err(CapabilityRejection::IncompatibleOs);
    }
    if requirement.runtime_kind == RuntimeKind::TartVm {
        if requirement.execution_os != ExecutionOs::Macos {
            return Err(CapabilityRejection::IncompatibleOs);
        }
        if requirement.architecture != Architecture::Arm64 {
            return Err(CapabilityRejection::IncompatibleArchitecture);
        }
        if !requirement.runner.target_kind.is_bitbucket()
            && requirement.runner.mode != RunnerMode::Ephemeral
        {
            return Err(CapabilityRejection::UnsupportedMode);
        }
    }
    if requirement.runner.target_kind.is_bitbucket() {
        if requirement.runner.mode != RunnerMode::Persistent {
            return Err(CapabilityRejection::UnsupportedMode);
        }
        if requirement.execution_os == ExecutionOs::Windows
            && (requirement.architecture != Architecture::X64
                || requirement.runtime_kind != RuntimeKind::NativeProcess)
        {
            return Err(CapabilityRejection::IncompatibleArchitecture);
        }
    }
    Ok(())
}
