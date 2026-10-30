//! Drives the Docker runtime against a stand-in `docker` CLI, to pin down
//! what it reports about a runner when Docker itself can't answer.

use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, process::Command};

use anyhow::{Result, ensure};
use gridops_core::fleet::{
    Architecture, CiTarget, ExecutionOs, ExternalId, ResourceRequest, RuntimeKind,
    ids::{BootstrapReference, PreparedEnvironmentId, ProfileId, Revision},
    protocol::{
        agent::{
            AgentOperation, ExecutionConfiguration, PrepareEnvironment, PreparedWorkload,
            RunnerMode, StartRunner,
        },
        outcomes::FailureCode,
        primitives::{ImageReference, WireTimestamp},
    },
};
use gridops_host_agent::{
    docker::DockerRuntime,
    helper::{Bootstrap, HelperRequest, HelperResponse, RuntimeHelper},
};
use serde_json::json;

/// Answers `docker inspect` as Docker would in the state named by the
/// `state` file beside it; `$3` is the `-f` template, if any.
const STAND_IN: &str = r#"#!/bin/sh
case "$(cat "$(dirname "$0")/state")" in
  running) running=true ;;
  exited) running=false ;;
  gone) echo 'Error: No such object: runner' >&2; exit 1 ;;
  *) echo 'Cannot connect to the Docker daemon. Is the docker daemon running?' >&2; exit 1 ;;
esac
case "$3" in
  *State.Running*Labels*) echo "$running environment" ;;
  *State.Running*) echo "$running" ;;
  *Labels*) echo environment ;;
  *) echo '[{}]' ;;
esac
"#;

#[tokio::test]
async fn a_runner_is_reported_gone_only_when_docker_says_so() -> Result<()> {
    let Some(directory) = std::env::var_os("STAND_IN_DOCKER") else {
        // The runtime runs `docker` from PATH, which a test can't change for
        // its own process, so it reruns itself with the stand-in first.
        let directory =
            std::env::temp_dir().join(format!("gridops-docker-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory)?;
        let script = directory.join("docker");
        fs::write(&script, STAND_IN)?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        let status = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "a_runner_is_reported_gone_only_when_docker_says_so",
            ])
            .env(
                "PATH",
                format!("{}:{}", directory.display(), std::env::var("PATH")?),
            )
            .env("STAND_IN_DOCKER", &directory)
            .status()?;
        fs::remove_dir_all(&directory)?;
        ensure!(status.success(), "the run with the stand-in Docker failed");
        return Ok(());
    };
    let directory = Path::new(&directory);
    let runtime = DockerRuntime::new(None, None);
    let resource = serde_json::from_value(json!({"kind": "docker", "object_id": "runner"}))?;
    let request = |operation| HelperRequest {
        environment_id: None,
        docker_socket: false,
        prepared: None,
        bootstrap: None,
        operation,
    };
    let failed = |code, retryable| Some(HelperResponse::Failed { code, retryable });
    for (state, inspect, cleanup) in [
        ("running", None, None),
        (
            "exited",
            failed(FailureCode::ResourceUnavailable, false),
            None,
        ),
        (
            "gone",
            failed(FailureCode::ResourceUnavailable, false),
            None,
        ),
        // Busy or restarting Docker: neither an exit nor proof of absence.
        (
            "unreachable",
            failed(FailureCode::Runtime, true),
            failed(FailureCode::Runtime, true),
        ),
    ] {
        fs::write(directory.join("state"), state)?;
        let inspected = runtime
            .call(request(AgentOperation::Inspect {
                expected_resource: Clone::clone(&resource),
            }))
            .await;
        match &inspect {
            None => ensure!(
                matches!(inspected, HelperResponse::Succeeded { .. }),
                "{state}: {inspected:?}"
            ),
            Some(expected) => ensure!(&inspected == expected, "{state}: {inspected:?}"),
        }
        if state == "gone" || state == "unreachable" {
            let cleaned = runtime
                .call(request(AgentOperation::Cleanup {
                    expected_resource: Clone::clone(&resource),
                }))
                .await;
            match &cleanup {
                None => ensure!(
                    matches!(cleaned, HelperResponse::Succeeded { .. }),
                    "{state} cleanup: {cleaned:?}"
                ),
                Some(expected) => ensure!(&cleaned == expected, "{state} cleanup: {cleaned:?}"),
            }
        }
    }
    Ok(())
}

/// Records every `docker create` invocation's arguments to a file beside
/// the stand-in, and answers `start`/`inspect` as a container that came up
/// cleanly, so `start()` runs to completion against it.
const CREATE_RECORDING_STAND_IN: &str = r#"#!/bin/sh
case "$1" in
  inspect)
    case "$3" in
      *Id*) echo 'Error: No such object: runner' >&2; exit 1 ;;
      *State.Running*) echo true ;;
      *State.ExitCode*) echo 0 ;;
      *) exit 1 ;;
    esac
    ;;
  create)
    shift
    echo "$@" >> "$(dirname "$0")/create-args"
    echo fake-container-id
    ;;
  start)
    cat >/dev/null
    ;;
  *) exit 1 ;;
esac
"#;

/// Guards against regressing any of the Docker runtime adjustments a GitHub
/// Actions runner image needs: `cap-drop ALL` alone leaves root unable to
/// `chown`/`chmod` as setup actions do, the image's own entrypoint (if any)
/// must be bypassed rather than left to run its env-var-driven registration
/// flow, that bypass must not assume `dumb-init` (GitHub's own image doesn't
/// ship it, so `--init` reaps zombies instead), and ephemeral containers
/// must invoke the run script relative to the image's own working directory,
/// not a `myoung34`-specific absolute path GitHub's own runner image lacks.
#[tokio::test]
async fn a_runner_container_is_created_with_the_flags_its_image_needs() -> Result<()> {
    let Some(_directory) = std::env::var_os("STAND_IN_DOCKER_CREATE") else {
        let directory =
            std::env::temp_dir().join(format!("gridops-docker-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory)?;
        let script = directory.join("docker");
        fs::write(&script, CREATE_RECORDING_STAND_IN)?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        let status = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "a_runner_container_is_created_with_the_flags_its_image_needs",
            ])
            .env(
                "PATH",
                format!("{}:{}", directory.display(), std::env::var("PATH")?),
            )
            .env("STAND_IN_DOCKER_CREATE", &directory)
            .status()?;
        let recorded = fs::read_to_string(directory.join("create-args")).unwrap_or_default();
        fs::remove_dir_all(&directory)?;
        ensure!(status.success(), "the run with the stand-in Docker failed");
        ensure!(
            recorded.contains("--cap-add CHOWN")
                && recorded.contains("--cap-add DAC_OVERRIDE")
                && recorded.contains("--cap-add FOWNER")
                && recorded.contains("--cap-add SETUID")
                && recorded.contains("--cap-add SETGID"),
            "missing a capability restored on top of --cap-drop ALL: {recorded}"
        );
        ensure!(
            recorded.contains("--entrypoint ")
                && !recorded.contains("--entrypoint /usr/bin/dumb-init"),
            "the image's own entrypoint must be bypassed without assuming dumb-init: {recorded}"
        );
        ensure!(
            recorded.contains("--init"),
            "Docker's own init must reap the job's orphaned processes: {recorded}"
        );
        ensure!(
            recorded.contains("./run.sh") && !recorded.contains("/home/runner/run.sh"),
            "ephemeral mode must invoke run.sh relative to the image's own working \
             directory, not myoung34's absolute path: {recorded}"
        );
        return Ok(());
    };
    let runtime = DockerRuntime::new(None, None);
    let prepared_environment_id = PreparedEnvironmentId::try_new(uuid::Uuid::new_v4())?;
    let execution_profile_id = ProfileId::try_new(uuid::Uuid::new_v4())?;
    let profile_revision = Revision::new(1)?;
    let prepared = PrepareEnvironment {
        execution_profile_id,
        profile_revision,
        workload: PreparedWorkload::CiRunner,
        target: CiTarget::GithubRepository {
            installation_id: ExternalId::new(1)?,
            repository_id: ExternalId::new(1)?,
        },
        runtime_kind: RuntimeKind::Docker,
        execution_os: ExecutionOs::Linux,
        architecture: Architecture::X64,
        mode: RunnerMode::Ephemeral,
        image: ImageReference::try_from("ghcr.io/actions/actions-runner:latest".to_owned())?,
        labels: Vec::new(),
        configuration: ExecutionConfiguration::default(),
        resource_request: ResourceRequest::new(1_000, 1_024, 0)?,
        allocation_ids: Vec::new(),
    };
    let start = StartRunner {
        prepared_environment_id,
        execution_profile_id,
        profile_revision,
        bootstrap_reference: BootstrapReference::try_new(uuid::Uuid::new_v4())?,
        bootstrap_expires_at: WireTimestamp::from_millis(gridops_core::now_millis())?,
        resource_request: prepared.resource_request,
        allocation_ids: Vec::new(),
    };
    let request = HelperRequest {
        environment_id: Some(prepared_environment_id),
        docker_socket: false,
        prepared: Some(prepared),
        bootstrap: Some(Bootstrap(
            json!({"kind": "github_jit", "jit_config": "jit-token"}).to_string(),
        )),
        operation: AgentOperation::StartRunner(start),
    };
    let response = runtime.call(request).await;
    ensure!(
        matches!(response, HelperResponse::Succeeded { .. }),
        "{response:?}"
    );
    Ok(())
}
