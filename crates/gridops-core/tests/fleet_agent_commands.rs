//! Exercises command/session fencing and owned execution contracts. A valid
//! command is still untrusted for dispatch until journal and grants are checked.

use anyhow::Result;
use gridops_core::fleet::{
    ids::*,
    protocol::{agent::*, primitives::*},
};
use serde_json::{Value, json};

fn id(n: u32) -> String {
    format!("{n:08x}-0000-4000-8000-000000000001")
}
fn fixture() -> Value {
    json!({
        "protocol_version":1,"command_id":id(1),"operation_id":id(2),"idempotency_key":"prepare:fixture",
        "host_id":id(3),"host_epoch":1,"control_plane_incarnation":id(4),"authority_session_id":id(5),
        "scope":{"kind":"placement","placement_id":id(6),"backend_id":id(7),"generation":1},
        "expected_revision":1,"issued_at":"2026-10-09T12:00:00.000Z","deadline_at":"2026-10-09T12:20:00.000Z",
        "request_hash":"","operation":{
            "kind":"prepare_environment","execution_profile_id":id(8),"profile_revision":1,"workload":"ci_runner",
            "target":{"platform":"github_repository","installation_id":1,"repository_id":10},
            "runtime_kind":"docker","execution_os":"linux","architecture":"x64","mode":"ephemeral",
            "image":"runner:fixture","labels":["self-hosted","Linux"],"configuration":{},
            "resource_request":{"cpu_millis":1000,"memory_mib":1024,"disk_bytes":1_073_741_824},
            "allocation_ids":[id(9),id(10)]
        }
    })
}
fn command(value: Value) -> Result<AgentCommandEnvelope> {
    Ok(serde_json::from_value::<AgentCommandEnvelope>(value)?.seal_hash()?)
}
fn session(command: &AgentCommandEnvelope) -> Result<ExpectedAgentSession> {
    Ok(ExpectedAgentSession {
        host_id: command.host_id,
        host_epoch: command.host_epoch,
        control_plane_incarnation: command.control_plane_incarnation,
        authority_session_id: command.authority_session_id,
        credential_generation: Generation::new(1)?,
        authority_expires_at: WireTimestamp::from_millis(command.issued_at.millis() + 90_000)?,
    })
}

#[test]
fn immutable_hash_and_exact_session_fence_commands() -> Result<()> {
    let original = command(fixture())?;
    let current = session(&original)?;
    let now = original.issued_at;
    assert_eq!(
        original.validate_against(&current, now)?.envelope(),
        &original
    );
    let replay: AgentCommandEnvelope =
        parse_json(&serde_json::to_vec(&original)?, DEFAULT_JSON_BYTES)?;
    assert_eq!(replay, original);
    for field in [
        "host_id",
        "host_epoch",
        "control_plane_incarnation",
        "authority_session_id",
    ] {
        let mut altered = serde_json::to_value(&original)?;
        altered[field] = if field == "host_epoch" {
            2.into()
        } else {
            id(11).into()
        };
        let altered = command(altered)?;
        assert_eq!(
            altered.validate_against(&current, now).err(),
            Some(CommandError::Authority),
            "{field}"
        );
    }
    let mut altered = original.clone();
    altered.command_id = id(12).parse()?;
    assert_eq!(
        altered.validate_against(&current, now).err(),
        Some(CommandError::Hash)
    );
    let mut altered = original.clone();
    altered.scope = CommandScope::Placement {
        placement_id: id(13).parse()?,
        backend_id: id(7).parse()?,
        generation: Generation::new(2)?,
    };
    assert_eq!(
        altered.validate_against(&current, now).err(),
        Some(CommandError::Hash)
    );
    for time in [
        now.millis() - 1,
        current.authority_expires_at.millis(),
        original.deadline_at.millis(),
    ] {
        assert_eq!(
            original
                .validate_against(&current, WireTimestamp::from_millis(time)?)
                .err(),
            Some(CommandError::Deadline)
        );
    }
    let mut unsafe_body = fixture();
    unsafe_body["token"] = "synthetic-secret".into();
    assert!(serde_json::from_value::<AgentCommandEnvelope>(unsafe_body).is_err());
    Ok(())
}

fn start_fixture() -> Value {
    let mut value = fixture();
    value["deadline_at"] = "2026-10-09T12:01:00.000Z".into();
    value["operation"] = json!({"kind":"start_runner","prepared_environment_id":id(14),"execution_profile_id":id(8),
        "profile_revision":1,"bootstrap_reference":id(15),"bootstrap_expires_at":"2026-10-09T12:01:00.000Z",
        "resource_request":{"cpu_millis":1000,"memory_mib":1024,"disk_bytes":1_073_741_824},"allocation_ids":[id(9),id(10)]});
    value
}

#[test]
fn native_start_uses_prepared_identity_and_bounded_grants() -> Result<()> {
    let start = command(start_fixture())?;
    assert!(
        start
            .validate_against(&session(&start)?, start.issued_at)
            .is_ok()
    );
    assert!(!serde_json::to_string(&start)?.contains("process_id"));
    let mut wrong = start_fixture();
    wrong["scope"] = json!({"kind":"host"});
    assert_eq!(
        serde_json::from_value::<AgentCommandEnvelope>(wrong)?
            .seal_hash()
            .err(),
        Some(CommandError::Scope)
    );
    for invalid_deadline in ["2026-10-09T12:00:00.000Z", "2026-10-09T12:01:00.001Z"] {
        let mut wrong = start_fixture();
        wrong["deadline_at"] = invalid_deadline.into();
        assert_eq!(
            serde_json::from_value::<AgentCommandEnvelope>(wrong)?
                .seal_hash()
                .err(),
            Some(CommandError::Deadline)
        );
    }
    let mut wrong = start_fixture();
    wrong["operation"]["bootstrap_expires_at"] = "2026-10-09T12:00:59.999Z".into();
    assert_eq!(
        serde_json::from_value::<AgentCommandEnvelope>(wrong)?
            .seal_hash()
            .err(),
        Some(CommandError::Deadline)
    );
    let mut current = session(&start)?;
    current.authority_expires_at = WireTimestamp::from_millis(start.deadline_at.millis() - 1)?;
    assert_eq!(
        start.validate_against(&current, start.issued_at).err(),
        Some(CommandError::Deadline)
    );
    let mut missing = start_fixture();
    missing["operation"]
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("operation"))?
        .remove("prepared_environment_id");
    assert!(serde_json::from_value::<AgentCommandEnvelope>(missing).is_err());
    Ok(())
}

#[test]
fn lifecycle_operations_require_exact_owned_identity() -> Result<()> {
    let native = json!({"kind":"native_process","boot_id":"boot-fixture","process_id":42,"start_marker":"start-fixture"});
    let docker = json!({"kind":"docker","object_id":"owned-container"});
    let operations = [
        json!({"kind":"inspect","expected_resource":native}),
        json!({"kind":"drain","expected_resource":native}),
        json!({"kind":"stop","expected_resource":native,"mode":"idle_only"}),
        json!({"kind":"stop","expected_resource":native,"mode":"cancel_busy"}),
        json!({"kind":"cleanup","expected_resource":native}),
        json!({"kind":"sandbox_file","expected_resource":docker,"action":{"kind":"read","path":"work/log.txt","offset":0,"max_bytes":1024}}),
        json!({"kind":"sandbox_file","expected_resource":docker,"action":{"kind":"write","path":"work/log.txt","artifact_id":id(20),"expected_length":0,"sha256":Sha256Digest::of(b"")}}),
        json!({"kind":"sandbox_artifact","expected_resource":docker,"action":{"kind":"publish","path":"work/log.txt","artifact_id":id(20)}}),
        json!({"kind":"sandbox_artifact","expected_resource":docker,"action":{"kind":"fetch","artifact_id":id(20),"offset":0,"max_bytes":1024}}),
    ];
    for operation in operations {
        let mut value = fixture();
        value["operation"] = operation;
        let parsed = command(value.clone())?;
        assert!(
            parsed
                .validate_against(&session(&parsed)?, parsed.issued_at)
                .is_ok()
        );
        value["scope"] = json!({"kind":"host"});
        assert_eq!(
            serde_json::from_value::<AgentCommandEnvelope>(value)?
                .seal_hash()
                .err(),
            Some(CommandError::Scope)
        );
    }
    for operation in [
        json!({"kind":"inspect_host"}),
        json!({"kind":"drain_host"}),
        json!({"kind":"upgrade_agent","signed_manifest_reference":id(21),"expected_version":"1.0.1","current_version":"1.0.0","drain_operation_id":id(2)}),
    ] {
        let mut value = fixture();
        value["operation"] = operation;
        value["scope"] = json!({"kind":"host"});
        assert!(command(value).is_ok());
    }
    for resource in [
        json!({"kind":"native_process","process_id":42}),
        json!({"kind":"path","path":"/tmp/arbitrary"}),
    ] {
        let mut value = fixture();
        value["operation"] = json!({"kind":"stop","expected_resource":resource,"mode":"idle_only"});
        assert!(serde_json::from_value::<AgentCommandEnvelope>(value).is_err());
    }
    Ok(())
}

#[test]
fn sandbox_supports_real_scripts_with_bounded_redacted_content() -> Result<()> {
    let mut value = fixture();
    value["operation"] = json!({"kind":"execute_sandbox","expected_resource":{"kind":"docker","object_id":"owned-container"},
        "program":"/bin/sh","argv":["-c","synthetic-sensitive\necho done","", "a".repeat(1024)],
        "working_directory":"work","timeout_ms":300_000,"output_limit_bytes":1_048_576});
    let parsed = command(value.clone())?;
    assert!(!format!("{parsed:?}").contains("sensitive"));
    for argv in [json!(["a".repeat(16385)]), json!(["bad\u{0000}argument"])] {
        let mut bad = value.clone();
        bad["operation"]["argv"] = argv;
        assert!(serde_json::from_value::<AgentCommandEnvelope>(bad).is_err());
    }
    for argv in [json!(vec!["a".repeat(16384); 5]), json!(vec![""; 129])] {
        let mut bad = value.clone();
        bad["operation"]["argv"] = argv;
        assert_eq!(
            serde_json::from_value::<AgentCommandEnvelope>(bad)?
                .seal_hash()
                .err(),
            Some(CommandError::Configuration)
        );
    }
    for kind in ["host_shell", "powershell", "exec"] {
        let mut bad = value.clone();
        bad["operation"]["kind"] = kind.into();
        assert!(serde_json::from_value::<AgentCommandEnvelope>(bad).is_err());
    }
    value["operation"]["expected_resource"] = json!({"kind":"tart_vm","name":"unowned-vm"});
    assert_eq!(
        serde_json::from_value::<AgentCommandEnvelope>(value)?
            .seal_hash()
            .err(),
        Some(CommandError::Configuration)
    );
    Ok(())
}

#[test]
fn preparation_checks_backend_matrix_and_ancestor_allocation_depth() -> Result<()> {
    for (runtime, os, arch) in [
        ("docker", "linux", "x64"),
        ("docker", "linux", "arm64"),
        ("native_process", "windows", "x64"),
        ("native_process", "windows", "arm64"),
        ("native_process", "macos", "x64"),
        ("tart_vm", "macos", "arm64"),
    ] {
        let mut value = fixture();
        value["operation"]["runtime_kind"] = runtime.into();
        value["operation"]["execution_os"] = os.into();
        value["operation"]["architecture"] = arch.into();
        assert!(command(value).is_ok());
    }
    for (runtime, os, arch) in [
        ("docker", "windows", "x64"),
        ("docker", "macos", "arm64"),
        ("tart_vm", "macos", "x64"),
        ("tart_vm", "linux", "arm64"),
    ] {
        let mut value = fixture();
        value["operation"]["runtime_kind"] = runtime.into();
        value["operation"]["execution_os"] = os.into();
        value["operation"]["architecture"] = arch.into();
        assert_eq!(
            serde_json::from_value::<AgentCommandEnvelope>(value)?
                .seal_hash()
                .err(),
            Some(CommandError::Configuration)
        );
    }
    for count in [0, 64, 65] {
        let mut value = fixture();
        value["operation"]["allocation_ids"] = json!((1..=count).map(id).collect::<Vec<_>>());
        assert_eq!(command(value).is_ok(), count == 64);
    }
    let mut value = fixture();
    value["operation"]["allocation_ids"] = json!([id(9), id(9)]);
    assert!(command(value).is_err());
    Ok(())
}

#[test]
fn timestamp_constructor_and_database_round_trip_agree_at_boundaries() -> Result<()> {
    for millis in [-62_167_219_200_000, 253_402_300_799_999] {
        let time = WireTimestamp::from_millis(millis)?;
        let decoded: WireTimestamp = serde_json::from_str(&serde_json::to_string(&time)?)?;
        assert_eq!(decoded.millis(), millis);
    }
    for millis in [-62_167_219_200_001, 253_402_300_800_000, i64::MAX, i64::MIN] {
        assert!(WireTimestamp::from_millis(millis).is_err());
    }
    assert!(WireTimestamp::try_from("2016-12-31T23:59:60.000Z".to_owned()).is_err());
    Ok(())
}

#[test]
fn content_and_image_boundaries_do_not_use_identity_limits() -> Result<()> {
    assert!(ImageReference::try_from("a".repeat(300)).is_ok());
    assert!(ImageReference::try_from("a".repeat(301)).is_err());
    for image in ["", "a\nb"] {
        assert!(ImageReference::try_from(image.to_owned()).is_err());
    }
    assert!(SandboxProgram::try_from("a".repeat(1024)).is_ok());
    for program in [String::new(), "a".repeat(1025), "nul\0program".into()] {
        assert!(SandboxProgram::try_from(program).is_err());
    }
    let program = SandboxProgram::try_from("fixture-private-command".to_owned())?;
    assert_eq!(program.as_str(), "fixture-private-command");
    assert!(!format!("{program:?}").contains("fixture-private"));
    let argument = SandboxArgument::try_from("fixture-private-argument".to_owned())?;
    assert!(!format!("{argument:?}").contains("fixture-private"));
    let digest = Sha256Digest::of(b"example");
    let wire = String::from(digest);
    assert_eq!(Sha256Digest::try_from(wire.clone())?, digest);
    for invalid in [
        String::new(),
        format!("{wire}="),
        "!".repeat(43),
        "A".repeat(42) + "B",
    ] {
        assert!(Sha256Digest::try_from(invalid).is_err());
    }
    Ok(())
}
