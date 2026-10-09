//! Untrusted discovery cannot invent support or carry policy. These fixtures
//! exercise exact session freshness, capability limits and upstream ceilings.

use anyhow::Result;
use gridops_core::fleet::{
    Architecture, ExecutionOs, HostEpoch, ResourceRequest, RuntimeKind,
    capabilities::*,
    protocol::{agent::RunnerMode, primitives::*},
};
use serde_json::{Value, json};

fn report() -> Value {
    json!({"schema_version":1,"runtime_kind":"docker","execution_os":"linux","architecture":"arm64",
        "supported_runners":[{"target_kind":"github_repository","mode":"ephemeral"}],
        "environments":[{"reference":"runner:fixture","readiness":"ready","version":"1.0"}],
        "resources":{"cpu":"hard_limit","memory":"hard_limit","disk":"estimate"},
        "interactive":"unsupported","docker_socket":"unsupported"})
}
struct Fixture {
    identity: BackendIdentity,
    requirement: ExecutionRequirement,
    probe: CurrentProbe,
    authority: CurrentAuthority,
    now: WireTimestamp,
}
fn fixture() -> Result<Fixture> {
    let host = "10000000-0000-4000-8000-000000000001".parse()?;
    let backend = "20000000-0000-4000-8000-000000000001".parse()?;
    let epoch = HostEpoch::new(1)?;
    let session = "30000000-0000-4000-8000-000000000001".parse()?;
    let incarnation = "40000000-0000-4000-8000-000000000001".parse()?;
    let boot = "boot-fixture".to_owned().try_into()?;
    Ok(Fixture {
        identity: BackendIdentity {
            host_id: host,
            backend_id: backend,
            runtime_kind: RuntimeKind::Docker,
            execution_os: ExecutionOs::Linux,
            architecture: Architecture::Arm64,
        },
        requirement: ExecutionRequirement {
            runtime_kind: RuntimeKind::Docker,
            execution_os: ExecutionOs::Linux,
            architecture: Architecture::Arm64,
            runner: SupportedRunner {
                target_kind: TargetKind::GithubRepository,
                mode: RunnerMode::Ephemeral,
            },
            environment: "runner:fixture".to_owned().try_into()?,
            resources: ResourceRequest::new(1000, 1024, 1024)?,
            requires_interactive: false,
            requires_docker_socket: false,
        },
        probe: CurrentProbe {
            host_id: host,
            backend_id: backend,
            received_at: WireTimestamp::from_millis(100_000)?,
            host_epoch: epoch,
            authority_session_id: session,
            boot_id: boot,
            control_plane_incarnation: incarnation,
        },
        authority: CurrentAuthority {
            host_id: host,
            host_epoch: epoch,
            authority_session_id: session,
            boot_id: "boot-fixture".to_owned().try_into()?,
            control_plane_incarnation: incarnation,
        },
        now: WireTimestamp::from_millis(100_000)?,
    })
}
fn evaluate(value: Value, f: &Fixture) -> Result<Result<CompatibleBackend, CapabilityRejection>> {
    Ok(
        serde_json::from_value::<BackendCapabilities>(value)?.evaluate(
            &f.identity,
            &f.requirement,
            Some(&f.probe),
            &f.authority,
            f.now,
        ),
    )
}

#[test]
fn discovery_is_typed_bounded_and_duplicate_free() -> Result<()> {
    for bad in [
        json!({}),
        json!({"anything":true}),
        json!({"schema_version":2}),
    ] {
        assert!(serde_json::from_value::<BackendCapabilities>(bad).is_err());
    }
    let mut bad = report();
    bad["allow_native"] = true.into();
    assert!(serde_json::from_value::<BackendCapabilities>(bad).is_err());
    for (field, value) in [
        ("runtime_kind", "shell"),
        ("interactive", "trusted"),
        ("docker_socket", "yes"),
    ] {
        let mut bad = report();
        bad[field] = value.into();
        assert!(serde_json::from_value::<BackendCapabilities>(bad).is_err());
    }
    let mut duplicate = report();
    duplicate["supported_runners"] = json!([{"target_kind":"github_repository","mode":"ephemeral"},{"target_kind":"github_repository","mode":"ephemeral"}]);
    assert!(serde_json::from_value::<BackendCapabilities>(duplicate).is_err());
    let mut duplicate = report();
    duplicate["environments"] = json!([{"reference":"same","readiness":"ready"},{"reference":"same","readiness":"preparable"}]);
    assert!(serde_json::from_value::<BackendCapabilities>(duplicate).is_err());
    for count in [128, 129] {
        let mut value = report();
        value["environments"] = json!(
            (0..count)
                .map(|n| json!({"reference":format!("image:{n}"),"readiness":"preparable"}))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            serde_json::from_value::<BackendCapabilities>(value).is_ok(),
            count == 128
        );
    }
    let mut oversized = report();
    oversized["supported_runners"] = json!(vec![
        json!({"target_kind":"github_repository","mode":"ephemeral"});
        33
    ]);
    assert!(serde_json::from_value::<BackendCapabilities>(oversized).is_err());
    let parsed: BackendCapabilities = serde_json::from_value(report())?;
    assert_eq!(
        serde_json::from_value::<BackendCapabilities>(serde_json::to_value(&parsed)?)?,
        parsed
    );
    assert_eq!(parsed.report().supported_runners.len(), 1);
    Ok(())
}

#[test]
fn current_session_probe_uses_exact_identity_and_server_freshness() -> Result<()> {
    let caps: BackendCapabilities = serde_json::from_value(report())?;
    let f = fixture()?;
    assert_eq!(
        caps.evaluate(&f.identity, &f.requirement, None, &f.authority, f.now)
            .err(),
        Some(CapabilityRejection::StaleProbe)
    );
    for (time, valid) in [
        (100_000, true),
        (144_999, true),
        (145_000, false),
        (99_999, false),
    ] {
        assert_eq!(
            caps.evaluate(
                &f.identity,
                &f.requirement,
                Some(&f.probe),
                &f.authority,
                WireTimestamp::from_millis(time)?
            )
            .is_ok(),
            valid
        );
    }
    for axis in 0..6 {
        let mut altered = f.probe.clone();
        match axis {
            0 => altered.host_id = "90000000-0000-4000-8000-000000000001".parse()?,
            1 => altered.backend_id = "90000000-0000-4000-8000-000000000001".parse()?,
            2 => altered.host_epoch = HostEpoch::new(2)?,
            3 => altered.authority_session_id = "90000000-0000-4000-8000-000000000001".parse()?,
            4 => altered.boot_id = "different-boot".to_owned().try_into()?,
            _ => {
                altered.control_plane_incarnation =
                    "90000000-0000-4000-8000-000000000001".parse()?;
            }
        }
        assert_eq!(
            caps.evaluate(
                &f.identity,
                &f.requirement,
                Some(&altered),
                &f.authority,
                f.now
            )
            .err(),
            Some(CapabilityRejection::AuthorityMismatch)
        );
    }
    let mut authority = f.authority.clone();
    authority.host_id = "90000000-0000-4000-8000-000000000001".parse()?;
    assert_eq!(
        caps.evaluate(
            &f.identity,
            &f.requirement,
            Some(&f.probe),
            &authority,
            f.now
        )
        .err(),
        Some(CapabilityRejection::AuthorityMismatch)
    );
    for (field, value) in [
        ("runtime_kind", "native_process"),
        ("execution_os", "macos"),
        ("architecture", "x64"),
    ] {
        let mut wrong = report();
        wrong[field] = value.into();
        assert_eq!(
            evaluate(wrong, &f)?.err(),
            Some(CapabilityRejection::IdentityMismatch)
        );
    }
    Ok(())
}

#[test]
fn reported_support_does_not_grant_resources_trust_or_missing_images() -> Result<()> {
    let mut f = fixture()?;
    for readiness in ["ready", "preparable", "unavailable", "unknown"] {
        let mut value = report();
        value["environments"][0]["readiness"] = readiness.into();
        let result = evaluate(value, &f)?;
        assert_eq!(result.is_ok(), matches!(readiness, "ready" | "preparable"));
        if let Ok(compatible) = result {
            assert_eq!(
                compatible.environment_readiness(),
                if readiness == "ready" {
                    EnvironmentReadiness::Ready
                } else {
                    EnvironmentReadiness::Preparable
                }
            );
        }
    }
    let mut value = report();
    value["environments"][0]["reference"] = "other:release".into();
    assert_eq!(
        evaluate(value, &f)?.err(),
        Some(CapabilityRejection::ImageUnavailable)
    );
    let mut value = report();
    value["supported_runners"] = json!([]);
    assert_eq!(
        evaluate(value, &f)?.err(),
        Some(CapabilityRejection::UnsupportedMode)
    );
    for resource in ["cpu", "memory", "disk"] {
        let mut value = report();
        value["resources"][resource] = "unsupported".into();
        assert_eq!(
            evaluate(value, &f)?.err(),
            Some(CapabilityRejection::ResourceUnsupported)
        );
    }
    f.requirement.resources = ResourceRequest::new(1000, 1024, 0)?;
    let mut value = report();
    value["resources"] = json!({"cpu":"reservation","memory":"reservation","disk":"unsupported"});
    assert!(evaluate(value, &f)?.is_ok());
    f.requirement.requires_interactive = true;
    assert_eq!(
        evaluate(report(), &f)?.err(),
        Some(CapabilityRejection::InteractiveUnsupported)
    );
    let mut value = report();
    value["interactive"] = "requires_authorized_helper".into();
    assert!(evaluate(value.clone(), &f)?.is_ok());
    f.requirement.requires_docker_socket = true;
    assert_eq!(
        evaluate(value.clone(), &f)?.err(),
        Some(CapabilityRejection::DockerSocketUnsupported)
    );
    value["docker_socket"] = "requires_explicit_grant".into();
    assert!(evaluate(value, &f)?.is_ok());
    Ok(())
}

#[test]
fn compatibility_ceilings_apply_even_when_agent_claims_support() -> Result<()> {
    let f = fixture()?;
    let caps: BackendCapabilities = serde_json::from_value(report())?;
    for (axis, reason) in [
        (0, CapabilityRejection::IncompatibleRuntime),
        (1, CapabilityRejection::IncompatibleOs),
        (2, CapabilityRejection::IncompatibleArchitecture),
    ] {
        let mut requirement = f.requirement.clone();
        match axis {
            0 => requirement.runtime_kind = RuntimeKind::NativeProcess,
            1 => requirement.execution_os = ExecutionOs::Windows,
            _ => requirement.architecture = Architecture::X64,
        }
        assert_eq!(
            caps.evaluate(
                &f.identity,
                &requirement,
                Some(&f.probe),
                &f.authority,
                f.now
            )
            .err(),
            Some(reason)
        );
    }
    for target in [
        TargetKind::GithubRepository,
        TargetKind::GithubOrganization,
        TargetKind::BitbucketWorkspace,
        TargetKind::BitbucketRepository,
    ] {
        for (runtime, os, arch, allowed) in [
            (
                RuntimeKind::Docker,
                ExecutionOs::Linux,
                Architecture::X64,
                true,
            ),
            (
                RuntimeKind::Docker,
                ExecutionOs::Linux,
                Architecture::Arm64,
                true,
            ),
            (
                RuntimeKind::Docker,
                ExecutionOs::Windows,
                Architecture::X64,
                false,
            ),
            (
                RuntimeKind::TartVm,
                ExecutionOs::Macos,
                Architecture::Arm64,
                true,
            ),
            (
                RuntimeKind::TartVm,
                ExecutionOs::Macos,
                Architecture::X64,
                false,
            ),
            (
                RuntimeKind::TartVm,
                ExecutionOs::Linux,
                Architecture::Arm64,
                false,
            ),
            (
                RuntimeKind::NativeProcess,
                ExecutionOs::Windows,
                Architecture::Arm64,
                !target.is_bitbucket(),
            ),
            (
                RuntimeKind::NativeProcess,
                ExecutionOs::Windows,
                Architecture::X64,
                true,
            ),
            (
                RuntimeKind::NativeProcess,
                ExecutionOs::Macos,
                Architecture::X64,
                true,
            ),
        ] {
            let mut requirement = f.requirement.clone();
            requirement.runtime_kind = runtime;
            requirement.execution_os = os;
            requirement.architecture = arch;
            requirement.runner = SupportedRunner {
                target_kind: target,
                mode: if target.is_bitbucket() {
                    RunnerMode::Persistent
                } else {
                    RunnerMode::Ephemeral
                },
            };
            assert_eq!(
                static_compatibility(&requirement).is_ok(),
                allowed,
                "{target:?}/{runtime:?}/{os:?}/{arch:?}"
            );
        }
    }
    let mut requirement = f.requirement.clone();
    requirement.runner.target_kind = TargetKind::BitbucketRepository;
    assert_eq!(
        static_compatibility(&requirement).err(),
        Some(CapabilityRejection::UnsupportedMode)
    );
    requirement.runner.target_kind = TargetKind::GithubRepository;
    requirement.runner.mode = RunnerMode::Persistent;
    requirement.runtime_kind = RuntimeKind::TartVm;
    requirement.execution_os = ExecutionOs::Macos;
    assert_eq!(
        static_compatibility(&requirement).err(),
        Some(CapabilityRejection::UnsupportedMode)
    );
    Ok(())
}
