use std::{collections::HashMap, env, time::Duration};

use anyhow::{Context as _, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bollard::{
    API_DEFAULT_VERSION, Docker,
    errors::Error as DockerError,
    models::{ContainerCreateBody, HostConfig, NetworkCreateRequest},
    query_parameters::{
        AttachContainerOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder,
        ListContainersOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
        RestartContainerOptionsBuilder, StartContainerOptions, StopContainerOptionsBuilder,
    },
};
use futures_util::{StreamExt as _, TryStreamExt as _};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;
use tokio::io::AsyncWriteExt as _;
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[derive(Clone)]
struct ManagerState {
    docker: Docker,
    token: SecretString,
}

struct ManagerAuth;

impl FromRequestParts<ManagerState> for ManagerAuth {
    type Rejection = ManagerError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ManagerState,
    ) -> Result<Self, Self::Rejection> {
        let supplied = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ManagerError::Unauthorized)?;
        let expected = state.token.expose_secret();
        if supplied.len() != expected.len()
            || !bool::from(supplied.as_bytes().ct_eq(expected.as_bytes()))
        {
            return Err(ManagerError::Unauthorized);
        }
        Ok(Self)
    }
}

#[derive(Debug)]
enum ManagerError {
    Unauthorized,
    Forbidden(String),
    BadRequest(String),
    Conflict(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ManagerError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".into()),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, message),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Internal(error) => {
                tracing::error!(error = ?error, "runner manager request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Runner manager request failed.".into(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<DockerError> for ManagerError {
    fn from(error: DockerError) -> Self {
        match &error {
            DockerError::DockerResponseServerError {
                status_code: 404, ..
            } => Self::NotFound("Managed runner container was not found.".into()),
            DockerError::DockerResponseServerError {
                status_code: 409,
                message,
            } => Self::Conflict(message.clone()),
            _ => Self::Internal(error.into()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRunner {
    runner_id: String,
    pool_id: String,
    name: String,
    image: String,
    mode: String,
    jit_config: Option<String>,
    registration_token: Option<String>,
    registration_url: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    runner_group: Option<String>,
    #[serde(default)]
    pull_image: bool,
    cpu_limit: f64,
    memory_limit_mb: i64,
    network: String,
}

impl ProvisionRunner {
    fn validate(&self) -> Result<(), ManagerError> {
        if self.runner_id.is_empty() || self.pool_id.is_empty() {
            return Err(ManagerError::BadRequest(
                "Runner and pool identifiers are required.".into(),
            ));
        }
        if !valid_docker_name(&self.name) || !valid_docker_name(&self.network) {
            return Err(ManagerError::BadRequest(
                "Runner or network name contains unsupported characters.".into(),
            ));
        }
        if self.image.trim().is_empty() || self.image.len() > 512 {
            return Err(ManagerError::BadRequest("Runner image is invalid.".into()));
        }
        match self.mode.as_str() {
            "ephemeral" => {
                if self.jit_config.as_ref().is_none_or(|value| {
                    !(20..=900_000).contains(&value.len()) || value.contains(['\n', '\r'])
                }) {
                    return Err(ManagerError::BadRequest(
                        "Runner JIT configuration is invalid.".into(),
                    ));
                }
            }
            "persistent" => {
                if self.registration_token.as_ref().is_none_or(|value| {
                    !(20..=2_048).contains(&value.len()) || value.contains(['\n', '\r'])
                }) || self
                    .registration_url
                    .as_deref()
                    .is_none_or(|value| !valid_registration_url(value))
                    || self.labels.is_empty()
                    || self.labels.len() > 100
                    || self.labels.iter().any(|label| {
                        label.is_empty() || label.len() > 100 || label.contains([',', '\n', '\r'])
                    })
                    || self.runner_group.as_ref().is_some_and(|group| {
                        group.is_empty() || group.len() > 100 || group.contains(['\n', '\r'])
                    })
                {
                    return Err(ManagerError::BadRequest(
                        "Persistent runner registration is invalid.".into(),
                    ));
                }
            }
            _ => {
                return Err(ManagerError::BadRequest(
                    "Runner mode must be ephemeral or persistent.".into(),
                ));
            }
        }
        if !(0.25..=64.0).contains(&self.cpu_limit)
            || !(256..=262_144).contains(&self.memory_limit_mb)
        {
            return Err(ManagerError::BadRequest(
                "Runner resource limits are outside the supported range.".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedRunner {
    id: String,
    names: Vec<String>,
    image: String,
    state: String,
    status: String,
    labels: HashMap<String, String>,
    created_at: String,
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    follow: bool,
    tail: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gridops_manager=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let token = env::var("GRIDOPS_MANAGER_TOKEN")
        .context("GRIDOPS_MANAGER_TOKEN is required by the runner manager")?;
    let socket =
        env::var("GRIDOPS_DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".into());
    let bind = env::var("GRIDOPS_MANAGER_BIND").unwrap_or_else(|_| "127.0.0.1:8788".into());
    let docker = Docker::connect_with_socket(&socket, 120, API_DEFAULT_VERSION)
        .context("could not connect to Docker")?;
    let state = ManagerState {
        docker,
        token: SecretString::from(token),
    };

    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/runners", get(list_runners).post(provision_runner))
        .route("/v1/runners/{container_id}", delete(delete_runner))
        .route("/v1/runners/{container_id}/logs", get(logs))
        .route("/v1/runners/{container_id}/{action}", post(control_runner))
        .with_state(state)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_mins(3),
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(address = bind, "GridOps Rust runner manager listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
) -> Result<Json<Value>, ManagerError> {
    state.docker.ping().await?;
    let version = state.docker.version().await?;
    let info = state.docker.info().await?;
    Ok(Json(json!({
        "status": "ok",
        "dockerVersion": version.version,
        "apiVersion": version.api_version,
        "availableCpus": info.ncpu,
    })))
}

async fn list_runners(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
) -> Result<Json<Value>, ManagerError> {
    let filters = HashMap::from([(
        "label".to_owned(),
        vec!["io.gridops.managed=true".to_owned()],
    )]);
    let options = ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let containers = state.docker.list_containers(Some(options)).await?;
    let runners = containers
        .into_iter()
        .map(|container| ManagedRunner {
            id: container.id.unwrap_or_default(),
            names: container.names.unwrap_or_default(),
            image: container.image.unwrap_or_default(),
            state: container
                .state
                .map_or_else(String::new, |value| value.to_string()),
            status: container.status.unwrap_or_default(),
            labels: container.labels.unwrap_or_default(),
            created_at: chrono::DateTime::from_timestamp(container.created.unwrap_or_default(), 0)
                .map_or_else(String::new, |value| value.to_rfc3339()),
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "runners": runners })))
}

async fn provision_runner(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
    Json(mut input): Json<ProvisionRunner>,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    input.validate()?;
    let bootstrap_secret = take_bootstrap_secret(&mut input)?;
    ensure_network(&state.docker, &input.network).await?;
    ensure_image(&state.docker, &input.image, input.pull_image).await?;

    let filters = HashMap::from([("name".to_owned(), vec![input.name.clone()])]);
    let existing = state
        .docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await?;
    if existing.iter().any(|container| {
        container
            .names
            .as_ref()
            .is_some_and(|names| names.iter().any(|name| name == &format!("/{}", input.name)))
    }) {
        return Err(ManagerError::Conflict(
            "A runner container with this name already exists.".into(),
        ));
    }

    let memory_bytes = input.memory_limit_mb * 1_024 * 1_024;
    let (cmd, env) = runner_command(&input)?;
    let labels = HashMap::from([
        ("io.gridops.managed".into(), "true".into()),
        ("io.gridops.runner-id".into(), input.runner_id),
        ("io.gridops.pool-id".into(), input.pool_id),
        ("io.gridops.mode".into(), input.mode),
    ]);
    let body = ContainerCreateBody {
        image: Some(input.image),
        cmd: Some(cmd),
        env: Some(env),
        attach_stdin: Some(true),
        open_stdin: Some(true),
        labels: Some(labels),
        host_config: Some(HostConfig {
            auto_remove: Some(false),
            network_mode: Some(input.network),
            nano_cpus: Some((input.cpu_limit * 1_000_000_000.0) as i64),
            memory: Some(memory_bytes),
            memory_swap: Some(memory_bytes),
            pids_limit: Some(2_048),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let container = state
        .docker
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(&input.name)
                    .build(),
            ),
            body,
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
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
        return Err(error.into());
    }
    let bootstrap_result = async {
        let mut connection = state
            .docker
            .attach_container(
                &container.id,
                Some(
                    AttachContainerOptionsBuilder::default()
                        .stdin(true)
                        .stream(true)
                        .build(),
                ),
            )
            .await?;
        connection
            .input
            .write_all(bootstrap_secret.expose_secret().as_bytes())
            .await
            .map_err(|error| ManagerError::Internal(error.into()))?;
        connection
            .input
            .write_all(b"\n")
            .await
            .map_err(|error| ManagerError::Internal(error.into()))?;
        connection
            .input
            .shutdown()
            .await
            .map_err(|error| ManagerError::Internal(error.into()))?;
        Result::<(), ManagerError>::Ok(())
    }
    .await;
    if let Err(error) = bootstrap_result {
        let _ = state
            .docker
            .remove_container(
                &container.id,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
        return Err(error);
    }
    let details = state.docker.inspect_container(&container.id, None).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": details.id.unwrap_or(container.id),
            "name": details.name.unwrap_or_default().trim_start_matches('/'),
            "state": details.state.and_then(|value| value.status).map_or_else(|| "unknown".into(), |value| value.to_string()),
            "createdAt": details.created,
        })),
    ))
}

async fn control_runner(
    State(state): State<ManagerState>,
    Path((container_id, action)): Path<(String, String)>,
    _auth: ManagerAuth,
) -> Result<Json<Value>, ManagerError> {
    validate_container_id(&container_id)?;
    let labels = managed_container_labels(&state.docker, &container_id).await?;
    if matches!(action.as_str(), "start" | "restart")
        && labels.get("io.gridops.mode").map(String::as_str) == Some("ephemeral")
    {
        return Err(ManagerError::Conflict(
            "Ephemeral runners cannot be started or restarted; rebuild the runner instead.".into(),
        ));
    }
    let status = match action.as_str() {
        "start" => {
            state
                .docker
                .start_container(&container_id, None::<StartContainerOptions>)
                .await
                .or_else(ignore_not_modified)?;
            "running"
        }
        "stop" => {
            state
                .docker
                .stop_container(
                    &container_id,
                    Some(StopContainerOptionsBuilder::default().t(30).build()),
                )
                .await
                .or_else(ignore_not_modified)?;
            "stopped"
        }
        "pause" => {
            state.docker.pause_container(&container_id).await?;
            "paused"
        }
        "resume" => {
            state.docker.unpause_container(&container_id).await?;
            "running"
        }
        "restart" => {
            state
                .docker
                .restart_container(
                    &container_id,
                    Some(RestartContainerOptionsBuilder::default().t(30).build()),
                )
                .await?;
            "running"
        }
        _ => {
            return Err(ManagerError::BadRequest(
                "Runner action must be start, stop, pause, resume, or restart.".into(),
            ));
        }
    };
    Ok(Json(json!({ "status": status })))
}

async fn logs(
    State(state): State<ManagerState>,
    Path(container_id): Path<String>,
    Query(query): Query<LogsQuery>,
    _auth: ManagerAuth,
) -> Result<Response, ManagerError> {
    validate_container_id(&container_id)?;
    managed_container_labels(&state.docker, &container_id).await?;
    let tail = query.tail.unwrap_or_else(|| "500".into());
    if !valid_log_tail(&tail) {
        return Err(ManagerError::BadRequest(
            "Log tail must be all or a number from 0 to 100000.".into(),
        ));
    }
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .timestamps(true)
        .follow(query.follow)
        .tail(&tail)
        .build();
    let stream = state
        .docker
        .logs(&container_id, Some(options))
        .map(|result| result.map(|line| line.to_string()));
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

async fn delete_runner(
    State(state): State<ManagerState>,
    Path(container_id): Path<String>,
    _auth: ManagerAuth,
) -> Result<Json<Value>, ManagerError> {
    validate_container_id(&container_id)?;
    managed_container_labels(&state.docker, &container_id).await?;
    state
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
        .await?;
    Ok(Json(json!({ "status": "deleted" })))
}

async fn ensure_network(docker: &Docker, name: &str) -> Result<(), ManagerError> {
    match docker.inspect_network(name, None).await {
        Ok(_) => Ok(()),
        Err(DockerError::DockerResponseServerError {
            status_code: 404, ..
        }) => {
            docker
                .create_network(NetworkCreateRequest {
                    name: name.into(),
                    driver: Some("bridge".into()),
                    internal: Some(false),
                    labels: Some(HashMap::from([(
                        "io.gridops.managed".into(),
                        "true".into(),
                    )])),
                    ..Default::default()
                })
                .await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

async fn ensure_image(docker: &Docker, image: &str, pull_image: bool) -> Result<(), ManagerError> {
    match docker.inspect_image(image).await {
        Ok(_) if !pull_image => Ok(()),
        Ok(_)
        | Err(DockerError::DockerResponseServerError {
            status_code: 404, ..
        }) => {
            docker
                .create_image(
                    Some(
                        CreateImageOptionsBuilder::default()
                            .from_image(image)
                            .build(),
                    ),
                    None,
                    None,
                )
                .try_collect::<Vec<_>>()
                .await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn runner_command(input: &ProvisionRunner) -> Result<(Vec<String>, Vec<String>), ManagerError> {
    match input.mode.as_str() {
        "ephemeral" => Ok((
            vec![
                "/bin/bash".into(),
                "-lc".into(),
                instrumented_runner_script(
                    r#"set -euo pipefail
IFS= read -r GRIDOPS_BOOTSTRAP_SECRET
[ -n "$GRIDOPS_BOOTSTRAP_SECRET" ]
runner_args=(--jitconfig "$GRIDOPS_BOOTSTRAP_SECRET")
unset GRIDOPS_BOOTSTRAP_SECRET"#,
                    r#"/home/runner/run.sh "${runner_args[@]}""#,
                ),
            ],
            vec!["ACTIONS_RUNNER_PRINT_LOG_TO_STDOUT=1".into()],
        )),
        "persistent" => {
            let registration_url = input.registration_url.as_ref().ok_or_else(|| {
                ManagerError::BadRequest("Runner registration URL is required.".into())
            })?;
            let script = r#"set -euo pipefail
if [ ! -f .runner ]; then
IFS= read -r GRIDOPS_BOOTSTRAP_SECRET
[ -n "$GRIDOPS_BOOTSTRAP_SECRET" ]
args=(--unattended --url "$GRIDOPS_REGISTRATION_URL" --token "$GRIDOPS_BOOTSTRAP_SECRET" --name "$GRIDOPS_RUNNER_NAME" --labels "$GRIDOPS_RUNNER_LABELS" --work _work --replace --disableupdate)
if [ -n "${GRIDOPS_RUNNER_GROUP:-}" ]; then args+=(--runnergroup "$GRIDOPS_RUNNER_GROUP"); fi
./config.sh "${args[@]}"
unset GRIDOPS_BOOTSTRAP_SECRET
fi"#;
            Ok((
                vec![
                    "/bin/bash".into(),
                    "-lc".into(),
                    instrumented_runner_script(script, "./run.sh"),
                ],
                vec![
                    format!("GRIDOPS_REGISTRATION_URL={registration_url}"),
                    format!("GRIDOPS_RUNNER_NAME={}", input.name),
                    format!("GRIDOPS_RUNNER_LABELS={}", input.labels.join(",")),
                    format!(
                        "GRIDOPS_RUNNER_GROUP={}",
                        input.runner_group.as_deref().unwrap_or_default()
                    ),
                    "ACTIONS_RUNNER_PRINT_LOG_TO_STDOUT=1".into(),
                ],
            ))
        }
        _ => Err(ManagerError::BadRequest("Runner mode is invalid.".into())),
    }
}

fn instrumented_runner_script(setup: &str, launch: &str) -> String {
    format!(
        r#"{setup}
{launch} &
runner_pid=$!
trap 'kill -TERM "$runner_pid" 2>/dev/null || true' TERM INT

# GitHub's runner writes the actual step console stream to rotating files under
# _diag/pages. Mirror every page to container stdout so GridOps can stream job
# output while it is still running, while retaining the runner's secret masking.
runner_root=${{GRIDOPS_RUNNER_ROOT:-/home/runner}}
forwarded_state=/tmp/gridops-forwarded-job-state
rm -rf "$forwarded_state"
mkdir -p "$forwarded_state"

flush_job_logs() {{
  for job_log in "$runner_root"/_diag/pages/*.log; do
    [ -f "$job_log" ] || continue
    state_file="$forwarded_state/$(basename "$job_log").offset"
    offset=0
    if [ -f "$state_file" ]; then read -r offset < "$state_file" || offset=0; fi
    size=$(wc -c < "$job_log" 2>/dev/null) || continue
    if [ "$size" -gt "$offset" ]; then
      if [ "$offset" -eq 0 ]; then
        printf '\n[GRIDOPS JOB LOG %s]\n' "$(basename "$job_log")"
      fi
      head -c "$size" "$job_log" 2>/dev/null | tail -c "+$((offset + 1))" || true
      printf '%s\n' "$size" > "$state_file"
    fi
  done
}}

(
  while kill -0 "$runner_pid" 2>/dev/null; do
    flush_job_logs
    sleep 0.1
  done
  flush_job_logs
) &
forwarder_pid=$!

set +e
wait "$runner_pid"
runner_status=$?
set -e
wait "$forwarder_pid" 2>/dev/null || true
exit "$runner_status""#
    )
}

fn take_bootstrap_secret(input: &mut ProvisionRunner) -> Result<SecretString, ManagerError> {
    let value = match input.mode.as_str() {
        "ephemeral" => input.jit_config.take(),
        "persistent" => input.registration_token.take(),
        _ => None,
    }
    .ok_or_else(|| ManagerError::BadRequest("Runner bootstrap credential is required.".into()))?;
    Ok(SecretString::from(value))
}

fn ignore_not_modified(error: DockerError) -> Result<(), DockerError> {
    if matches!(
        error,
        DockerError::DockerResponseServerError {
            status_code: 304,
            ..
        }
    ) {
        return Ok(());
    }
    Err(error)
}

fn validate_container_id(id: &str) -> Result<(), ManagerError> {
    if (12..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(ManagerError::BadRequest(
        "Container identifier is invalid.".into(),
    ))
}

async fn managed_container_labels(
    docker: &Docker,
    id: &str,
) -> Result<HashMap<String, String>, ManagerError> {
    let details = docker.inspect_container(id, None).await?;
    let labels = details.config.and_then(|config| config.labels);
    if let Some(labels) = labels.filter(managed_labels) {
        return Ok(labels);
    }
    Err(ManagerError::Forbidden(
        "GridOps can only control containers carrying its managed-runner label.".into(),
    ))
}

fn managed_labels(labels: &HashMap<String, String>) -> bool {
    labels.get("io.gridops.managed").map(String::as_str) == Some("true")
        && labels
            .get("io.gridops.runner-id")
            .is_some_and(|value| !value.is_empty())
        && labels
            .get("io.gridops.pool-id")
            .is_some_and(|value| !value.is_empty())
}

fn valid_docker_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_registration_url(value: &str) -> bool {
    value.starts_with("https://github.com/")
        && value.len() <= 512
        && !value.bytes().any(|byte| byte.is_ascii_whitespace())
}

fn valid_log_tail(value: &str) -> bool {
    value == "all" || value.parse::<u32>().is_ok_and(|lines| lines <= 100_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provision_request(mode: &str) -> ProvisionRunner {
        ProvisionRunner {
            runner_id: "runner-1".into(),
            pool_id: "pool-1".into(),
            name: "runner-1".into(),
            image: "ghcr.io/actions/actions-runner:latest".into(),
            mode: mode.into(),
            jit_config: (mode == "ephemeral").then(|| "fake-jit-credential-123456".into()),
            registration_token: (mode == "persistent").then(|| "fake-registration-123456".into()),
            registration_url: (mode == "persistent")
                .then(|| "https://github.com/iamngoni/gridops".into()),
            labels: vec!["self-hosted".into(), "linux".into()],
            runner_group: None,
            pull_image: false,
            cpu_limit: 2.0,
            memory_limit_mb: 4_096,
            network: "gridops-runners".into(),
        }
    }

    #[test]
    fn requires_complete_gridops_management_labels() {
        let mut labels = HashMap::from([
            ("io.gridops.managed".into(), "true".into()),
            ("io.gridops.runner-id".into(), "runner-1".into()),
            ("io.gridops.pool-id".into(), "pool-1".into()),
        ]);
        assert!(managed_labels(&labels));
        labels.remove("io.gridops.pool-id");
        assert!(!managed_labels(&labels));
        labels.insert("io.gridops.pool-id".into(), "pool-1".into());
        labels.insert("io.gridops.managed".into(), "false".into());
        assert!(!managed_labels(&labels));
    }

    #[test]
    fn validates_registration_urls() {
        assert!(valid_registration_url(
            "https://github.com/iamngoni/gridops"
        ));
        assert!(valid_registration_url("https://github.com/iamngoni"));
        assert!(!valid_registration_url(
            "https://example.com/iamngoni/gridops"
        ));
        assert!(!valid_registration_url("https://github.com/iamngoni bad"));
    }

    #[test]
    fn validates_log_tails() {
        assert!(valid_log_tail("all"));
        assert!(valid_log_tail("0"));
        assert!(valid_log_tail("500"));
        assert!(!valid_log_tail("100001"));
        assert!(!valid_log_tail("recent"));
    }

    #[test]
    fn keeps_ephemeral_bootstrap_credentials_out_of_docker_metadata() -> Result<(), ManagerError> {
        let mut input = provision_request("ephemeral");
        input.validate()?;
        let secret = take_bootstrap_secret(&mut input)?;
        let (command, environment) = runner_command(&input)?;
        let metadata = format!("{command:?}{environment:?}");

        assert_eq!(secret.expose_secret(), "fake-jit-credential-123456");
        assert!(input.jit_config.is_none());
        assert!(!metadata.contains(secret.expose_secret()));
        assert!(metadata.contains("read -r GRIDOPS_BOOTSTRAP_SECRET"));
        assert!(metadata.contains("_diag/pages/*.log"));
        assert!(metadata.contains("GRIDOPS JOB LOG"));
        Ok(())
    }

    #[test]
    fn keeps_persistent_bootstrap_credentials_out_of_docker_metadata() -> Result<(), ManagerError> {
        let mut input = provision_request("persistent");
        input.validate()?;
        let secret = take_bootstrap_secret(&mut input)?;
        let (command, environment) = runner_command(&input)?;
        let metadata = format!("{command:?}{environment:?}");

        assert_eq!(secret.expose_secret(), "fake-registration-123456");
        assert!(input.registration_token.is_none());
        assert!(!metadata.contains(secret.expose_secret()));
        assert!(metadata.contains("if [ ! -f .runner ]"));
        assert!(metadata.contains("_diag/pages/*.log"));
        assert!(metadata.contains("GRIDOPS JOB LOG"));
        Ok(())
    }

    #[test]
    fn generated_runner_shell_scripts_are_valid_bash() -> Result<(), ManagerError> {
        for mode in ["ephemeral", "persistent"] {
            let (command, _) = runner_command(&provision_request(mode))?;
            let Some(script) = command.get(2) else {
                return Err(ManagerError::Internal(anyhow::anyhow!(
                    "runner command should include an inline script"
                )));
            };
            let status = std::process::Command::new("bash")
                .args(["-n", "-c", script])
                .status()
                .map_err(|error| ManagerError::Internal(error.into()))?;
            assert!(
                status.success(),
                "generated {mode} runner script is invalid"
            );
        }
        Ok(())
    }

    #[test]
    fn job_console_pages_are_forwarded_to_runner_stdout() -> Result<(), ManagerError> {
        let directory =
            std::env::temp_dir().join(format!("gridops-runner-log-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(directory.join("_diag/pages"))
            .map_err(|error| ManagerError::Internal(error.into()))?;
        let script = instrumented_runner_script(
            "set -euo pipefail",
            r#"/bin/bash -lc 'sleep 0.2; printf "secret-masked job output\n" > "$GRIDOPS_RUNNER_ROOT/_diag/pages/job.log"; sleep 0.5'"#,
        );
        let output = std::process::Command::new("bash")
            .args(["-c", &script])
            .env("GRIDOPS_RUNNER_ROOT", &directory)
            .output()
            .map_err(|error| ManagerError::Internal(error.into()))?;
        std::fs::remove_dir_all(&directory)
            .map_err(|error| ManagerError::Internal(error.into()))?;
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("GRIDOPS JOB LOG job.log"));
        assert!(
            stdout.contains("secret-masked job output"),
            "forwarded stdout was: {stdout}"
        );
        Ok(())
    }
}
