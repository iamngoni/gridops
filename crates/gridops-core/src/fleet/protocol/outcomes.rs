//! Typed, command-correlated results returned by an authenticated host agent.
//!
//! Results are parseable journal evidence.  `validate_against` is the apply
//! boundary: a result from another command, host epoch, authority session, or
//! immutable request hash can never be applied to the command being reduced.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    agent::{AgentCommandEnvelope, AgentOperation, CommandScope, LocalResourceIdentity},
    chunks::TransferChunk,
    primitives::{
        AttemptNumber, BoundedInteger, BrowserUint53, ByteOffset, Sha256Digest, WireTimestamp,
    },
};
use crate::fleet::ids::{
    ArtifactId, AuthoritySessionId, CommandId, ControlPlaneIncarnation, DiagnosticId, Generation,
    HostEpoch, HostId, LogStreamId, OperationId, PreparedEnvironmentId, ProfileId,
    ReleaseManifestId, ResultId, Revision, UpstreamText,
};

/// A bounded, non-secret diagnostic classification.  Detailed provider or
/// process output belongs in a separately authorized log stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    Authority,
    Configuration,
    Deadline,
    Dependency,
    ImageUnavailable,
    Permission,
    ResourceUnavailable,
    Runtime,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    Cancelled,
    Configuration,
    Dependency,
    Deadline,
    Permission,
    ResourceUnavailable,
    Runtime,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertainCode {
    AuthorityLost,
    DeliveryUnknown,
    LocalStateUnknown,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticReference {
    pub id: DiagnosticId,
    pub code: DiagnosticCode,
}

/// Agent claims are deliberately named as claims.  Durable absence and
/// delayed-start proof is established by the server journal, not by parsing
/// this object or by the boolean fields alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalAbsenceClaim {
    pub resource: LocalResourceIdentity,
    pub observed_at: WireTimestamp,
    pub evidence: DiagnosticReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelayedStartExclusionClaim {
    pub resource: LocalResourceIdentity,
    pub excluded_at: WireTimestamp,
    pub evidence: DiagnosticReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxExitStatus {
    Exited { code: BrowserUint53 },
    Signaled { signal: BoundedInteger<1, 64> },
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogStreamReference {
    pub stream_id: LogStreamId,
    pub bytes: ByteOffset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub artifact_id: ArtifactId,
    pub size_bytes: ByteOffset,
    pub digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxFileResult {
    Read {
        chunk: Option<TransferChunk>,
        size_bytes: ByteOffset,
        digest: Sha256Digest,
    },
    Written {
        artifact: ArtifactReceipt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxArtifactResult {
    Published {
        artifact: ArtifactReceipt,
    },
    Fetched {
        chunk: TransferChunk,
        size_bytes: ByteOffset,
        digest: Sha256Digest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SuccessEvidence {
    PreparedEnvironment {
        prepared_environment_id: PreparedEnvironmentId,
    },
    RunnerStarted {
        prepared_environment_id: PreparedEnvironmentId,
        execution_profile_id: ProfileId,
        profile_revision: Revision,
        local_identity: LocalResourceIdentity,
    },
    Inspected {
        local_identity: Option<LocalResourceIdentity>,
    },
    Drained {
        local_identity: Option<LocalResourceIdentity>,
    },
    Stopped {
        local_identity: LocalResourceIdentity,
    },
    Cleaned {
        local_identity: LocalResourceIdentity,
        local_absence: LocalAbsenceClaim,
        delayed_start_exclusion: DelayedStartExclusionClaim,
    },
    AgentUpgraded {
        signed_manifest_reference: ReleaseManifestId,
        version: UpstreamText,
    },
    SandboxExecuted {
        local_identity: LocalResourceIdentity,
        exit_status: SandboxExitStatus,
        stdout: Option<LogStreamReference>,
        stderr: Option<LogStreamReference>,
    },
    SandboxFileApplied {
        local_identity: LocalResourceIdentity,
        action: super::agent::SandboxFileAction,
        result: SandboxFileResult,
    },
    SandboxArtifactApplied {
        local_identity: LocalResourceIdentity,
        action: super::agent::SandboxArtifactAction,
        result: SandboxArtifactResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandOutcome {
    Accepted {
        attempt: AttemptNumber,
    },
    Running {
        attempt: AttemptNumber,
    },
    Succeeded {
        evidence: Box<SuccessEvidence>,
    },
    Failed {
        code: FailureCode,
        retryable: bool,
        diagnostic: Option<DiagnosticReference>,
    },
    Uncertain {
        code: UncertainCode,
        diagnostic: Option<DiagnosticReference>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommandResult {
    pub result_id: ResultId,
    pub command_id: CommandId,
    pub operation_id: OperationId,
    pub host_id: HostId,
    pub host_epoch: HostEpoch,
    pub control_plane_incarnation: ControlPlaneIncarnation,
    pub authority_session_id: AuthoritySessionId,
    pub scope: CommandScope,
    pub generation: Option<Generation>,
    pub request_hash: Sha256Digest,
    pub observed_at: WireTimestamp,
    pub outcome: CommandOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OutcomeError {
    #[error("result does not match the immutable command identity")]
    Correlation,
    #[error("result scope does not match the command operation")]
    Scope,
    #[error("result evidence does not match the command operation")]
    Evidence,
    #[error("result request hash is not the command hash")]
    Hash,
}

/// A result that has passed exact command correlation.  The wrapper does not
/// prove authority, permissions, local journal ownership, absence, or durable
/// persistence, and must never be treated as authorization to apply state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedResult<'a>(&'a AgentCommandResult);

impl<'a> ValidatedResult<'a> {
    #[must_use]
    pub const fn result(self) -> &'a AgentCommandResult {
        self.0
    }
}

impl AgentCommandResult {
    pub fn validate_against(
        &self,
        command: &AgentCommandEnvelope,
    ) -> Result<ValidatedResult<'_>, OutcomeError> {
        if self.command_id != command.command_id
            || self.operation_id != command.operation_id
            || self.host_id != command.host_id
            || self.host_epoch != command.host_epoch
            || self.control_plane_incarnation != command.control_plane_incarnation
            || self.authority_session_id != command.authority_session_id
            || self.scope != command.scope
        {
            return Err(OutcomeError::Correlation);
        }
        if String::from(self.request_hash) != command.request_hash {
            return Err(OutcomeError::Hash);
        }
        if !scope_generation_matches(&self.scope, self.generation) {
            return Err(OutcomeError::Scope);
        }
        if let CommandOutcome::Succeeded { evidence } = &self.outcome {
            if !evidence_matches(command, evidence.as_ref()) {
                return Err(OutcomeError::Evidence);
            }
        }
        Ok(ValidatedResult(self))
    }
}

fn scope_generation_matches(scope: &CommandScope, generation: Option<Generation>) -> bool {
    match (scope, generation) {
        (CommandScope::Host, None) => true,
        (
            CommandScope::Placement {
                generation: expected,
                ..
            },
            Some(actual),
        ) => *expected == actual,
        _ => false,
    }
}

fn evidence_matches(command: &AgentCommandEnvelope, evidence: &SuccessEvidence) -> bool {
    match (&command.operation, evidence) {
        (AgentOperation::PrepareEnvironment(_), SuccessEvidence::PreparedEnvironment { .. }) => {
            true
        }
        (
            AgentOperation::StartRunner(command),
            SuccessEvidence::RunnerStarted {
                prepared_environment_id,
                execution_profile_id,
                profile_revision,
                ..
            },
        ) => {
            command.prepared_environment_id == *prepared_environment_id
                && command.execution_profile_id == *execution_profile_id
                && command.profile_revision == *profile_revision
        }
        (AgentOperation::InspectHost, SuccessEvidence::Inspected { .. }) => true,
        (
            AgentOperation::Inspect { expected_resource },
            SuccessEvidence::Inspected { local_identity },
        ) => local_identity.as_ref() == Some(expected_resource),
        (AgentOperation::DrainHost, SuccessEvidence::Drained { local_identity }) => {
            local_identity.is_none()
        }
        (
            AgentOperation::Drain { expected_resource },
            SuccessEvidence::Drained { local_identity },
        ) => local_identity.as_ref() == Some(expected_resource),
        (
            AgentOperation::Stop {
                expected_resource, ..
            },
            SuccessEvidence::Stopped { local_identity },
        ) => expected_resource == local_identity,
        (
            AgentOperation::Cleanup { expected_resource },
            SuccessEvidence::Cleaned {
                local_identity,
                local_absence,
                delayed_start_exclusion,
            },
        ) => {
            expected_resource == local_identity
                && local_absence.resource == *local_identity
                && delayed_start_exclusion.resource == *local_identity
        }
        (
            AgentOperation::UpgradeAgent {
                signed_manifest_reference,
                expected_version,
                ..
            },
            SuccessEvidence::AgentUpgraded {
                signed_manifest_reference: actual_manifest,
                version,
            },
        ) => signed_manifest_reference == actual_manifest && expected_version == version,
        (
            AgentOperation::ExecuteSandbox {
                expected_resource, ..
            },
            SuccessEvidence::SandboxExecuted { local_identity, .. },
        ) => expected_resource == local_identity,
        (
            AgentOperation::SandboxFile {
                expected_resource,
                action,
            },
            SuccessEvidence::SandboxFileApplied {
                local_identity,
                action: actual_action,
                result,
            },
        ) => {
            expected_resource == local_identity
                && action == actual_action
                && file_result_matches(action, result, &command.scope)
        }
        (
            AgentOperation::SandboxArtifact {
                expected_resource,
                action,
            },
            SuccessEvidence::SandboxArtifactApplied {
                local_identity,
                action: actual_action,
                result,
            },
        ) => {
            expected_resource == local_identity
                && action == actual_action
                && artifact_result_matches(action, result, &command.scope)
        }
        _ => false,
    }
}

fn file_result_matches(
    action: &super::agent::SandboxFileAction,
    result: &SandboxFileResult,
    scope: &CommandScope,
) -> bool {
    match (action, result) {
        (
            super::agent::SandboxFileAction::Read {
                offset, max_bytes, ..
            },
            SandboxFileResult::Read {
                chunk,
                size_bytes,
                digest,
            },
        ) => match chunk {
            Some(chunk) => {
                let CommandScope::Placement {
                    placement_id,
                    backend_id,
                    generation,
                } = scope
                else {
                    return false;
                };
                chunk.offset() == *offset
                    && size_bytes.get() == chunk.bytes().len() as u64
                    && size_bytes.get() <= max_bytes.get()
                    && *digest == Sha256Digest::of(chunk.bytes())
                    && chunk.owner().placement_id == *placement_id
                    && chunk.owner().backend_id == *backend_id
                    && chunk.owner().generation == *generation
            }
            None => size_bytes.get() == 0 && *digest == Sha256Digest::of(&[]),
        },
        (
            super::agent::SandboxFileAction::Write {
                artifact_id,
                expected_length,
                sha256,
                ..
            },
            SandboxFileResult::Written { artifact },
        ) => {
            artifact.artifact_id == *artifact_id
                && artifact.size_bytes == *expected_length
                && artifact.digest == *sha256
        }
        _ => false,
    }
}

fn artifact_result_matches(
    action: &super::agent::SandboxArtifactAction,
    result: &SandboxArtifactResult,
    scope: &CommandScope,
) -> bool {
    match (action, result) {
        (
            super::agent::SandboxArtifactAction::Publish { artifact_id, .. },
            SandboxArtifactResult::Published { artifact },
        ) => *artifact_id == artifact.artifact_id,
        (
            super::agent::SandboxArtifactAction::Fetch {
                artifact_id,
                offset,
                max_bytes,
            },
            SandboxArtifactResult::Fetched {
                chunk,
                size_bytes,
                digest,
            },
        ) => {
            let CommandScope::Placement {
                placement_id,
                backend_id,
                generation,
            } = scope
            else {
                return false;
            };
            matches!(
                chunk.stream(),
                super::chunks::StreamReference::Artifact { artifact_id: stream_id }
                    if stream_id == artifact_id
            ) && chunk.offset() == *offset
                && size_bytes.get() == chunk.bytes().len() as u64
                && size_bytes.get() <= max_bytes.get()
                && *digest == Sha256Digest::of(chunk.bytes())
                && chunk.owner().placement_id == *placement_id
                && chunk.owner().backend_id == *backend_id
                && chunk.owner().generation == *generation
        }
        _ => false,
    }
}
