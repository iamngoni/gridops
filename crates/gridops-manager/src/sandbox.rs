//! Agent sandboxes: secret-free containers that the "Fix with agent" loop drives
//! through repository uploads and command execution.
//!
//! A sandbox holds no credentials. The caller copies the repository in as a tar
//! archive and turns the resulting diff into commits itself, so everything the
//! agent runs here only ever sees the workspace. Sandboxes carry their own
//! labels instead of `io.gridops.managed`: the reconciler deletes unrecorded
//! managed containers as orphans, and the runner routes refuse to touch them.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
};
use bollard::{
    Docker,
    container::LogOutput,
    errors::Error as DockerError,
    exec::{CreateExecOptions, StartExecOptions, StartExecResults},
    models::{
        ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, HostConfig,
        HostConfigLogConfig,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, ListContainersOptionsBuilder, RemoveContainerOptionsBuilder,
        StartContainerOptions, UploadToContainerOptionsBuilder,
    },
};
use futures_util::{Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    LeaseClaim, ManagerAuth, ManagerError, ManagerLimits, ManagerState, ensure_disk_capacity,
    ensure_image, ensure_network, ensure_provisioning_enabled, valid_docker_name,
    validate_capacity_lease,
};

/// Marks a container as an agent sandbox. Capacity accounting counts these
/// alongside runner containers.
pub(super) const SANDBOX_LABEL: &str = "io.gridops.agent-sandbox";
const SANDBOX_ID_LABEL: &str = "io.gridops.sandbox-id";
const SANDBOX_NAME_PREFIX: &str = "gridops-agent-";
/// Capacity leases for sandboxes are reserved under this pool through the
/// ordinary `POST /v1/admissions` route.
const SANDBOX_POOL_ID: &str = "gridops-agent";
const SANDBOX_PROVIDER: &str = "docker";
const SANDBOX_WORKSPACE: &str = "/workspace";
/// Root inside the sandbox, so uploaded root-owned files stay writable. Package
/// managers such as apt still need to change ownership and switch users.
const SANDBOX_USER: &str = "0:0";
const SANDBOX_CAPABILITIES: [&str; 5] = ["CHOWN", "DAC_OVERRIDE", "FOWNER", "SETUID", "SETGID"];
const SANDBOX_MIN_CPU: f64 = 0.25;
const SANDBOX_MAX_CPU: f64 = 64.0;
const SANDBOX_MIN_MEMORY_MB: i64 = 256;
const SANDBOX_MAX_MEMORY_MB: i64 = 262_144;

const EXEC_DEFAULT_TIMEOUT_SECONDS: i64 = 300;
const EXEC_MIN_TIMEOUT_SECONDS: i64 = 1;
const EXEC_MAX_TIMEOUT_SECONDS: i64 = 1_200;
/// Seconds `timeout` waits after SIGTERM before it sends SIGKILL.
const EXEC_KILL_GRACE_SECONDS: u64 = 5;
/// The manager abandons an exec this long after the in-container deadline, in
/// case `timeout` itself is missing or wedged.
const EXEC_BACKSTOP_GRACE: Duration = Duration::from_secs(20);
const EXEC_DEFAULT_MAX_OUTPUT_BYTES: i64 = 65_536;
const EXEC_MIN_MAX_OUTPUT_BYTES: i64 = 1_024;
const EXEC_MAX_MAX_OUTPUT_BYTES: i64 = 1_048_576;
const EXEC_MAX_ARGUMENTS: usize = 4_096;
const EXEC_MAX_COMMAND_BYTES: usize = 262_144;
const EXEC_MAX_ENV_ENTRIES: usize = 128;
const EXEC_MAX_ENV_VALUE_BYTES: usize = 32_768;
const MAX_PATH_BYTES: usize = 4_096;

/// Route ceiling for exec: the longest clamped command plus the backstop.
pub(super) const EXEC_ROUTE_TIMEOUT: Duration = Duration::from_mins(21);
/// Route ceiling for archive uploads.
pub(super) const ARCHIVE_ROUTE_TIMEOUT: Duration = Duration::from_mins(10);
/// Docker answers an upload only after extracting it, so the client's default
/// two-minute request timeout is too short for a large repository.
const ARCHIVE_DOCKER_TIMEOUT: Duration = Duration::from_secs(9 * 60 + 30);
const ARCHIVE_MAX_BYTES: u64 = 1 << 30;

const UPLOAD_OK: u8 = 0;
const UPLOAD_TOO_LARGE: u8 = 1;
const UPLOAD_INTERRUPTED: u8 = 2;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateSandbox {
    sandbox_id: String,
    image: String,
    #[serde(default)]
    pull_image: bool,
    cpu_limit: f64,
    memory_limit_mb: i64,
    network: String,
    capacity_lease: String,
}

impl CreateSandbox {
    fn validate(&self) -> Result<(), ManagerError> {
        validate_sandbox_id(&self.sandbox_id)?;
        if self.image.trim().is_empty()
            || self.image.len() > 512
            || self
                .image
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(ManagerError::BadRequest("Sandbox image is invalid.".into()));
        }
        if !self.cpu_limit.is_finite()
            || !(SANDBOX_MIN_CPU..=SANDBOX_MAX_CPU).contains(&self.cpu_limit)
            || !(SANDBOX_MIN_MEMORY_MB..=SANDBOX_MAX_MEMORY_MB).contains(&self.memory_limit_mb)
        {
            return Err(ManagerError::BadRequest(format!(
                "Sandbox CPU must be between {SANDBOX_MIN_CPU} and {SANDBOX_MAX_CPU} and memory between {SANDBOX_MIN_MEMORY_MB} and {SANDBOX_MAX_MEMORY_MB} MB."
            )));
        }
        if !valid_docker_name(&self.network) {
            return Err(ManagerError::BadRequest(
                "Sandbox network name contains unsupported characters.".into(),
            ));
        }
        if self.capacity_lease.is_empty() || self.capacity_lease.len() > 128 {
            return Err(ManagerError::BadRequest(
                "Sandbox capacity lease identifier is required.".into(),
            ));
        }
        Ok(())
    }

    /// Sandbox leases are reserved with the sandbox id as the runner id.
    fn lease_claim(&self) -> LeaseClaim<'_> {
        LeaseClaim {
            lease_id: &self.capacity_lease,
            runner_id: &self.sandbox_id,
            pool_id: SANDBOX_POOL_ID,
            provider: SANDBOX_PROVIDER,
            cpu_limit: self.cpu_limit,
            memory_limit_mb: self.memory_limit_mb,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SandboxExecRequest {
    command: Vec<String>,
    workdir: Option<String>,
    timeout_seconds: Option<i64>,
    max_output_bytes: Option<i64>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

impl SandboxExecRequest {
    fn plan(&self) -> Result<ExecPlan, ManagerError> {
        validate_command(&self.command)?;
        let workdir = match self.workdir.as_deref() {
            None => SANDBOX_WORKSPACE.to_owned(),
            Some(value) => normalized_absolute_path(value).ok_or_else(|| {
                ManagerError::BadRequest(
                    "Exec working directory must be an absolute path without . or .. segments."
                        .into(),
                )
            })?,
        };
        let env = exec_environment(&self.env)?;
        let timeout_seconds = clamp_timeout_seconds(self.timeout_seconds);
        let mut command = vec![
            "timeout".to_owned(),
            "-k".to_owned(),
            EXEC_KILL_GRACE_SECONDS.to_string(),
            timeout_seconds.to_string(),
        ];
        command.extend(self.command.iter().cloned());
        Ok(ExecPlan {
            command,
            workdir,
            env,
            timeout: Duration::from_secs(timeout_seconds),
            max_output_bytes: clamp_max_output_bytes(self.max_output_bytes),
        })
    }
}

/// A validated exec, with the command already wrapped in its deadline.
#[derive(Debug)]
struct ExecPlan {
    command: Vec<String>,
    workdir: String,
    env: Vec<String>,
    timeout: Duration,
    max_output_bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SandboxExecResponse {
    /// `-1` when Docker never reported one, such as after the manager backstop.
    exit_code: i64,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    timed_out: bool,
    duration_ms: u64,
}

#[derive(Debug, Deserialize)]
pub(super) struct ArchiveQuery {
    path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SandboxSummary {
    id: String,
    sandbox_id: String,
    name: String,
    state: String,
    created_at: String,
}

pub(super) async fn create_sandbox(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
    Json(input): Json<CreateSandbox>,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    input.validate()?;
    let _guard = state.provision_lock.lock().await;
    ensure_provisioning_enabled(&state)?;
    if input.network != state.limits.runner_network {
        return Err(ManagerError::Forbidden(
            "Agent sandboxes may only join the configured GridOps runner network.".into(),
        ));
    }
    validate_capacity_lease(&state, &input.lease_claim()).await?;
    let result = create_sandbox_inner(&state, &input).await;
    state
        .reservations
        .lock()
        .await
        .remove(&input.capacity_lease);
    result
}

async fn create_sandbox_inner(
    state: &ManagerState,
    input: &CreateSandbox,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    ensure_network(&state.docker, &input.network).await?;
    ensure_image(&state.docker, &input.image, input.pull_image).await?;
    ensure_disk_capacity(&state.limits)?;

    let name = sandbox_container_name(&input.sandbox_id);
    match state.docker.inspect_container(&name, None).await {
        Ok(_) => {
            return Err(ManagerError::Conflict(
                "An agent sandbox with this identifier already exists.".into(),
            ));
        }
        Err(DockerError::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(error) => return Err(error.into()),
    }
    let container = state
        .docker
        .create_container(
            Some(CreateContainerOptionsBuilder::default().name(&name).build()),
            sandbox_container_body(input, &state.limits),
        )
        .await?;
    if let Err(error) = state
        .docker
        .start_container(&container.id, None::<StartContainerOptions>)
        .await
    {
        let _ = state
            .docker
            .remove_container(
                &container.id,
                Some(
                    RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(true)
                        .build(),
                ),
            )
            .await;
        return Err(error.into());
    }
    let details = state.docker.inspect_container(&container.id, None).await?;
    let state = container_state(&details);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": details.id.unwrap_or(container.id),
            "sandboxId": input.sandbox_id,
            "name": name,
            "state": state,
        })),
    ))
}

pub(super) async fn list_sandboxes(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
) -> Result<Json<Value>, ManagerError> {
    let filters = HashMap::from([("label".to_owned(), vec![format!("{SANDBOX_LABEL}=true")])]);
    let containers = state
        .docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await?;
    let sandboxes = containers
        .into_iter()
        .filter_map(|container| {
            let labels = container.labels.unwrap_or_default();
            let sandbox_id = labels.get(SANDBOX_ID_LABEL)?.clone();
            if !sandbox_labels_match(&labels, &sandbox_id) {
                return None;
            }
            Some(SandboxSummary {
                id: container.id.unwrap_or_default(),
                name: container
                    .names
                    .unwrap_or_default()
                    .first()
                    .map(|name| name.trim_start_matches('/').to_owned())
                    .unwrap_or_else(|| sandbox_container_name(&sandbox_id)),
                sandbox_id,
                state: container
                    .state
                    .map_or_else(String::new, |value| value.to_string()),
                created_at: chrono::DateTime::from_timestamp(
                    container.created.unwrap_or_default(),
                    0,
                )
                .map_or_else(String::new, |value| value.to_rfc3339()),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "sandboxes": sandboxes })))
}

pub(super) async fn delete_sandbox(
    State(state): State<ManagerState>,
    Path(sandbox_id): Path<String>,
    _auth: ManagerAuth,
) -> Result<StatusCode, ManagerError> {
    validate_sandbox_id(&sandbox_id)?;
    let details = match find_sandbox(&state.docker, &sandbox_id).await {
        Ok(details) => details,
        Err(ManagerError::NotFound(_)) => return Ok(StatusCode::NO_CONTENT),
        Err(error) => return Err(error),
    };
    let container_id = details
        .id
        .unwrap_or_else(|| sandbox_container_name(&sandbox_id));
    match state
        .docker
        .remove_container(
            &container_id,
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(true)
                    .build(),
            ),
        )
        .await
    {
        Ok(())
        | Err(DockerError::DockerResponseServerError {
            status_code: 404, ..
        }) => Ok(StatusCode::NO_CONTENT),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn exec_in_sandbox(
    State(state): State<ManagerState>,
    Path(sandbox_id): Path<String>,
    _auth: ManagerAuth,
    Json(input): Json<SandboxExecRequest>,
) -> Result<Json<SandboxExecResponse>, ManagerError> {
    validate_sandbox_id(&sandbox_id)?;
    let plan = input.plan()?;
    let details = find_sandbox(&state.docker, &sandbox_id).await?;
    if !sandbox_running(&details) {
        return Err(ManagerError::Conflict(
            "Agent sandbox is not running.".into(),
        ));
    }
    let container_id = details
        .id
        .unwrap_or_else(|| sandbox_container_name(&sandbox_id));
    run_exec(&state.docker, &container_id, &plan)
        .await
        .map(Json)
}

pub(super) async fn upload_sandbox_archive(
    State(state): State<ManagerState>,
    Path(sandbox_id): Path<String>,
    Query(query): Query<ArchiveQuery>,
    _auth: ManagerAuth,
    request: Request,
) -> Result<StatusCode, ManagerError> {
    validate_sandbox_id(&sandbox_id)?;
    let path = archive_destination(query.path.as_deref().unwrap_or(SANDBOX_WORKSPACE))?;
    if declared_length_exceeds(request.headers(), ARCHIVE_MAX_BYTES) {
        return Err(archive_too_large());
    }
    let details = find_sandbox(&state.docker, &sandbox_id).await?;
    let container_id = details
        .id
        .clone()
        .unwrap_or_else(|| sandbox_container_name(&sandbox_id));
    // Docker only extracts into an existing directory.
    if sandbox_running(&details) {
        create_directory(&state.docker, &container_id, &path).await?;
    }
    let failure = Arc::new(AtomicU8::new(UPLOAD_OK));
    let body = limited_archive_stream(request.into_body(), ARCHIVE_MAX_BYTES, failure.clone());
    let result = state
        .docker
        .clone()
        .with_timeout(ARCHIVE_DOCKER_TIMEOUT)
        .upload_to_container(
            &container_id,
            Some(
                UploadToContainerOptionsBuilder::default()
                    .path(&path)
                    .build(),
            ),
            bollard::body_try_stream(body),
        )
        .await;
    match failure.load(Ordering::Relaxed) {
        UPLOAD_TOO_LARGE => return Err(archive_too_large()),
        UPLOAD_INTERRUPTED => {
            return Err(ManagerError::BadRequest(
                "The archive upload was interrupted before it completed.".into(),
            ));
        }
        _ => {}
    }
    result.map_err(archive_docker_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_directory(
    docker: &Docker,
    container_id: &str,
    path: &str,
) -> Result<(), ManagerError> {
    let plan = ExecPlan {
        command: vec!["mkdir".into(), "-p".into(), "--".into(), path.to_owned()],
        workdir: "/".into(),
        env: Vec::new(),
        timeout: Duration::from_secs(30),
        max_output_bytes: 4_096,
    };
    let outcome = run_exec(docker, container_id, &plan).await?;
    if outcome.exit_code != 0 {
        return Err(ManagerError::BadRequest(format!(
            "Could not create the upload directory in the agent sandbox: {}",
            outcome.stderr.trim()
        )));
    }
    Ok(())
}

async fn run_exec(
    docker: &Docker,
    container_id: &str,
    plan: &ExecPlan,
) -> Result<SandboxExecResponse, ManagerError> {
    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                attach_stdin: Some(false),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                env: Some(plan.env.clone()),
                cmd: Some(plan.command.clone()),
                working_dir: Some(plan.workdir.clone()),
                ..Default::default()
            },
        )
        .await
        .map_err(sandbox_docker_error)?;
    let started = Instant::now();
    let StartExecResults::Attached { mut output, .. } = docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                tty: false,
                output_capacity: None,
            }),
        )
        .await
        .map_err(sandbox_docker_error)?
    else {
        return Err(ManagerError::Internal(anyhow::anyhow!(
            "sandbox exec started detached"
        )));
    };
    let mut stdout = OutputCapture::new(plan.max_output_bytes);
    let mut stderr = OutputCapture::new(plan.max_output_bytes);
    let collected = tokio::time::timeout(plan.timeout + EXEC_BACKSTOP_GRACE, async {
        while let Some(frame) = output.next().await {
            match frame? {
                LogOutput::StdOut { message } | LogOutput::Console { message } => {
                    stdout.push(&message);
                }
                LogOutput::StdErr { message } => stderr.push(&message),
                LogOutput::StdIn { .. } => {}
            }
        }
        Ok::<(), DockerError>(())
    })
    .await;
    let elapsed = started.elapsed();
    let (exit_code, timed_out) = match collected {
        Ok(Ok(())) => {
            let exit_code = exec_exit_code(docker, &exec.id).await?;
            (exit_code, timed_out_exit(exit_code, elapsed, plan.timeout))
        }
        Ok(Err(error)) => return Err(sandbox_docker_error(error)),
        Err(_) => {
            tracing::warn!(
                container_id,
                timeout_seconds = plan.timeout.as_secs(),
                "sandbox exec outlived its in-container deadline; abandoning it"
            );
            (-1, true)
        }
    };
    let (stdout, stdout_truncated) = stdout.finish();
    let (stderr, stderr_truncated) = stderr.finish();
    Ok(SandboxExecResponse {
        exit_code,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        timed_out,
        duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    })
}

/// The output stream closes as the process exits, but Docker can take a moment
/// to record the exit code.
async fn exec_exit_code(docker: &Docker, exec_id: &str) -> Result<i64, ManagerError> {
    for _ in 0..40 {
        let inspect = docker
            .inspect_exec(exec_id)
            .await
            .map_err(sandbox_docker_error)?;
        if inspect.running != Some(true) {
            return Ok(inspect.exit_code.unwrap_or(-1));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(-1)
}

/// `timeout` exits 124 after SIGTERM and 137 after the follow-up SIGKILL. A
/// command can exit with either code on its own (137 is also an OOM kill), so
/// only codes reported once the deadline has passed count as timeouts.
fn timed_out_exit(exit_code: i64, elapsed: Duration, timeout: Duration) -> bool {
    matches!(exit_code, 124 | 137) && elapsed + Duration::from_millis(500) >= timeout
}

/// Keeps the first quarter and the last three quarters of a byte budget from a
/// stream of unknown length, so memory stays bounded however much is written.
#[derive(Debug)]
struct OutputCapture {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_limit: usize,
    tail_limit: usize,
    total: usize,
}

impl OutputCapture {
    fn new(max_bytes: usize) -> Self {
        let head_limit = max_bytes / 4;
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_limit,
            tail_limit: max_bytes - head_limit,
            total: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len());
        let head_room = self.head_limit.saturating_sub(self.head.len());
        let (head, rest) = bytes.split_at(head_room.min(bytes.len()));
        self.head.extend_from_slice(head);
        if rest.len() >= self.tail_limit {
            self.tail.clear();
            self.tail
                .extend(rest.iter().skip(rest.len() - self.tail_limit));
            return;
        }
        let overflow = (self.tail.len() + rest.len()).saturating_sub(self.tail_limit);
        self.tail.drain(..overflow);
        self.tail.extend(rest);
    }

    /// Returns the captured text and whether anything was dropped.
    fn finish(self) -> (String, bool) {
        let dropped = self.total - self.head.len() - self.tail.len();
        let mut head = self.head;
        let tail = Vec::from(self.tail);
        if dropped == 0 {
            head.extend_from_slice(&tail);
            return (String::from_utf8_lossy(&head).into_owned(), false);
        }
        (
            format!(
                "{}\n…[{dropped} bytes truncated]…\n{}",
                String::from_utf8_lossy(&head),
                String::from_utf8_lossy(&tail)
            ),
            true,
        )
    }
}

/// Builds the sandbox container. It deliberately takes no Docker socket: a
/// sandbox must never be able to control the host engine.
fn sandbox_container_body(input: &CreateSandbox, limits: &ManagerLimits) -> ContainerCreateBody {
    let memory_bytes = input.memory_limit_mb * 1_024 * 1_024;
    ContainerCreateBody {
        image: Some(input.image.clone()),
        entrypoint: Some(vec!["/bin/sh".into(), "-c".into()]),
        cmd: Some(vec![format!(
            "mkdir -p {SANDBOX_WORKSPACE} && exec sleep infinity"
        )]),
        user: Some(SANDBOX_USER.into()),
        working_dir: Some(SANDBOX_WORKSPACE.into()),
        env: Some(Vec::new()),
        labels: Some(sandbox_labels(&input.sandbox_id)),
        attach_stdin: Some(false),
        open_stdin: Some(false),
        tty: Some(false),
        host_config: Some(HostConfig {
            auto_remove: Some(false),
            network_mode: Some(input.network.clone()),
            nano_cpus: Some((input.cpu_limit * 1_000_000_000.0) as i64),
            memory: Some(memory_bytes),
            memory_swap: Some(
                memory_bytes.saturating_add(memory_bytes * limits.runner_swap_percent / 100),
            ),
            pids_limit: Some(limits.runner_pids_limit),
            binds: None,
            mounts: None,
            group_add: None,
            privileged: Some(false),
            // Reaps the orphans that commands leave behind, which `sleep`
            // would otherwise accumulate against the pids limit.
            init: Some(true),
            cap_drop: Some(vec!["ALL".into()]),
            cap_add: Some(SANDBOX_CAPABILITIES.map(String::from).to_vec()),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            oom_score_adj: Some(500),
            log_config: Some(HostConfigLogConfig {
                typ: Some("json-file".into()),
                config: Some(HashMap::from([
                    ("max-size".into(), limits.log_max_size.clone()),
                    ("max-file".into(), limits.log_max_files.to_string()),
                ])),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn sandbox_labels(sandbox_id: &str) -> HashMap<String, String> {
    HashMap::from([
        (SANDBOX_LABEL.into(), "true".into()),
        (SANDBOX_ID_LABEL.into(), sandbox_id.into()),
    ])
}

/// A container is the named sandbox only if it carries both sandbox labels and
/// is not also a managed runner.
fn sandbox_labels_match(labels: &HashMap<String, String>, sandbox_id: &str) -> bool {
    labels.get(SANDBOX_LABEL).map(String::as_str) == Some("true")
        && labels.get(SANDBOX_ID_LABEL).map(String::as_str) == Some(sandbox_id)
        && labels.get("io.gridops.managed").map(String::as_str) != Some("true")
}

fn sandbox_container_name(sandbox_id: &str) -> String {
    format!("{SANDBOX_NAME_PREFIX}{sandbox_id}")
}

async fn find_sandbox(
    docker: &Docker,
    sandbox_id: &str,
) -> Result<ContainerInspectResponse, ManagerError> {
    let details = docker
        .inspect_container(&sandbox_container_name(sandbox_id), None)
        .await
        .map_err(sandbox_docker_error)?;
    let labels = details
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref());
    if !labels.is_some_and(|labels| sandbox_labels_match(labels, sandbox_id)) {
        return Err(ManagerError::Forbidden(
            "GridOps can only control containers carrying its agent-sandbox labels.".into(),
        ));
    }
    Ok(details)
}

fn sandbox_running(details: &ContainerInspectResponse) -> bool {
    details
        .state
        .as_ref()
        .and_then(|state| state.status.as_ref())
        .is_some_and(|status| matches!(status, ContainerStateStatusEnum::RUNNING))
}

fn container_state(details: &ContainerInspectResponse) -> String {
    details
        .state
        .as_ref()
        .and_then(|state| state.status.as_ref())
        .map_or_else(|| "unknown".into(), ToString::to_string)
}

fn sandbox_docker_error(error: DockerError) -> ManagerError {
    match error {
        DockerError::DockerResponseServerError {
            status_code: 404, ..
        } => ManagerError::NotFound("Agent sandbox was not found.".into()),
        error => error.into(),
    }
}

/// The sandbox was found before the upload, so a 404 here means the path.
fn archive_docker_error(error: DockerError) -> ManagerError {
    match error {
        DockerError::DockerResponseServerError {
            status_code: 404, ..
        } => ManagerError::NotFound("The upload path does not exist in the agent sandbox.".into()),
        DockerError::DockerResponseServerError {
            status_code: 400,
            message,
        } => ManagerError::BadRequest(message.chars().take(2_000).collect()),
        DockerError::DockerResponseServerError {
            status_code: 403,
            message,
        } => ManagerError::Forbidden(message.chars().take(2_000).collect()),
        DockerError::DockerResponseServerError {
            status_code,
            message,
        } if status_code >= 500 => ManagerError::Upstream {
            status: StatusCode::BAD_GATEWAY,
            code: "sandbox_archive_failed".into(),
            message: message.chars().take(2_000).collect(),
        },
        DockerError::RequestTimeoutError => ManagerError::Upstream {
            status: StatusCode::GATEWAY_TIMEOUT,
            code: "sandbox_archive_timeout".into(),
            message: "Docker did not finish extracting the archive in time.".into(),
        },
        error => error.into(),
    }
}

fn archive_too_large() -> ManagerError {
    ManagerError::PayloadTooLarge(format!(
        "Sandbox archives may be at most {} MiB.",
        ARCHIVE_MAX_BYTES / 1_024 / 1_024
    ))
}

fn declared_length_exceeds(headers: &HeaderMap, limit: u64) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > limit)
}

/// Streams the request body to Docker without buffering it, failing once it
/// passes `limit`. A raw body bypasses axum's `DefaultBodyLimit`, which only
/// governs buffering extractors, so the limit is enforced here.
fn limited_archive_stream(
    body: Body,
    limit: u64,
    failure: Arc<AtomicU8>,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static {
    let mut received = 0_u64;
    body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(|error| {
            failure.store(UPLOAD_INTERRUPTED, Ordering::Relaxed);
            io::Error::other(error)
        })?;
        received = received.saturating_add(chunk.len() as u64);
        if received > limit {
            failure.store(UPLOAD_TOO_LARGE, Ordering::Relaxed);
            return Err(io::Error::other("sandbox archive exceeds the upload limit"));
        }
        Ok(chunk)
    })
}

fn validate_sandbox_id(id: &str) -> Result<(), ManagerError> {
    if valid_sandbox_id(id) {
        return Ok(());
    }
    Err(ManagerError::BadRequest(
        "Sandbox identifier must be 8 to 64 lowercase letters, digits, or hyphens.".into(),
    ))
}

fn valid_sandbox_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Uploads may only land in the workspace or scratch space.
fn archive_destination(path: &str) -> Result<String, ManagerError> {
    normalized_absolute_path(path)
        .filter(|path| {
            ["/workspace", "/tmp"].iter().any(|root| {
                path == root
                    || path
                        .strip_prefix(root)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
        .ok_or_else(|| {
            ManagerError::BadRequest(
                "Archive path must be an absolute path under /workspace or /tmp without . or .. segments."
                    .into(),
            )
        })
}

/// Collapses repeated and trailing slashes; rejects relative paths, `.` and
/// `..` segments, and control characters.
fn normalized_absolute_path(path: &str) -> Option<String> {
    if !path.starts_with('/') || path.len() > MAX_PATH_BYTES || path.chars().any(char::is_control) {
        return None;
    }
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "." | ".."))
    {
        return None;
    }
    Some(format!("/{}", segments.join("/")))
}

fn validate_command(command: &[String]) -> Result<(), ManagerError> {
    let total = command.iter().map(String::len).sum::<usize>();
    if command
        .first()
        .is_none_or(|program| program.trim().is_empty())
        || command.len() > EXEC_MAX_ARGUMENTS
        || total > EXEC_MAX_COMMAND_BYTES
        || command.iter().any(|argument| argument.contains('\0'))
    {
        return Err(ManagerError::BadRequest(
            "Exec command must be a non-empty argument list without NUL bytes.".into(),
        ));
    }
    Ok(())
}

fn exec_environment(env: &BTreeMap<String, String>) -> Result<Vec<String>, ManagerError> {
    if env.len() > EXEC_MAX_ENV_ENTRIES {
        return Err(ManagerError::BadRequest(format!(
            "Exec environment may set at most {EXEC_MAX_ENV_ENTRIES} variables."
        )));
    }
    env.iter()
        .map(|(name, value)| {
            if !valid_env_name(name)
                || value.len() > EXEC_MAX_ENV_VALUE_BYTES
                || value.contains('\0')
            {
                return Err(ManagerError::BadRequest(format!(
                    "Exec environment variable {name:?} is invalid."
                )));
            }
            Ok(format!("{name}={value}"))
        })
        .collect()
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 256
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn clamp_timeout_seconds(value: Option<i64>) -> u64 {
    let seconds = value
        .unwrap_or(EXEC_DEFAULT_TIMEOUT_SECONDS)
        .clamp(EXEC_MIN_TIMEOUT_SECONDS, EXEC_MAX_TIMEOUT_SECONDS);
    u64::try_from(seconds).unwrap_or(EXEC_MIN_TIMEOUT_SECONDS.unsigned_abs())
}

fn clamp_max_output_bytes(value: Option<i64>) -> usize {
    let bytes = value
        .unwrap_or(EXEC_DEFAULT_MAX_OUTPUT_BYTES)
        .clamp(EXEC_MIN_MAX_OUTPUT_BYTES, EXEC_MAX_MAX_OUTPUT_BYTES);
    usize::try_from(bytes).unwrap_or(1_024)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use anyhow::Context as _;
    use secrecy::SecretString;
    use tokio::sync::Mutex;

    use super::*;
    use crate::{CapacityReservation, SharedDockerSocket, managed_labels};

    const TOKEN: &str = "sandbox-test-token";

    fn limits() -> ManagerLimits {
        ManagerLimits {
            available_cpus: 8.0,
            total_memory_mb: 16_384,
            cpu_budget: 6.0,
            memory_budget_mb: 12_288,
            max_runners: 3,
            min_free_disk_mb: 1_024,
            min_free_disk_percent: 10,
            runner_network: "gridops-runners".into(),
            log_max_size: "20m".into(),
            log_max_files: 5,
            runner_pids_limit: 1_024,
            runner_swap_percent: 100,
        }
    }

    fn create_request() -> CreateSandbox {
        CreateSandbox {
            sandbox_id: "3f2b6c1e-0d4a-4d0e-9a51-7c1f0b2e8d44".into(),
            image: "ghcr.io/actions/actions-runner:latest".into(),
            pull_image: true,
            cpu_limit: 2.0,
            memory_limit_mb: 2_048,
            network: "gridops-runners".into(),
            capacity_lease: "lease-1".into(),
        }
    }

    fn exec_request(command: &[&str]) -> SandboxExecRequest {
        SandboxExecRequest {
            command: command.iter().map(|value| (*value).to_owned()).collect(),
            workdir: None,
            timeout_seconds: None,
            max_output_bytes: None,
            env: BTreeMap::new(),
        }
    }

    fn captured(max_bytes: usize, chunks: &[&[u8]]) -> (String, bool) {
        let mut capture = OutputCapture::new(max_bytes);
        for chunk in chunks {
            capture.push(chunk);
        }
        capture.finish()
    }

    #[test]
    fn accepts_only_lowercase_sandbox_ids() {
        assert!(valid_sandbox_id("3f2b6c1e-0d4a-4d0e-9a51-7c1f0b2e8d44"));
        assert!(valid_sandbox_id("abcd1234"));
        assert!(valid_sandbox_id(&"a".repeat(64)));
        for invalid in [
            "abc123",
            "ABCD1234",
            "abcd_1234",
            "abcd/1234",
            "abcd..1234",
            "abcd 1234",
        ] {
            assert!(!valid_sandbox_id(invalid), "{invalid} should be rejected");
        }
        assert!(!valid_sandbox_id(&"a".repeat(65)));
    }

    #[test]
    fn validates_sandbox_creation_requests() -> Result<(), ManagerError> {
        create_request().validate()?;
        let invalid: [fn(&mut CreateSandbox); 9] = [
            |input| input.sandbox_id = "Bad_ID".into(),
            |input| input.image = " ".into(),
            |input| input.image = "ubuntu latest".into(),
            |input| input.cpu_limit = f64::NAN,
            |input| input.cpu_limit = 0.0,
            |input| input.cpu_limit = 128.0,
            |input| input.memory_limit_mb = 64,
            |input| input.network = "bad network".into(),
            |input| input.capacity_lease = String::new(),
        ];
        for mutate in invalid {
            let mut input = create_request();
            mutate(&mut input);
            assert!(matches!(input.validate(), Err(ManagerError::BadRequest(_))));
        }
        Ok(())
    }

    #[test]
    fn sandbox_leases_match_admissions_reserved_for_the_agent_pool() {
        let input = create_request();
        // What the caller reserves through POST /v1/admissions.
        let reservation = CapacityReservation {
            runner_id: input.sandbox_id.clone(),
            pool_id: "gridops-agent".into(),
            provider: "docker".into(),
            cpu_limit: 2.0,
            memory_limit_mb: 2_048,
            expires_at: Instant::now() + Duration::from_mins(5),
        };
        assert!(input.lease_claim().matches(&reservation));
        let other_pool = CapacityReservation {
            pool_id: "pool-1".into(),
            ..reservation.clone()
        };
        assert!(!input.lease_claim().matches(&other_pool));
        let other_size = CapacityReservation {
            memory_limit_mb: 4_096,
            ..reservation
        };
        assert!(!input.lease_claim().matches(&other_size));
    }

    #[test]
    fn archive_paths_stay_in_workspace_or_tmp() -> Result<(), ManagerError> {
        assert_eq!(archive_destination("/workspace")?, "/workspace");
        assert_eq!(archive_destination("/workspace/repo/")?, "/workspace/repo");
        assert_eq!(archive_destination("//workspace//repo")?, "/workspace/repo");
        assert_eq!(archive_destination("/tmp")?, "/tmp");
        assert_eq!(archive_destination("/tmp/cache")?, "/tmp/cache");
        for invalid in [
            "workspace",
            "/",
            "/etc",
            "/workspacefoo",
            "/tmpfoo/x",
            "/workspace/../etc",
            "/workspace/./repo",
            "/workspace/re\0po",
            "/workspace/re\npo",
        ] {
            assert!(
                matches!(
                    archive_destination(invalid),
                    Err(ManagerError::BadRequest(_))
                ),
                "{invalid:?} should be rejected"
            );
        }
        assert!(
            archive_destination(&format!("/workspace/{}", "a".repeat(MAX_PATH_BYTES))).is_err()
        );
        Ok(())
    }

    #[test]
    fn exec_plans_wrap_commands_in_an_in_container_deadline() -> Result<(), ManagerError> {
        let mut request = exec_request(&["bash", "-lc", "npm test"]);
        request.workdir = Some("/workspace/repo/".into());
        request.timeout_seconds = Some(90);
        request.env = BTreeMap::from([
            ("NODE_ENV".to_owned(), "test".to_owned()),
            ("CI".to_owned(), "true".to_owned()),
        ]);
        let plan = request.plan()?;
        assert_eq!(
            plan.command,
            ["timeout", "-k", "5", "90", "bash", "-lc", "npm test"]
        );
        assert_eq!(plan.workdir, "/workspace/repo");
        assert_eq!(plan.env, ["CI=true", "NODE_ENV=test"]);
        assert_eq!(plan.timeout, Duration::from_secs(90));
        assert_eq!(plan.max_output_bytes, 65_536);
        // The route must outlive the longest exec and its backstop.
        assert!(
            Duration::from_secs(EXEC_MAX_TIMEOUT_SECONDS.unsigned_abs()) + EXEC_BACKSTOP_GRACE
                < EXEC_ROUTE_TIMEOUT,
            "exec route timeout is shorter than the exec backstop"
        );
        Ok(())
    }

    #[test]
    fn exec_plans_clamp_timeouts_and_output_budgets() -> Result<(), ManagerError> {
        let defaults = exec_request(&["true"]).plan()?;
        assert_eq!(defaults.workdir, "/workspace");
        assert_eq!(defaults.timeout, Duration::from_mins(5));
        assert_eq!(defaults.max_output_bytes, 65_536);
        for (requested, expected) in [(0, 1), (-5, 1), (1, 1), (1_200, 1_200), (5_000, 1_200)] {
            let mut request = exec_request(&["true"]);
            request.timeout_seconds = Some(requested);
            let plan = request.plan()?;
            assert_eq!(plan.timeout, Duration::from_secs(expected));
            assert_eq!(plan.command.get(3), Some(&expected.to_string()));
        }
        for (requested, expected) in [
            (0, 1_024),
            (10, 1_024),
            (4_096, 4_096),
            (1_048_576, 1_048_576),
            (10_000_000, 1_048_576),
        ] {
            let mut request = exec_request(&["true"]);
            request.max_output_bytes = Some(requested);
            assert_eq!(request.plan()?.max_output_bytes, expected);
        }
        Ok(())
    }

    #[test]
    fn exec_plans_reject_invalid_commands_workdirs_and_environment() {
        let mut invalid = vec![
            exec_request(&[]),
            exec_request(&[""]),
            exec_request(&["echo", "nul\0byte"]),
        ];
        for workdir in ["relative", "/workspace/../etc", "/workspace/\0"] {
            let mut request = exec_request(&["true"]);
            request.workdir = Some(workdir.into());
            invalid.push(request);
        }
        for name in ["", "1ABC", "BAD-NAME", "BAD=NAME", "WITH SPACE"] {
            let mut request = exec_request(&["true"]);
            request.env = BTreeMap::from([(name.to_owned(), "value".to_owned())]);
            invalid.push(request);
        }
        let mut nul_value = exec_request(&["true"]);
        nul_value.env = BTreeMap::from([("CI".to_owned(), "tr\0ue".to_owned())]);
        invalid.push(nul_value);
        for request in invalid {
            assert!(
                matches!(request.plan(), Err(ManagerError::BadRequest(_))),
                "{request:?} should be rejected"
            );
        }
    }

    #[test]
    fn output_under_budget_is_returned_whole() {
        assert_eq!(captured(1_024, &[]), (String::new(), false));
        assert_eq!(
            captured(1_024, &[b"hello ", b"world\n"]),
            ("hello world\n".to_owned(), false)
        );
        let exact = "x".repeat(1_024);
        assert_eq!(captured(1_024, &[exact.as_bytes()]), (exact, false));
    }

    #[test]
    fn output_over_budget_keeps_head_and_tail_with_marker() {
        // 2,000 bytes into a 1,024-byte budget: 256 head bytes, 768 tail bytes.
        let output = (0..2_000)
            .map(|index| char::from(b'a' + (index % 26) as u8))
            .collect::<String>();
        let (text, truncated) = captured(1_024, &[output.as_bytes()]);
        assert!(truncated);
        let expected = format!(
            "{}\n…[976 bytes truncated]…\n{}",
            &output[..256],
            &output[2_000 - 768..]
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn chunked_output_truncates_like_a_single_write() {
        let output = (0..10_000)
            .map(|index| char::from(b'0' + (index % 10) as u8))
            .collect::<String>();
        let whole = captured(1_024, &[output.as_bytes()]);
        for chunk_size in [1, 7, 100, 255, 256, 257, 768, 1_024, 4_096] {
            let chunks = output.as_bytes().chunks(chunk_size).collect::<Vec<_>>();
            assert_eq!(
                captured(1_024, &chunks),
                whole,
                "chunk size {chunk_size} changed the result"
            );
        }
        // One byte over the budget still truncates.
        let (_, truncated) = captured(1_024, &[&[b'x'; 1_025]]);
        assert!(truncated);
    }

    #[test]
    fn captured_output_is_decoded_as_lossy_utf8() {
        let (text, truncated) = captured(1_024, &[b"ok \xff\xfe done"]);
        assert!(!truncated);
        assert_eq!(text, "ok \u{fffd}\u{fffd} done");
    }

    #[test]
    fn timeout_exit_codes_only_count_after_the_deadline() {
        let deadline = Duration::from_mins(1);
        assert!(timed_out_exit(124, Duration::from_mins(1), deadline));
        assert!(timed_out_exit(137, Duration::from_secs(65), deadline));
        assert!(timed_out_exit(124, Duration::from_millis(59_800), deadline));
        // Exiting with 124 or 137 early is the command's own doing, such as
        // an inner timeout or an out-of-memory kill.
        assert!(!timed_out_exit(124, Duration::from_secs(3), deadline));
        assert!(!timed_out_exit(137, Duration::from_secs(10), deadline));
        assert!(!timed_out_exit(0, Duration::from_secs(70), deadline));
        assert!(!timed_out_exit(1, Duration::from_secs(70), deadline));
        assert!(!timed_out_exit(143, Duration::from_secs(70), deadline));
    }

    #[test]
    fn sandbox_containers_are_hardened_and_secret_free() -> anyhow::Result<()> {
        let input = create_request();
        let body = sandbox_container_body(&input, &limits());

        assert_eq!(body.image.as_deref(), Some(input.image.as_str()));
        assert_eq!(body.user.as_deref(), Some("0:0"));
        assert_eq!(body.working_dir.as_deref(), Some("/workspace"));
        assert_eq!(
            body.entrypoint,
            Some(vec!["/bin/sh".to_owned(), "-c".to_owned()])
        );
        assert_eq!(
            body.cmd,
            Some(vec![
                "mkdir -p /workspace && exec sleep infinity".to_owned()
            ])
        );
        assert_eq!(body.env, Some(Vec::new()));
        assert_ne!(body.open_stdin, Some(true));

        let labels = body.labels.as_ref().context("sandbox labels")?;
        assert_eq!(labels.get(SANDBOX_LABEL).map(String::as_str), Some("true"));
        assert_eq!(
            labels.get(SANDBOX_ID_LABEL).map(String::as_str),
            Some(input.sandbox_id.as_str())
        );
        assert!(!labels.contains_key("io.gridops.managed"));
        assert!(sandbox_labels_match(labels, &input.sandbox_id));

        let host = body.host_config.as_ref().context("sandbox host config")?;
        assert_eq!(host.binds, None);
        assert_eq!(host.mounts, None);
        assert_eq!(host.group_add, None);
        assert_eq!(host.privileged, Some(false));
        assert_eq!(host.cap_drop, Some(vec!["ALL".to_owned()]));
        assert_eq!(
            host.cap_add,
            Some(
                ["CHOWN", "DAC_OVERRIDE", "FOWNER", "SETUID", "SETGID"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(
            host.security_opt,
            Some(vec!["no-new-privileges:true".to_owned()])
        );
        assert_eq!(host.network_mode.as_deref(), Some("gridops-runners"));
        assert_eq!(host.nano_cpus, Some(2_000_000_000));
        assert_eq!(host.memory, Some(2_048 * 1_024 * 1_024));
        assert_eq!(host.memory_swap, Some(2 * 2_048 * 1_024 * 1_024));
        assert_eq!(host.pids_limit, Some(1_024));
        assert_eq!(host.oom_score_adj, Some(500));
        assert_eq!(host.init, Some(true));
        let log = host.log_config.as_ref().context("sandbox log config")?;
        assert_eq!(log.typ.as_deref(), Some("json-file"));
        let log_options = log.config.as_ref().context("sandbox log options")?;
        assert_eq!(log_options.get("max-size").map(String::as_str), Some("20m"));
        assert_eq!(log_options.get("max-file").map(String::as_str), Some("5"));

        let serialized = serde_json::to_string(&body)?;
        assert!(!serialized.contains("docker.sock"));
        Ok(())
    }

    #[test]
    fn sandbox_labels_identify_only_the_named_sandbox() {
        let labels = sandbox_labels("abcd1234");
        assert!(sandbox_labels_match(&labels, "abcd1234"));
        assert!(!sandbox_labels_match(&labels, "abcd12345"));
        // Runner routes refuse sandboxes outright.
        assert!(!managed_labels(&labels));

        let mut runner = labels.clone();
        runner.insert("io.gridops.managed".into(), "true".into());
        assert!(!sandbox_labels_match(&runner, "abcd1234"));
        let mut unlabelled = labels;
        unlabelled.remove(SANDBOX_LABEL);
        assert!(!sandbox_labels_match(&unlabelled, "abcd1234"));
    }

    #[test]
    fn declared_archive_lengths_over_the_limit_are_rejected() {
        let headers = |length: &str| {
            let mut headers = HeaderMap::new();
            if let Ok(value) = length.parse() {
                headers.insert(header::CONTENT_LENGTH, value);
            }
            headers
        };
        assert!(!declared_length_exceeds(
            &HeaderMap::new(),
            ARCHIVE_MAX_BYTES
        ));
        assert!(!declared_length_exceeds(
            &headers("1073741824"),
            ARCHIVE_MAX_BYTES
        ));
        assert!(declared_length_exceeds(
            &headers("1073741825"),
            ARCHIVE_MAX_BYTES
        ));
    }

    #[tokio::test]
    async fn archive_streams_fail_once_they_pass_the_limit() {
        let failure = Arc::new(AtomicU8::new(UPLOAD_OK));
        let chunks = limited_archive_stream(Body::from(vec![0_u8; 64]), 64, failure.clone())
            .collect::<Vec<_>>()
            .await;
        assert!(chunks.iter().all(Result::is_ok));
        assert_eq!(failure.load(Ordering::Relaxed), UPLOAD_OK);

        let chunks = limited_archive_stream(Body::from(vec![0_u8; 65]), 64, failure.clone())
            .collect::<Vec<_>>()
            .await;
        assert!(chunks.iter().any(Result::is_err));
        assert_eq!(failure.load(Ordering::Relaxed), UPLOAD_TOO_LARGE);
    }

    /// Serves the real router against a Docker endpoint that refuses every
    /// connection, so these tests cover only what happens before Docker.
    struct TestManager {
        base_url: String,
        state: ManagerState,
        socket: std::path::PathBuf,
    }

    impl TestManager {
        async fn start() -> anyhow::Result<Self> {
            let socket = std::env::temp_dir().join(format!(
                "gridops-sandbox-test-{}.sock",
                uuid::Uuid::new_v4()
            ));
            std::fs::write(&socket, "")?;
            let docker = Docker::connect_with_socket(
                &socket.to_string_lossy(),
                2,
                bollard::API_DEFAULT_VERSION,
            )?;
            let state = ManagerState {
                docker,
                tart: None,
                token: SecretString::from(TOKEN),
                limits: limits(),
                shared_docker_socket: Some(SharedDockerSocket {
                    host_path: "/var/run/docker.sock".into(),
                    group_id: 0,
                }),
                reservations: Arc::new(Mutex::new(HashMap::new())),
                provisioning_paused: Arc::new(AtomicBool::new(false)),
                provision_lock: Arc::new(Mutex::new(())),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base_url = format!("http://{}", listener.local_addr()?);
            let app = crate::router(state.clone());
            tokio::spawn(async move { axum::serve(listener, app).await });
            Ok(Self {
                base_url,
                state,
                socket,
            })
        }

        fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
            reqwest::Client::new()
                .request(method, format!("{}{path}", self.base_url))
                .bearer_auth(TOKEN)
        }

        async fn error_code(response: reqwest::Response) -> anyhow::Result<String> {
            let body = response.json::<Value>().await?;
            Ok(body
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned())
        }
    }

    impl Drop for TestManager {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.socket);
        }
    }

    fn create_body() -> Value {
        json!({
            "sandboxId": "3f2b6c1e-0d4a-4d0e-9a51-7c1f0b2e8d44",
            "image": "ghcr.io/actions/actions-runner:latest",
            "pullImage": true,
            "cpuLimit": 2.0,
            "memoryLimitMb": 2048,
            "network": "gridops-runners",
            "capacityLease": "lease-1",
        })
    }

    #[tokio::test]
    async fn sandbox_routes_require_the_manager_token() -> anyhow::Result<()> {
        let manager = TestManager::start().await?;
        let client = reqwest::Client::new();
        for (method, path) in [
            (reqwest::Method::GET, "/v1/sandboxes"),
            (reqwest::Method::POST, "/v1/sandboxes"),
            (reqwest::Method::DELETE, "/v1/sandboxes/abcd1234"),
            (reqwest::Method::POST, "/v1/sandboxes/abcd1234/exec"),
            (reqwest::Method::PUT, "/v1/sandboxes/abcd1234/archive"),
        ] {
            let response = client
                .request(method.clone(), format!("{}{path}", manager.base_url))
                .bearer_auth("wrong-token-value!")
                .header(header::CONTENT_TYPE, "application/json")
                .body("{}")
                .send()
                .await?;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} accepted a bad token"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn sandbox_creation_honours_pause_network_and_lease_guardrails() -> anyhow::Result<()> {
        let manager = TestManager::start().await?;
        let create = |body: &Value| {
            manager
                .request(reqwest::Method::POST, "/v1/sandboxes")
                .json(body)
                .send()
        };

        manager
            .state
            .provisioning_paused
            .store(true, Ordering::Relaxed);
        let response = create(&create_body()).await?;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            TestManager::error_code(response).await?,
            "provisioning_paused"
        );
        manager
            .state
            .provisioning_paused
            .store(false, Ordering::Relaxed);

        let mut foreign_network = create_body();
        foreign_network["network"] = json!("bridge");
        assert_eq!(
            create(&foreign_network).await?.status(),
            StatusCode::FORBIDDEN
        );

        let response = create(&create_body()).await?;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            TestManager::error_code(response).await?,
            "capacity_lease_invalid"
        );

        let reservation = |pool_id: &str| CapacityReservation {
            runner_id: "3f2b6c1e-0d4a-4d0e-9a51-7c1f0b2e8d44".into(),
            pool_id: pool_id.into(),
            provider: "docker".into(),
            cpu_limit: 2.0,
            memory_limit_mb: 2_048,
            expires_at: Instant::now() + Duration::from_mins(5),
        };
        manager
            .state
            .reservations
            .lock()
            .await
            .insert("lease-1".into(), reservation("pool-1"));
        assert_eq!(
            create(&create_body()).await?.status(),
            StatusCode::FORBIDDEN
        );

        // A matching lease is consumed even though Docker is unreachable.
        manager
            .state
            .reservations
            .lock()
            .await
            .insert("lease-1".into(), reservation("gridops-agent"));
        assert_eq!(
            create(&create_body()).await?.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(manager.state.reservations.lock().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn sandbox_requests_are_validated_before_docker_is_touched() -> anyhow::Result<()> {
        let manager = TestManager::start().await?;
        let exec = |sandbox_id: &str, body: Value| {
            manager
                .request(
                    reqwest::Method::POST,
                    &format!("/v1/sandboxes/{sandbox_id}/exec"),
                )
                .json(&body)
                .send()
        };
        assert_eq!(
            exec("Not_A_Sandbox", json!({ "command": ["true"] }))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            exec("abcd1234", json!({ "command": [] })).await?.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            exec(
                "abcd1234",
                json!({ "command": ["true"], "workdir": "/workspace/../etc" })
            )
            .await?
            .status(),
            StatusCode::BAD_REQUEST
        );

        let response = manager
            .request(
                reqwest::Method::PUT,
                "/v1/sandboxes/abcd1234/archive?path=/etc",
            )
            .body(Vec::<u8>::new())
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            manager
                .request(reqwest::Method::DELETE, "/v1/sandboxes/UPPER_CASE")
                .send()
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
        Ok(())
    }
}
