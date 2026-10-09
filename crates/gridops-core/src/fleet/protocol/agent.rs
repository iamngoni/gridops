//! Immutable agent command envelopes and owned runtime operations.
//! A parsed command still requires authenticated session, journal and capability
//! checks before dispatch. Preparation never starts a CI listener or host shell.

use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::primitives::{
    IdempotencyKey, ImageReference, MAX_RESOURCE_DOMAIN_DEPTH, ProtocolVersion, SandboxArgument,
    WireTimestamp,
};
use crate::fleet::{
    Architecture, CiTarget, ExecutionOs, ResourceRequest, RuntimeKind,
    ids::{
        AllocationId, ArtifactId, AuthoritySessionId, BackendId, BootstrapReference, CommandId,
        ControlPlaneIncarnation, Generation, HostEpoch, HostId, OperationId, PlacementId,
        PreparedEnvironmentId, ProfileId, ReleaseManifestId, Revision, SessionId, UpstreamText,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerMode {
    Ephemeral,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparedWorkload {
    CiRunner,
    FixSandbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandScope {
    Host,
    Placement {
        placement_id: PlacementId,
        backend_id: BackendId,
        generation: Generation,
    },
}

/// Safe profile configuration. Unknown configuration cannot become arbitrary
/// runtime flags. Each new field needs an explicit backend interpretation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConfiguration {
    #[serde(default)]
    pub network: Option<UpstreamText>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub requires_interactive: bool,
    #[serde(default)]
    pub requires_docker_socket: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareEnvironment {
    pub execution_profile_id: ProfileId,
    pub profile_revision: Revision,
    pub workload: PreparedWorkload,
    pub target: CiTarget,
    pub runtime_kind: RuntimeKind,
    pub execution_os: ExecutionOs,
    pub architecture: Architecture,
    pub mode: RunnerMode,
    pub image: ImageReference,
    pub labels: Vec<UpstreamText>,
    pub configuration: ExecutionConfiguration,
    pub resource_request: ResourceRequest,
    pub allocation_ids: Vec<AllocationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalResourceIdentity {
    Docker {
        object_id: UpstreamText,
    },
    TartVm {
        name: UpstreamText,
    },
    NativeProcess {
        boot_id: UpstreamText,
        process_id: super::primitives::BoundedInteger<1, { u32::MAX as u64 }>,
        start_marker: UpstreamText,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRunner {
    pub prepared_environment_id: PreparedEnvironmentId,
    pub execution_profile_id: ProfileId,
    pub profile_revision: Revision,
    pub bootstrap_reference: BootstrapReference,
    pub bootstrap_expires_at: WireTimestamp,
    // The local journal resolves this command's prepared_environment_id to an
    // owned environment. A native listener PID only exists after Start succeeds.
    pub resource_request: ResourceRequest,
    pub allocation_ids: Vec<AllocationId>,
}

/// Only this explicit mode can request cancellation of busy work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopMode {
    IdleOnly,
    CancelBusy,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentOperation {
    PrepareEnvironment(PrepareEnvironment),
    StartRunner(StartRunner),
    InspectHost,
    DrainHost,
    Inspect {
        expected_resource: LocalResourceIdentity,
    },
    Drain {
        expected_resource: LocalResourceIdentity,
    },
    Stop {
        expected_resource: LocalResourceIdentity,
        mode: StopMode,
    },
    Cleanup {
        expected_resource: LocalResourceIdentity,
    },
    UpgradeAgent {
        signed_manifest_reference: ReleaseManifestId,
        expected_version: UpstreamText,
        current_version: UpstreamText,
        drain_operation_id: OperationId,
    },
    ExecuteSandbox {
        expected_resource: LocalResourceIdentity,
        program: super::primitives::SandboxProgram,
        argv: Vec<SandboxArgument>,
        working_directory: Option<super::primitives::SandboxPath>,
        timeout_ms: super::primitives::BoundedInteger<1, 300_000>,
        output_limit_bytes: super::primitives::BoundedInteger<1, 1_048_576>,
    },
    SandboxFile {
        expected_resource: LocalResourceIdentity,
        action: SandboxFileAction,
    },
    SandboxArtifact {
        expected_resource: LocalResourceIdentity,
        action: SandboxArtifactAction,
    },
}

impl std::fmt::Debug for AgentOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOperation")
            .field("kind", &self.database_kind())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxFileAction {
    Read {
        path: super::primitives::SandboxPath,
        offset: super::primitives::ByteOffset,
        max_bytes: super::primitives::BoundedInteger<1, 262_144>,
    },
    Write {
        path: super::primitives::SandboxPath,
        artifact_id: ArtifactId,
        expected_length: super::primitives::ByteOffset,
        sha256: super::primitives::Sha256Digest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxArtifactAction {
    Publish {
        path: super::primitives::SandboxPath,
        artifact_id: ArtifactId,
    },
    Fetch {
        artifact_id: ArtifactId,
        offset: super::primitives::ByteOffset,
        max_bytes: super::primitives::BoundedInteger<1, 262_144>,
    },
}

impl AgentOperation {
    pub const fn database_kind(&self) -> &'static str {
        match self {
            Self::PrepareEnvironment(_) => "prepare_environment",
            Self::StartRunner(_) => "start_runner",
            Self::InspectHost | Self::Inspect { .. } => "inspect",
            Self::DrainHost | Self::Drain { .. } => "drain",
            Self::Stop { .. } => "stop",
            Self::Cleanup { .. } => "cleanup",
            Self::UpgradeAgent { .. } => "upgrade_agent",
            Self::ExecuteSandbox { .. } => "execute_sandbox",
            Self::SandboxFile { .. } => "sandbox_file",
            Self::SandboxArtifact { .. } => "sandbox_artifact",
        }
    }

    fn check(&self, scope: &CommandScope, revision: Revision) -> Result<(), CommandError> {
        let host_operation = matches!(
            self,
            Self::InspectHost | Self::DrainHost | Self::UpgradeAgent { .. }
        );
        if host_operation != matches!(scope, CommandScope::Host) {
            return Err(CommandError::Scope);
        }
        match self {
            Self::PrepareEnvironment(prepare) => {
                check_allocations(&prepare.allocation_ids)?;
                if prepare.profile_revision != revision
                    || prepare.labels.len() > 100
                    || prepare.configuration.requires_interactive
                        != prepare.configuration.session_id.is_some()
                    || (prepare.workload == PreparedWorkload::FixSandbox
                        && (prepare.runtime_kind != RuntimeKind::Docker
                            || prepare.execution_os != ExecutionOs::Linux
                            || !matches!(prepare.target, CiTarget::GithubRepository { .. })))
                    || (prepare.runtime_kind == RuntimeKind::Docker
                        && prepare.execution_os != ExecutionOs::Linux)
                    || (prepare.runtime_kind == RuntimeKind::TartVm
                        && (prepare.execution_os != ExecutionOs::Macos
                            || prepare.architecture != Architecture::Arm64))
                    || (prepare.target.is_bitbucket() && prepare.mode != RunnerMode::Persistent)
                    || (prepare.target.is_github()
                        && prepare.runtime_kind == RuntimeKind::TartVm
                        && prepare.mode != RunnerMode::Ephemeral)
                    || (prepare.target.is_bitbucket()
                        && prepare.execution_os == ExecutionOs::Windows
                        && prepare.architecture == Architecture::Arm64)
                {
                    return Err(CommandError::Configuration);
                }
            }
            Self::StartRunner(start) => {
                check_allocations(&start.allocation_ids)?;
                if start.profile_revision != revision {
                    return Err(CommandError::Configuration);
                }
            }
            Self::ExecuteSandbox {
                expected_resource,
                argv,
                ..
            } => {
                if !matches!(expected_resource, LocalResourceIdentity::Docker { .. })
                    || argv.len() > 128
                    || argv
                        .iter()
                        .map(|argument| argument.as_str().len())
                        .sum::<usize>()
                        > 65_536
                {
                    return Err(CommandError::Configuration);
                }
            }
            Self::SandboxFile {
                expected_resource, ..
            }
            | Self::SandboxArtifact {
                expected_resource, ..
            } if !matches!(expected_resource, LocalResourceIdentity::Docker { .. }) => {
                return Err(CommandError::Configuration);
            }
            _ => {}
        }
        Ok(())
    }
}

fn check_allocations(ids: &[AllocationId]) -> Result<(), CommandError> {
    if ids.is_empty()
        || ids.len() > MAX_RESOURCE_DOMAIN_DEPTH
        || ids.iter().collect::<HashSet<_>>().len() != ids.len()
    {
        return Err(CommandError::Configuration);
    }
    Ok(())
}

/// Untrusted command material. Use `validate_against` before dispatch.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandEnvelope {
    pub protocol_version: ProtocolVersion,
    pub command_id: CommandId,
    pub operation_id: OperationId,
    pub idempotency_key: IdempotencyKey,
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub scope: CommandScope,
    pub expected_revision: Revision,
    pub issued_at: WireTimestamp,
    pub deadline_at: WireTimestamp,
    pub request_hash: String,
    pub operation: AgentOperation,
}

impl std::fmt::Debug for AgentCommandEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentCommandEnvelope")
            .field("command_id", &self.command_id)
            .field("host_id", &self.host_id)
            .field("host_epoch", &self.host_epoch)
            .field("scope", &self.scope)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

/// Context populated only after credential/session verification, never from
/// request JSON. The gateway must also enforce credential revocation in storage.
#[derive(Debug, Clone)]
pub struct ExpectedAgentSession {
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub credential_generation: Generation,
    pub authority_expires_at: WireTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandError {
    #[error("command belongs to a different authority session")]
    Authority,
    #[error("operation is not valid for this command scope")]
    Scope,
    #[error("command configuration is inconsistent")]
    Configuration,
    #[error("command deadline is invalid or expired")]
    Deadline,
    #[error("immutable command hash does not match")]
    Hash,
    #[error("command serialization failed")]
    Serialization,
}

/// Session, shape and hash have been checked. Dispatch still requires the
/// local journal's ownership/capability checks and monotonic authority expiry.
pub struct ValidatedCommand<'a>(&'a AgentCommandEnvelope);
impl<'a> ValidatedCommand<'a> {
    pub const fn envelope(&self) -> &'a AgentCommandEnvelope {
        self.0
    }
}

impl AgentCommandEnvelope {
    /// Hash canonical typed JSON with an empty hash field to avoid self-reference.
    /// No map or arbitrary JSON is allowed in a command, so field order is stable.
    pub fn canonical_hash(&self) -> Result<String, CommandError> {
        let mut canonical = self.clone();
        canonical.request_hash.clear();
        let bytes = serde_json::to_vec(&canonical).map_err(|_| CommandError::Serialization)?;
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)))
    }

    pub fn seal_hash(mut self) -> Result<Self, CommandError> {
        self.check_shape()?;
        self.request_hash = self.canonical_hash()?;
        Ok(self)
    }

    pub fn check_shape(&self) -> Result<(), CommandError> {
        self.operation.check(&self.scope, self.expected_revision)?;
        let duration = self
            .deadline_at
            .millis()
            .checked_sub(self.issued_at.millis())
            .ok_or(CommandError::Deadline)?;
        let cap = if matches!(self.operation, AgentOperation::StartRunner(_)) {
            60_000
        } else {
            3_600_000
        };
        if duration <= 0 || duration > cap {
            return Err(CommandError::Deadline);
        }
        if let AgentOperation::StartRunner(start) = &self.operation {
            if self.deadline_at > start.bootstrap_expires_at {
                return Err(CommandError::Deadline);
            }
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        session: &ExpectedAgentSession,
        now: WireTimestamp,
    ) -> Result<ValidatedCommand<'_>, CommandError> {
        self.check_shape()?;
        if self.host_id != session.host_id
            || self.host_epoch != session.host_epoch
            || self.control_plane_incarnation != session.control_plane_incarnation
            || self.authority_session_id != session.authority_session_id
        {
            return Err(CommandError::Authority);
        }
        if now < self.issued_at || now >= self.deadline_at || now >= session.authority_expires_at {
            return Err(CommandError::Deadline);
        }
        if matches!(self.operation, AgentOperation::StartRunner(_))
            && self.deadline_at > session.authority_expires_at
        {
            return Err(CommandError::Deadline);
        }
        if self.request_hash != self.canonical_hash()? {
            return Err(CommandError::Hash);
        }
        Ok(ValidatedCommand(self))
    }
}
