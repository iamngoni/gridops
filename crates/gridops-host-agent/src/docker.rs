//! Linux Docker execution through the local Docker CLI.
//!
//! Runs GitHub Actions runner containers for fleet placements. Containers are
//! named and labelled after their prepared environment, so every later
//! operation targets exactly the object this agent created. `GridOps` chooses
//! the image: each platform names it. The runner's registration material is
//! written to the container's stdin and never appears in its environment,
//! arguments or `docker inspect`.

use std::{process::Stdio, time::Duration};

use gridops_core::{
    fleet::{
        domain::{Architecture, RuntimeKind},
        ids::{DiagnosticId, UpstreamText},
        protocol::{
            agent::{
                AgentOperation, LocalResourceIdentity, PrepareEnvironment, RunnerMode, StopMode,
            },
            inventory::ObservedBackend,
            outcomes::{
                DelayedStartExclusionClaim, DiagnosticCode, DiagnosticReference, FailureCode,
                LocalAbsenceClaim, SuccessEvidence,
            },
            primitives::WireTimestamp,
        },
    },
    provisioning::instrumented_runner_script,
};
use serde::Deserialize;
use tokio::{io::AsyncWriteExt as _, process::Command};
use uuid::Uuid;

use crate::helper::{
    Bootstrap, ChildOutput, HelperRequest, HelperResponse, RunnerOutput, RuntimeHelper,
    StartRemains,
};

const LABEL: &str = "io.gridops.fleet.environment";
/// How many times, 3 s apart, an interrupted start waits for Docker to answer.
const DOCKER_WAIT_ATTEMPTS: u32 = 20;

pub struct DockerRuntime {
    network: Option<String>,
    socket: Option<String>,
    /// The last probe failure, so a change is logged once, not every probe.
    last_problem: std::sync::Mutex<Option<String>>,
}

/// Registration material produced by the reconciler.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Material {
    GithubJit {
        jit_config: String,
    },
    GithubRegistration {
        token: String,
        url: String,
        name: String,
        labels: Vec<String>,
        group: Option<String>,
    },
}

impl DockerRuntime {
    /// `socket` is the host Docker socket path to share when a trusted
    /// profile asks for it.
    pub fn new(network: Option<String>, socket: Option<String>) -> Self {
        Self {
            network,
            socket,
            last_problem: std::sync::Mutex::new(None),
        }
    }

    async fn prepare(
        &self,
        request: &HelperRequest,
        prepare: &PrepareEnvironment,
    ) -> HelperResponse {
        let Some(prepared_environment_id) = request.environment_id else {
            return failed(FailureCode::Configuration, false);
        };
        if prepare.runtime_kind != RuntimeKind::Docker {
            return failed(FailureCode::Unsupported, false);
        }
        let image = String::from(prepare.image.clone());
        // Pull every time, so a tag that moved (`latest`, a rebuilt CI image)
        // is picked up. The copy already here serves if the registry can't be
        // reached.
        if docker(&["pull", &image]).await.is_err()
            && docker(&["image", "inspect", &image]).await.is_err()
        {
            return failed(FailureCode::Dependency, true);
        }
        succeeded(SuccessEvidence::PreparedEnvironment {
            prepared_environment_id,
        })
    }

    async fn start(&self, request: &HelperRequest) -> HelperResponse {
        let (
            Some(environment),
            Some(prepared),
            Some(Bootstrap(material)),
            AgentOperation::StartRunner(start),
        ) = (
            request.environment_id,
            request.prepared.as_ref(),
            request.bootstrap.as_ref(),
            &request.operation,
        )
        else {
            return failed(FailureCode::Configuration, false);
        };
        let Ok(material) = serde_json::from_str::<Material>(material) else {
            return failed(FailureCode::Configuration, false);
        };
        let name = format!("gridops-{environment}");
        // A retried start finds the container it already created.
        if let Ok(id) = docker(&["inspect", "-f", "{{.Id}}", &name]).await {
            return started(start, id);
        }
        let (script, secret, mut env) = match (&material, prepared.mode) {
            (Material::GithubJit { jit_config }, RunnerMode::Ephemeral) => (
                instrumented_runner_script(
                    "set -euo pipefail\nIFS= read -r S\n[ -n \"$S\" ]\nrunner_args=(--jitconfig \"$S\")\nunset S",
                    "./run.sh \"${runner_args[@]}\"",
                ),
                jit_config.clone(),
                Vec::new(),
            ),
            (
                Material::GithubRegistration {
                    token,
                    url,
                    name,
                    labels,
                    group,
                },
                RunnerMode::Persistent,
            ) => (
                instrumented_runner_script(
                    "set -euo pipefail\nIFS= read -r S\nargs=(--unattended --url \"$GRIDOPS_REGISTRATION_URL\" --token \"$S\" --name \"$GRIDOPS_RUNNER_NAME\" --labels \"$GRIDOPS_RUNNER_LABELS\" --work _work --replace --disableupdate)\nif [ -n \"${GRIDOPS_RUNNER_GROUP:-}\" ]; then args+=(--runnergroup \"$GRIDOPS_RUNNER_GROUP\"); fi\n./config.sh \"${args[@]}\"\nunset S",
                    "./run.sh",
                ),
                token.clone(),
                vec![
                    format!("GRIDOPS_REGISTRATION_URL={url}"),
                    format!("GRIDOPS_RUNNER_NAME={name}"),
                    format!("GRIDOPS_RUNNER_LABELS={}", labels.join(",")),
                    format!("GRIDOPS_RUNNER_GROUP={}", group.clone().unwrap_or_default()),
                ],
            ),
            _ => return failed(FailureCode::Configuration, false),
        };
        // We bypass the runner image's own entrypoint below (which otherwise exports
        // this itself) because we run its registration/run scripts as root.
        env.push("RUNNER_ALLOW_RUNASROOT=1".into());
        let resources = &prepared.resource_request;
        let mut args = vec![
            "create".to_owned(),
            "-i".into(),
            "--name".into(),
            name.clone(),
            "--label".into(),
            format!("{LABEL}={environment}"),
            "--cpus".into(),
            format!("{:.3}", f64::from(resources.cpu_millis()) / 1000.0),
            "--memory".into(),
            format!("{}m", resources.memory_mib()),
            "--pids-limit".into(),
            "4096".into(),
            "--cap-drop".into(),
            "ALL".into(),
            // Dropping ALL also strips root's usual exemption from capability checks, so
            // chown/chmod calls the runner's own setup actions make (e.g. actions/setup-node)
            // fail even as uid 0 without these restored explicitly.
            "--cap-add".into(),
            "CHOWN".into(),
            "--cap-add".into(),
            "DAC_OVERRIDE".into(),
            "--cap-add".into(),
            "FOWNER".into(),
            "--cap-add".into(),
            "SETUID".into(),
            "--cap-add".into(),
            "SETGID".into(),
            "--security-opt".into(),
            "no-new-privileges:true".into(),
            "--log-opt".into(),
            "max-size=20m".into(),
            // An empty entrypoint runs the Cmd below as given. myoung34/github-runner's
            // `/entrypoint.sh` would otherwise take it as arguments and first run a
            // registration from env vars GridOps never sets (bootstrap secrets are piped
            // over stdin instead), which fails before our Cmd runs. GitHub's own image has
            // no entrypoint, so this changes nothing there.
            "--entrypoint".into(),
            "".into(),
            // Docker's own init reaps the job's orphaned processes, whatever the image
            // ships. dumb-init would do the same job, but GitHub's own image doesn't carry
            // that binary, so PID 1 duty falls to Docker's --init instead.
            "--init".into(),
        ];
        if let Some(network) = prepared
            .configuration
            .network
            .as_ref()
            .map(|network| network.as_str().to_owned())
            .or_else(|| self.network.clone())
        {
            args.extend(["--network".into(), network]);
        }
        if request.docker_socket {
            let Some(socket) = &self.socket else {
                return failed(FailureCode::Permission, false);
            };
            args.extend(["-v".into(), format!("{socket}:/var/run/docker.sock")]);
        }
        for variable in env {
            args.extend(["-e".into(), variable]);
        }
        args.extend([
            String::from(prepared.image.clone()),
            "/bin/bash".into(),
            "-lc".into(),
            script,
        ]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let Ok(id) = docker(&args).await else {
            return failed(FailureCode::Runtime, true);
        };
        if feed_secret(&id, &secret).await.is_err() {
            let _ = docker(&["rm", "-f", &id]).await;
            return failed(FailureCode::Runtime, true);
        }
        started(start, id)
    }

    async fn owned(&self, resource: &LocalResourceIdentity) -> Option<String> {
        let LocalResourceIdentity::Docker { object_id } = resource else {
            return None;
        };
        let id = object_id.as_str();
        let label = docker(&[
            "inspect",
            "-f",
            &format!("{{{{index .Config.Labels \"{LABEL}\"}}}}"),
            id,
        ])
        .await
        .ok()?;
        (!label.is_empty()).then(|| id.to_owned())
    }
}

impl DockerRuntime {
    /// Why Docker can't be offered on this host, if it can't.
    pub async fn problem(&self) -> Option<String> {
        docker_backend(self)
            .await
            .err()
            .map(|error| format!("{error:#}"))
    }
}

impl RuntimeHelper for DockerRuntime {
    async fn call(&self, request: HelperRequest) -> HelperResponse {
        match &request.operation {
            AgentOperation::PrepareEnvironment(prepare) => self.prepare(&request, prepare).await,
            AgentOperation::StartRunner(_) => self.start(&request).await,
            AgentOperation::Inspect { expected_resource } => {
                let LocalResourceIdentity::Docker { object_id } = expected_resource else {
                    return failed(FailureCode::Unsupported, false);
                };
                let state = docker(&[
                    "inspect",
                    "-f",
                    &format!("{{{{.State.Running}}}} {{{{index .Config.Labels \"{LABEL}\"}}}}"),
                    object_id.as_str(),
                ])
                .await;
                // A stopped or missing runner is reported as unavailable, which
                // tells the reconciler to retire it and clean it up. Docker
                // failing to answer says nothing about the runner, so it must
                // not read as an exit: that would kill a runner mid-job.
                match state {
                    // Running, and labelled as this agent's environment.
                    Ok(state)
                        if state.split_once(' ').is_some_and(|(running, label)| {
                            running == "true" && !label.is_empty()
                        }) =>
                    {
                        succeeded(SuccessEvidence::Inspected {
                            local_identity: Some(expected_resource.clone()),
                        })
                    }
                    Ok(_) => failed(FailureCode::ResourceUnavailable, false),
                    Err(error) if missing(&error) => {
                        failed(FailureCode::ResourceUnavailable, false)
                    }
                    Err(_) => failed(FailureCode::Runtime, true),
                }
            }
            AgentOperation::Stop {
                expected_resource,
                mode,
            } => {
                if let Some(id) = self.owned(expected_resource).await {
                    let result = match mode {
                        StopMode::IdleOnly => docker(&["stop", "-t", "60", &id]).await,
                        StopMode::CancelBusy => docker(&["kill", &id]).await,
                    };
                    if result.is_err() {
                        return failed(FailureCode::Runtime, true);
                    }
                }
                succeeded(SuccessEvidence::Stopped {
                    local_identity: expected_resource.clone(),
                })
            }
            AgentOperation::Cleanup { expected_resource } => {
                if let Some(id) = self.owned(expected_resource).await {
                    if docker(&["rm", "-f", &id]).await.is_err() {
                        return failed(FailureCode::Runtime, true);
                    }
                }
                let LocalResourceIdentity::Docker { object_id } = expected_resource else {
                    return failed(FailureCode::Unsupported, false);
                };
                match docker(&["inspect", object_id.as_str()]).await {
                    Err(error) if missing(&error) => cleaned(expected_resource),
                    _ => failed(FailureCode::Runtime, true),
                }
            }
            AgentOperation::InspectHost => succeeded(SuccessEvidence::Inspected {
                local_identity: None,
            }),
            _ => failed(FailureCode::Unsupported, false),
        }
    }

    async fn interrupted_start(&self, request: HelperRequest, _rebooted: bool) -> StartRemains {
        let Some(environment) = request.environment_id else {
            return StartRemains::Unknown;
        };
        let name = format!("gridops-{environment}");
        // A container outlives a reboot, but Docker may still be starting
        // after one, and until it answers nothing can be said.
        for _ in 0..DOCKER_WAIT_ATTEMPTS {
            match docker(&["inspect", "-f", "{{.Id}}", &name]).await {
                Ok(id) => {
                    return UpstreamText::new(id).map_or(StartRemains::Unknown, |object_id| {
                        StartRemains::Runner(LocalResourceIdentity::Docker { object_id })
                    });
                }
                Err(error) if missing(&error) => return StartRemains::Nothing,
                Err(_) => tokio::time::sleep(Duration::from_secs(3)).await,
            }
        }
        StartRemains::Unknown
    }

    /// `docker logs --follow` of a container this agent created, with
    /// Docker's timestamps, as the manager's live logs have them. Only stdout:
    /// it's where the job's console pages are mirrored, and one stream keeps
    /// the output identical every time it's read.
    async fn output(&self, resource: LocalResourceIdentity) -> Option<RunnerOutput> {
        let id = self.owned(&resource).await?;
        ChildOutput::spawn(Command::new("docker").args(["logs", "--follow", "--timestamps", &id]))
    }

    async fn backends(&self) -> Vec<ObservedBackend> {
        let result = docker_backend(self).await;
        let problem = result.as_ref().err().map(|error| format!("{error:#}"));
        if let Ok(mut last) = self.last_problem.lock() {
            if *last != problem {
                match &problem {
                    Some(problem) => tracing::warn!("Docker jobs unavailable: {problem}"),
                    None if last.is_some() => tracing::info!("Docker is available again"),
                    None => {}
                }
                last.clone_from(&problem);
            }
        }
        match result {
            Ok(backend) => vec![backend],
            // Reported as unavailable, so GridOps shows Docker and why it
            // can't be used rather than nothing at all.
            Err(_) => unavailable_backend(self).into_iter().collect(),
        }
    }
}

fn unavailable_backend(runtime: &DockerRuntime) -> Option<ObservedBackend> {
    let arch = if crate::probe::architecture() == Architecture::Arm64 {
        "arm64"
    } else {
        "x64"
    };
    serde_json::from_value(serde_json::json!({
        "local_key": "docker",
        "known_id": null,
        "domain_local_key": crate::probe::ROOT_DOMAIN,
        "runtime_kind": "docker",
        "execution_os": "linux",
        "architecture": arch,
        "readiness": "unavailable",
        "reason": "docker_unreachable",
        "runtime_identity": null,
        "capabilities": {
            "schema_version": 1,
            "runtime_kind": "docker",
            "execution_os": "linux",
            "architecture": arch,
            "supported_runners": [],
            "environments": [],
            "resources": {"cpu": "hard_limit", "memory": "hard_limit", "disk": "estimate"},
            "interactive": "unsupported",
            "docker_socket": if runtime.socket.is_some() { "requires_explicit_grant" } else { "unsupported" },
        }
    }))
    .ok()
}

async fn docker_backend(runtime: &DockerRuntime) -> anyhow::Result<ObservedBackend> {
    let platform = docker(&["version", "--format", "{{.Server.Os}}/{{.Server.Arch}}"]).await?;
    let architecture = match platform.as_str() {
        "linux/arm64" => Architecture::Arm64,
        "linux/amd64" => Architecture::X64,
        other => anyhow::bail!(
            "Docker reports {other:?}; GridOps runs Linux containers, so switch Docker Desktop to Linux containers"
        ),
    };
    let engine = docker(&["info", "--format", "{{.ID}}"]).await?;
    let arch = if architecture == Architecture::Arm64 {
        "arm64"
    } else {
        "x64"
    };
    Ok(serde_json::from_value(serde_json::json!({
        "local_key": "docker",
        "known_id": null,
        "domain_local_key": crate::probe::ROOT_DOMAIN,
        "runtime_kind": "docker",
        "execution_os": "linux",
        "architecture": arch,
        "readiness": "ready",
        "reason": null,
        "runtime_identity": format!("docker-{engine}"),
        "capabilities": {
            "schema_version": 1,
            "runtime_kind": "docker",
            "execution_os": "linux",
            "architecture": arch,
            "supported_runners": [
                {"target_kind": "github_repository", "mode": "ephemeral"},
                {"target_kind": "github_repository", "mode": "persistent"},
                {"target_kind": "github_organization", "mode": "ephemeral"},
                {"target_kind": "github_organization", "mode": "persistent"}
            ],
            "environments": [],
            // Preparing pulls whatever image the platform names.
            "any_image": true,
            "resources": {"cpu": "hard_limit", "memory": "hard_limit", "disk": "estimate"},
            "interactive": "unsupported",
            "docker_socket": if runtime.socket.is_some() { "requires_explicit_grant" } else { "unsupported" },
        }
    }))?)
}

/// Starts the created container with its stdin attached, writes the secret
/// line, and detaches once the container is running.
async fn feed_secret(id: &str, secret: &str) -> anyhow::Result<()> {
    let mut child = Command::new("docker")
        .args(["start", "--interactive", id])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    stdin.write_all(format!("{secret}\n").as_bytes()).await?;
    stdin.flush().await?;
    for _ in 0..50 {
        if docker(&["inspect", "-f", "{{.State.Running}}", id])
            .await
            .is_ok_and(|state| state == "true")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Give the script a moment to read the line before detaching.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;
    let running = docker(&["inspect", "-f", "{{.State.Running}}", id]).await?;
    let exit = docker(&["inspect", "-f", "{{.State.ExitCode}}", id]).await?;
    anyhow::ensure!(
        running == "true" || exit == "0",
        "runner container failed to start"
    );
    Ok(())
}

async fn docker(args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::time::timeout(
        Duration::from_mins(10),
        Command::new("docker")
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await?
    .map_err(|error| {
        anyhow::anyhow!(
            "couldn't run `docker` ({error}); is Docker installed and on this user's PATH?"
        )
    })?;
    anyhow::ensure!(
        output.status.success(),
        "`docker {}` failed: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Whether a failed `docker` call failed because the object doesn't exist.
fn missing(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("no such object") || message.contains("no such container")
}

fn started(
    start: &gridops_core::fleet::protocol::agent::StartRunner,
    id: String,
) -> HelperResponse {
    let Ok(object_id) = UpstreamText::new(id) else {
        return failed(FailureCode::Runtime, false);
    };
    succeeded(SuccessEvidence::RunnerStarted {
        prepared_environment_id: start.prepared_environment_id,
        execution_profile_id: start.execution_profile_id,
        profile_revision: start.profile_revision,
        local_identity: LocalResourceIdentity::Docker { object_id },
    })
}

pub(crate) fn cleaned(resource: &LocalResourceIdentity) -> HelperResponse {
    let (Ok(now), Ok(absence), Ok(exclusion)) = (
        WireTimestamp::from_millis(gridops_core::now_millis()),
        DiagnosticId::try_new(Uuid::new_v4()),
        DiagnosticId::try_new(Uuid::new_v4()),
    ) else {
        return failed(FailureCode::Runtime, true);
    };
    succeeded(SuccessEvidence::Cleaned {
        local_identity: resource.clone(),
        local_absence: LocalAbsenceClaim {
            resource: resource.clone(),
            observed_at: now,
            evidence: DiagnosticReference {
                id: absence,
                code: DiagnosticCode::Runtime,
            },
        },
        delayed_start_exclusion: DelayedStartExclusionClaim {
            resource: resource.clone(),
            excluded_at: now,
            evidence: DiagnosticReference {
                id: exclusion,
                code: DiagnosticCode::Runtime,
            },
        },
    })
}

fn succeeded(evidence: SuccessEvidence) -> HelperResponse {
    HelperResponse::Succeeded {
        evidence: Box::new(evidence),
    }
}

const fn failed(code: FailureCode, retryable: bool) -> HelperResponse {
    HelperResponse::Failed { code, retryable }
}
