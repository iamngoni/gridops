//! Exercises result correlation and bounded event batches against owned command fixtures.
//! Claims remain untrusted until durable reconciliation supplies runtime proof.

use gridops_core::fleet::ids::{
    AuthoritySessionId, ControlPlaneIncarnation, DiagnosticId, PreparedEnvironmentId, ResultId,
    UpstreamText,
};
use gridops_core::fleet::protocol::{
    agent::{
        AgentCommandEnvelope, AgentOperation, CommandScope, ExecutionConfiguration,
        LocalResourceIdentity, PrepareEnvironment, PreparedWorkload, RunnerMode, StopMode,
    },
    chunks::{ChunkOwnership, StreamReference, TransferChunk},
    events::{
        AckError, AgentEvent, AgentEventAck, AgentEventAckData, AgentEventBatch,
        AgentEventBatchData, AgentEventKind, EventBatchError,
    },
    outcomes::{
        AgentCommandResult, ArtifactReceipt, CommandOutcome, DelayedStartExclusionClaim,
        DiagnosticCode, DiagnosticReference, FailureCode, LocalAbsenceClaim, LogStreamReference,
        OutcomeError, SandboxArtifactResult, SandboxExitStatus, SandboxFileResult, SuccessEvidence,
    },
    primitives::{
        BrowserUint53, ByteOffset, IdempotencyKey, ImageReference, MAX_BATCH_BYTES,
        ProtocolVersion, Sha256Digest, WireTimestamp, parse_json,
    },
};
use gridops_core::fleet::{
    Architecture, BackendId, CiTarget, CommandId, ExternalId, Generation, HostEpoch, HostId,
    OperationId, PlacementId, ProfileId, ResourceRequest, Revision,
};
use serde_json::json;

#[expect(clippy::expect_used, reason = "constant valid fixture identifiers")]
fn id<T: std::str::FromStr>(value: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    value.parse().expect("valid test identifier")
}

#[expect(
    clippy::unwrap_used,
    reason = "constant valid protocol fixture construction"
)]
fn command() -> AgentCommandEnvelope {
    let profile = id::<ProfileId>("40000000-0000-4000-8000-000000000001");
    let host = id::<HostId>("10000000-0000-4000-8000-000000000001");
    let backend = id::<BackendId>("13000000-0000-4000-8000-000000000001");
    let placement = id::<PlacementId>("60000000-0000-4000-8000-000000000001");
    let generation = Generation::new(1).unwrap();
    AgentCommandEnvelope {
        protocol_version: ProtocolVersion::new(1).unwrap(),
        command_id: id::<CommandId>("70000000-0000-4000-8000-000000000001"),
        operation_id: id::<OperationId>("50000000-0000-4000-8000-000000000001"),
        idempotency_key: IdempotencyKey::parse("outcome-test").unwrap(),
        host_id: host,
        host_epoch: HostEpoch::new(1).unwrap(),
        control_plane_incarnation: id::<ControlPlaneIncarnation>(
            "77000000-0000-4000-8000-000000000001",
        ),
        authority_session_id: id::<AuthoritySessionId>("88000000-0000-4000-8000-000000000001"),
        scope: CommandScope::Placement {
            placement_id: placement,
            backend_id: backend,
            generation,
        },
        expected_revision: Revision::new(1).unwrap(),
        issued_at: WireTimestamp::from_millis(1_700_000_000_000).unwrap(),
        deadline_at: WireTimestamp::from_millis(1_700_000_060_000).unwrap(),
        request_hash: String::new(),
        operation: AgentOperation::PrepareEnvironment(PrepareEnvironment {
            execution_profile_id: profile,
            profile_revision: Revision::new(1).unwrap(),
            workload: PreparedWorkload::CiRunner,
            target: CiTarget::GithubRepository {
                installation_id: ExternalId::new(1).unwrap(),
                repository_id: ExternalId::new(2).unwrap(),
            },
            runtime_kind: gridops_core::fleet::RuntimeKind::Docker,
            execution_os: gridops_core::fleet::ExecutionOs::Linux,
            architecture: Architecture::Arm64,
            mode: RunnerMode::Ephemeral,
            image: ImageReference::try_from("runner:test".to_owned()).unwrap(),
            labels: vec![UpstreamText::new("self-hosted").unwrap()],
            configuration: ExecutionConfiguration::default(),
            resource_request: ResourceRequest::new(100, 64, 0).unwrap(),
            allocation_ids: vec![id("61000000-0000-4000-8000-000000000001")],
        }),
    }
    .seal_hash()
    .unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "constant valid protocol fixture construction"
)]
fn result_for(command: &AgentCommandEnvelope) -> AgentCommandResult {
    AgentCommandResult {
        result_id: id::<ResultId>("71000000-0000-4000-8000-000000000001"),
        command_id: command.command_id,
        operation_id: command.operation_id,
        host_id: command.host_id,
        host_epoch: command.host_epoch,
        control_plane_incarnation: command.control_plane_incarnation,
        authority_session_id: command.authority_session_id,
        scope: command.scope.clone(),
        generation: match &command.scope {
            CommandScope::Host => None,
            CommandScope::Placement { generation, .. } => Some(*generation),
        },
        request_hash: Sha256Digest::try_from(command.request_hash.clone()).unwrap(),
        observed_at: WireTimestamp::from_millis(1_700_000_001_000).unwrap(),
        outcome: CommandOutcome::Succeeded {
            evidence: Box::new(SuccessEvidence::PreparedEnvironment {
                prepared_environment_id: id::<PreparedEnvironmentId>(
                    "72000000-0000-4000-8000-000000000001",
                ),
            }),
        },
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "constant valid protocol fixture construction"
)]
fn docker_identity() -> LocalResourceIdentity {
    LocalResourceIdentity::Docker {
        object_id: UpstreamText::new("fixture-container").unwrap(),
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "constant valid protocol fixture construction"
)]
fn operation(value: serde_json::Value) -> AgentOperation {
    serde_json::from_value(value).unwrap()
}

fn file_action(
    value: serde_json::Value,
) -> gridops_core::fleet::protocol::agent::SandboxFileAction {
    match operation(value) {
        AgentOperation::SandboxFile { action, .. } => action,
        _ => unreachable!(),
    }
}

fn host_command(operation: AgentOperation) -> AgentCommandEnvelope {
    let mut command = command();
    command.scope = CommandScope::Host;
    command.operation = operation;
    command
}

fn placement_command(operation: AgentOperation) -> AgentCommandEnvelope {
    let mut command = command();
    command.operation = operation;
    command
}

#[expect(
    clippy::unwrap_used,
    reason = "constant valid protocol fixture construction"
)]
fn claim(resource: LocalResourceIdentity, diagnostic_id: &str) -> LocalAbsenceClaim {
    LocalAbsenceClaim {
        resource,
        observed_at: WireTimestamp::from_millis(1_700_000_002_000).unwrap(),
        evidence: DiagnosticReference {
            id: id(diagnostic_id),
            code: DiagnosticCode::Runtime,
        },
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "constant valid protocol fixture construction"
)]
fn exclusion(resource: LocalResourceIdentity, diagnostic_id: &str) -> DelayedStartExclusionClaim {
    DelayedStartExclusionClaim {
        resource,
        excluded_at: WireTimestamp::from_millis(1_700_000_003_000).unwrap(),
        evidence: DiagnosticReference {
            id: id(diagnostic_id),
            code: DiagnosticCode::Runtime,
        },
    }
}

#[test]
fn result_requires_exact_command_session_scope_and_hash() {
    let command = command();
    let result = result_for(&command);
    assert!(result.validate_against(&command).is_ok());

    let mut stale = result.clone();
    stale.host_id = id("10000000-0000-4000-8000-000000000002");
    assert_eq!(
        stale.validate_against(&command),
        Err(OutcomeError::Correlation)
    );

    let mut wrong_hash = result;
    wrong_hash.request_hash = Sha256Digest::of(b"different");
    assert_eq!(
        wrong_hash.validate_against(&command),
        Err(OutcomeError::Hash)
    );
}

#[test]
fn success_evidence_is_operation_specific_and_unknown_fields_fail() -> anyhow::Result<()> {
    let command = command();
    let mut result = result_for(&command);
    result.outcome = CommandOutcome::Succeeded {
        evidence: Box::new(SuccessEvidence::RunnerStarted {
            prepared_environment_id: id("72000000-0000-4000-8000-000000000001"),
            execution_profile_id: id("40000000-0000-4000-8000-000000000001"),
            profile_revision: Revision::new(1)?,
            local_identity: LocalResourceIdentity::Docker {
                object_id: UpstreamText::new("fixture-container")?,
            },
        }),
    };
    assert_eq!(
        result.validate_against(&command),
        Err(OutcomeError::Evidence)
    );

    let mut encoded = serde_json::to_value(result_for(&command))?;
    encoded
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("expected JSON object"))?
        .insert("unexpected".to_owned(), json!(true));
    assert!(serde_json::from_value::<AgentCommandResult>(encoded).is_err());
    Ok(())
}

#[test]
fn every_success_evidence_variant_is_bound_to_command_state() -> anyhow::Result<()> {
    let base = command();
    let expected = docker_identity();
    let prepared_environment_id = id("72000000-0000-4000-8000-000000000001");
    let profile_id = id("40000000-0000-4000-8000-000000000001");
    let profile_revision = Revision::new(1)?;

    let assert_success = |command: AgentCommandEnvelope, evidence: SuccessEvidence| {
        let mut result = result_for(&command);
        result.outcome = CommandOutcome::Succeeded {
            evidence: Box::new(evidence),
        };
        assert!(result.validate_against(&command).is_ok());
    };

    assert_success(
        base.clone(),
        SuccessEvidence::PreparedEnvironment {
            prepared_environment_id,
        },
    );

    let start = placement_command(operation(json!({
        "kind":"start_runner",
        "prepared_environment_id":prepared_environment_id,
        "execution_profile_id":profile_id,
        "profile_revision":1,
        "bootstrap_reference":"73000000-0000-4000-8000-000000000001",
        "bootstrap_expires_at":"2026-10-09T12:01:00.000Z",
        "resource_request":{"cpu_millis":100,"memory_mib":64,"disk_bytes":0},
        "allocation_ids":[]
    })));
    assert_success(
        start,
        SuccessEvidence::RunnerStarted {
            prepared_environment_id,
            execution_profile_id: profile_id,
            profile_revision,
            local_identity: expected.clone(),
        },
    );

    assert_success(
        host_command(AgentOperation::InspectHost),
        SuccessEvidence::Inspected {
            local_identity: None,
        },
    );
    let inspect = placement_command(AgentOperation::Inspect {
        expected_resource: expected.clone(),
    });
    assert_success(
        inspect,
        SuccessEvidence::Inspected {
            local_identity: Some(expected.clone()),
        },
    );
    assert_success(
        host_command(AgentOperation::DrainHost),
        SuccessEvidence::Drained {
            local_identity: None,
        },
    );
    let drain = placement_command(AgentOperation::Drain {
        expected_resource: expected.clone(),
    });
    assert_success(
        drain,
        SuccessEvidence::Drained {
            local_identity: Some(expected.clone()),
        },
    );
    let stop = placement_command(AgentOperation::Stop {
        expected_resource: expected.clone(),
        mode: StopMode::IdleOnly,
    });
    assert_success(
        stop,
        SuccessEvidence::Stopped {
            local_identity: expected.clone(),
        },
    );
    let cleanup = placement_command(AgentOperation::Cleanup {
        expected_resource: expected.clone(),
    });
    assert_success(
        cleanup,
        SuccessEvidence::Cleaned {
            local_identity: expected.clone(),
            local_absence: claim(expected.clone(), "75000000-0000-4000-8000-000000000001"),
            delayed_start_exclusion: exclusion(
                expected.clone(),
                "76000000-0000-4000-8000-000000000001",
            ),
        },
    );
    let upgrade = host_command(operation(json!({
        "kind":"upgrade_agent",
        "signed_manifest_reference":"77000000-0000-4000-8000-000000000001",
        "expected_version":"1.2.3",
        "current_version":"1.2.2",
        "drain_operation_id":"78000000-0000-4000-8000-000000000001"
    })));
    assert_success(
        upgrade,
        SuccessEvidence::AgentUpgraded {
            signed_manifest_reference: id("77000000-0000-4000-8000-000000000001"),
            version: UpstreamText::new("1.2.3")?,
        },
    );

    let sandbox = placement_command(operation(json!({
        "kind":"execute_sandbox","expected_resource":{"kind":"docker","object_id":"fixture-container"},
        "program":"/bin/sh","argv":["-c","echo ok"],"working_directory":"work",
        "timeout_ms":1000,"output_limit_bytes":1024
    })));
    assert_success(
        sandbox,
        SuccessEvidence::SandboxExecuted {
            local_identity: expected.clone(),
            exit_status: SandboxExitStatus::Exited {
                code: BrowserUint53::new(0)?,
            },
            stdout: Some(LogStreamReference {
                stream_id: id("79000000-0000-4000-8000-000000000001"),
                bytes: ByteOffset::new(3)?,
            }),
            stderr: None,
        },
    );

    let file_read = placement_command(operation(json!({
        "kind":"sandbox_file","expected_resource":{"kind":"docker","object_id":"fixture-container"},
        "action":{"kind":"read","path":"work/result.txt","offset":0,"max_bytes":1024}
    })));
    let file_read_action = match &file_read.operation {
        AgentOperation::SandboxFile { action, .. } => action.clone(),
        _ => unreachable!(),
    };
    let chunk = TransferChunk::from_bytes(
        StreamReference::Log {
            stream_id: id("80000000-0000-4000-8000-000000000001"),
        },
        ChunkOwnership {
            placement_id: id("60000000-0000-4000-8000-000000000001"),
            backend_id: id("13000000-0000-4000-8000-000000000001"),
            generation: Generation::new(1)?,
        },
        ByteOffset::new(0)?,
        b"ok",
        true,
        None,
    )?;
    assert_success(
        file_read,
        SuccessEvidence::SandboxFileApplied {
            local_identity: expected.clone(),
            action: file_read_action,
            result: SandboxFileResult::Read {
                chunk: Some(chunk),
                size_bytes: ByteOffset::new(2)?,
                digest: Sha256Digest::of(b"ok"),
            },
        },
    );

    let file_write = placement_command(operation(json!({
        "kind":"sandbox_file","expected_resource":{"kind":"docker","object_id":"fixture-container"},
        "action":{"kind":"write","path":"work/result.txt","artifact_id":"81000000-0000-4000-8000-000000000001",
        "expected_length":2,"sha256":String::from(Sha256Digest::of(b"ok"))}
    })));
    let write_action = match &file_write.operation {
        AgentOperation::SandboxFile { action, .. } => action.clone(),
        _ => unreachable!(),
    };
    assert_success(
        file_write,
        SuccessEvidence::SandboxFileApplied {
            local_identity: expected.clone(),
            action: write_action,
            result: SandboxFileResult::Written {
                artifact: ArtifactReceipt {
                    artifact_id: id("81000000-0000-4000-8000-000000000001"),
                    size_bytes: ByteOffset::new(2)?,
                    digest: Sha256Digest::of(b"ok"),
                },
            },
        },
    );

    let artifact = placement_command(operation(json!({
        "kind":"sandbox_artifact","expected_resource":{"kind":"docker","object_id":"fixture-container"},
        "action":{"kind":"publish","path":"work/result.txt","artifact_id":"82000000-0000-4000-8000-000000000001"}
    })));
    let artifact_action = match &artifact.operation {
        AgentOperation::SandboxArtifact { action, .. } => action.clone(),
        _ => unreachable!(),
    };
    assert_success(
        artifact,
        SuccessEvidence::SandboxArtifactApplied {
            local_identity: expected,
            action: artifact_action,
            result: SandboxArtifactResult::Published {
                artifact: ArtifactReceipt {
                    artifact_id: id("82000000-0000-4000-8000-000000000001"),
                    size_bytes: ByteOffset::new(2)?,
                    digest: Sha256Digest::of(b"ok"),
                },
            },
        },
    );

    let artifact_fetch = placement_command(operation(json!({
        "kind":"sandbox_artifact","expected_resource":{"kind":"docker","object_id":"fixture-container"},
        "action":{"kind":"fetch","artifact_id":"82000000-0000-4000-8000-000000000001","offset":0,"max_bytes":1024}
    })));
    let fetched_chunk = TransferChunk::from_bytes(
        StreamReference::Artifact {
            artifact_id: id("82000000-0000-4000-8000-000000000001"),
        },
        ChunkOwnership {
            placement_id: id("60000000-0000-4000-8000-000000000001"),
            backend_id: id("13000000-0000-4000-8000-000000000001"),
            generation: Generation::new(1)?,
        },
        ByteOffset::new(0)?,
        b"ok",
        true,
        None,
    )?;
    let artifact_fetch_action = match &artifact_fetch.operation {
        AgentOperation::SandboxArtifact { action, .. } => action.clone(),
        _ => unreachable!(),
    };
    assert_success(
        artifact_fetch,
        SuccessEvidence::SandboxArtifactApplied {
            local_identity: docker_identity(),
            action: artifact_fetch_action,
            result: SandboxArtifactResult::Fetched {
                chunk: fetched_chunk,
                size_bytes: ByteOffset::new(2)?,
                digest: Sha256Digest::of(b"ok"),
            },
        },
    );
    Ok(())
}

#[test]
fn success_evidence_rejects_rebound_identity_and_start_profile() -> anyhow::Result<()> {
    let mut start = placement_command(operation(json!({
        "kind":"start_runner",
        "prepared_environment_id":"72000000-0000-4000-8000-000000000001",
        "execution_profile_id":"40000000-0000-4000-8000-000000000001",
        "profile_revision":1,
        "bootstrap_reference":"73000000-0000-4000-8000-000000000001",
        "bootstrap_expires_at":"2026-10-09T12:01:00.000Z",
        "resource_request":{"cpu_millis":100,"memory_mib":64,"disk_bytes":0},
        "allocation_ids":[]
    })));
    let mut result = result_for(&start);
    result.outcome = CommandOutcome::Succeeded {
        evidence: Box::new(SuccessEvidence::RunnerStarted {
            prepared_environment_id: id("72000000-0000-4000-8000-000000000002"),
            execution_profile_id: id("40000000-0000-4000-8000-000000000001"),
            profile_revision: Revision::new(1)?,
            local_identity: docker_identity(),
        }),
    };
    assert_eq!(result.validate_against(&start), Err(OutcomeError::Evidence));

    if let AgentOperation::StartRunner(operation) = &mut start.operation {
        operation.profile_revision = Revision::new(2)?;
    }
    let mut profile_mismatch = result_for(&start);
    profile_mismatch.outcome = CommandOutcome::Succeeded {
        evidence: Box::new(SuccessEvidence::RunnerStarted {
            prepared_environment_id: id("72000000-0000-4000-8000-000000000001"),
            execution_profile_id: id("40000000-0000-4000-8000-000000000001"),
            profile_revision: Revision::new(1)?,
            local_identity: docker_identity(),
        }),
    };
    assert_eq!(
        profile_mismatch.validate_against(&start),
        Err(OutcomeError::Evidence)
    );

    let expected = docker_identity();
    let cleanup = placement_command(AgentOperation::Cleanup {
        expected_resource: expected.clone(),
    });
    let mut cleanup_result = result_for(&cleanup);
    cleanup_result.outcome = CommandOutcome::Succeeded {
        evidence: Box::new(SuccessEvidence::Cleaned {
            local_identity: expected.clone(),
            local_absence: claim(
                LocalResourceIdentity::Docker {
                    object_id: UpstreamText::new("different-container")?,
                },
                "75000000-0000-4000-8000-000000000001",
            ),
            delayed_start_exclusion: exclusion(expected, "76000000-0000-4000-8000-000000000001"),
        }),
    };
    assert_eq!(
        cleanup_result.validate_against(&cleanup),
        Err(OutcomeError::Evidence)
    );
    Ok(())
}

#[test]
fn event_batches_are_homogeneous_bounded_and_strict() -> anyhow::Result<()> {
    let command = command();
    let result = result_for(&command);
    let event_id = id("73000000-0000-4000-8000-000000000001");
    let event = AgentEvent {
        event_id,
        observed_at: result.observed_at,
        host_id: command.host_id,
        host_epoch: command.host_epoch,
        control_plane_incarnation: command.control_plane_incarnation,
        authority_session_id: command.authority_session_id,
        scope: Some(command.scope.clone()),
        kind: AgentEventKind::CommandResult {
            result: Box::new(result),
        },
    };
    let data = AgentEventBatchData {
        batch_id: id("74000000-0000-4000-8000-000000000001"),
        host_id: command.host_id,
        host_epoch: command.host_epoch,
        control_plane_incarnation: command.control_plane_incarnation,
        authority_session_id: command.authority_session_id,
        events: vec![event.clone()],
    };
    assert!(AgentEventBatch::try_from(data.clone()).is_ok());

    let mut duplicate = data.clone();
    duplicate.events.push(event);
    assert_eq!(
        AgentEventBatch::try_from(duplicate),
        Err(EventBatchError::DuplicateEvent)
    );

    let mut wrong_host = data.clone();
    if let AgentEventKind::CommandResult { result } = &mut wrong_host.events[0].kind {
        result.host_id = id("10000000-0000-4000-8000-000000000002");
    }
    assert_eq!(
        AgentEventBatch::try_from(wrong_host),
        Err(EventBatchError::ResultMismatch)
    );

    let mut wrong_scope = data.clone();
    if let AgentEventKind::CommandResult { result } = &mut wrong_scope.events[0].kind {
        result.scope = CommandScope::Host;
    }
    assert_eq!(
        AgentEventBatch::try_from(wrong_scope),
        Err(EventBatchError::ScopeMismatch)
    );

    for kind in [
        AgentEventKind::LocalAbsenceReported {
            claim: claim(docker_identity(), "75000000-0000-4000-8000-000000000001"),
        },
        AgentEventKind::DelayedStartsExcluded {
            claim: exclusion(docker_identity(), "76000000-0000-4000-8000-000000000001"),
        },
        AgentEventKind::ResourceObserved {
            resource: docker_identity(),
        },
    ] {
        let mut placement_only = data.clone();
        placement_only.events[0].scope = None;
        placement_only.events[0].kind = kind;
        assert_eq!(
            AgentEventBatch::try_from(placement_only),
            Err(EventBatchError::ScopeMismatch)
        );
    }

    let mut heartbeat = data.clone();
    heartbeat.events[0].kind = AgentEventKind::Heartbeat;
    assert_eq!(
        AgentEventBatch::try_from(heartbeat),
        Err(EventBatchError::ScopeMismatch)
    );
    let mut host_heartbeat = data.clone();
    host_heartbeat.events[0].scope = Some(CommandScope::Host);
    host_heartbeat.events[0].kind = AgentEventKind::Heartbeat;
    assert!(AgentEventBatch::try_from(host_heartbeat).is_ok());

    assert_eq!(
        AgentEventBatch::parse(&serde_json::to_vec(&serde_json::json!({
            "batch_id": data.batch_id,
            "host_id": data.host_id,
            "host_epoch": data.host_epoch,
            "control_plane_incarnation": data.control_plane_incarnation,
            "authority_session_id": data.authority_session_id,
            "events": [{
                "event_id": data.events[0].event_id,
                "observed_at": data.events[0].observed_at,
                "host_id": data.events[0].host_id,
                "host_epoch": data.events[0].host_epoch,
                "control_plane_incarnation": data.events[0].control_plane_incarnation,
                "authority_session_id": data.events[0].authority_session_id,
                "scope": data.events[0].scope,
                "kind": {"kind":"command_accepted","command_id":command.command_id,"request_hash":"invalid"}
            }]
        }))?,),
        Err(EventBatchError::InvalidJson)
    );

    let mut direct_oversized = data.clone();
    let mut huge_result = result_for(&command);
    huge_result.outcome = CommandOutcome::Succeeded {
        evidence: Box::new(SuccessEvidence::SandboxFileApplied {
            local_identity: docker_identity(),
            action: file_action(json!({
                "kind":"sandbox_file","expected_resource":{"kind":"docker","object_id":"fixture-container"},
                "action":{"kind":"read","path":"work/result.txt","offset":0,"max_bytes":262_144}
            })),
            result: SandboxFileResult::Read {
                chunk: Some(TransferChunk::from_bytes(
                    StreamReference::Log {
                        stream_id: id("80000000-0000-4000-8000-000000000001"),
                    },
                    ChunkOwnership {
                        placement_id: id("60000000-0000-4000-8000-000000000001"),
                        backend_id: id("13000000-0000-4000-8000-000000000001"),
                        generation: Generation::new(1)?,
                    },
                    ByteOffset::new(0)?,
                    &vec![0; 262_144],
                    true,
                    None,
                )?),
                size_bytes: ByteOffset::new(262_144)?,
                digest: Sha256Digest::of(&vec![0; 262_144]),
            },
        }),
    };
    if let AgentEventKind::CommandResult { result } = &mut direct_oversized.events[0].kind {
        **result = huge_result;
    }
    assert_eq!(
        AgentEventBatch::try_from(direct_oversized),
        Err(EventBatchError::PayloadTooLarge)
    );

    let oversized = vec![b' '; MAX_BATCH_BYTES + 1];
    assert_eq!(
        AgentEventBatch::parse(&oversized),
        Err(EventBatchError::PayloadTooLarge)
    );
    assert!(parse_json::<AgentEventBatch>(&serde_json::to_vec(&data)?, MAX_BATCH_BYTES).is_ok());
    Ok(())
}

#[test]
fn event_acks_are_bounded_unique_and_partitioned() {
    let batch_id = id("74000000-0000-4000-8000-000000000001");
    let event_id = id("73000000-0000-4000-8000-000000000001");
    let result_id = id("71000000-0000-4000-8000-000000000001");
    let ack = |accepted, replayed, result_ids| {
        AgentEventAck::try_from(AgentEventAckData {
            batch_id,
            accepted,
            replayed,
            result_ids,
        })
    };
    assert!(ack(vec![event_id], vec![], vec![result_id]).is_ok());
    assert_eq!(
        ack(vec![event_id, event_id], vec![], vec![]),
        Err(AckError::Duplicate)
    );
    assert_eq!(
        ack(vec![event_id], vec![event_id], vec![]),
        Err(AckError::Overlap)
    );
    assert_eq!(
        ack(vec![], vec![], vec![result_id, result_id]),
        Err(AckError::Duplicate)
    );
    assert_eq!(
        ack(vec![event_id; 101], vec![], vec![]),
        Err(AckError::TooManyIds)
    );
    assert_eq!(
        ack(
            vec![event_id; 100],
            vec![id("73000000-0000-4000-8000-000000000002")],
            vec![]
        ),
        Err(AckError::TooManyIds)
    );
}

#[test]
fn diagnostics_and_absence_claims_do_not_echo_secret_text() {
    let command = command();
    let mut result = result_for(&command);
    result.outcome = CommandOutcome::Failed {
        code: FailureCode::Runtime,
        retryable: false,
        diagnostic: Some(DiagnosticReference {
            id: id::<DiagnosticId>("75000000-0000-4000-8000-000000000001"),
            code: DiagnosticCode::Runtime,
        }),
    };
    let debug = format!("{result:?}");
    assert!(!debug.contains("authorization=super-secret"));
    assert!(!debug.contains("token"));
}
