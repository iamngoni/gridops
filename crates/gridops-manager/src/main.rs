use std::{collections::HashMap, env, time::Duration};

use anyhow::{Context as _, Result};
use axum::{
    Json, Router,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use bollard::{
    API_DEFAULT_VERSION, Docker,
    errors::Error as DockerError,
    models::{ContainerCreateBody, HostConfig, NetworkCreateRequest},
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
        LogsOptionsBuilder, RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
        StartContainerOptions, StopContainerOptionsBuilder,
    },
};
use futures_util::{StreamExt as _, TryStreamExt as _};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRunner {
    runner_id: String,
    pool_id: String,
    name: String,
    image: String,
    jit_config: String,
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
        if self.jit_config.len() < 20 || self.jit_config.len() > 900_000 {
            return Err(ManagerError::BadRequest(
                "Runner JIT configuration is invalid.".into(),
            ));
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
    Ok(Json(json!({
        "status": "ok",
        "dockerVersion": version.version,
        "apiVersion": version.api_version,
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
    Json(input): Json<ProvisionRunner>,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    input.validate()?;
    ensure_network(&state.docker, &input.network).await?;
    ensure_image(&state.docker, &input.image).await?;

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
    let labels = HashMap::from([
        ("io.gridops.managed".into(), "true".into()),
        ("io.gridops.runner-id".into(), input.runner_id),
        ("io.gridops.pool-id".into(), input.pool_id),
    ]);
    let body = ContainerCreateBody {
        image: Some(input.image),
        cmd: Some(vec![
            "/home/runner/run.sh".into(),
            "--jitconfig".into(),
            input.jit_config,
        ]),
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
    assert_managed_container(&state.docker, &container_id).await?;
    let status = match action.as_str() {
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
                "Runner action must be stop, pause, resume, or restart.".into(),
            ));
        }
    };
    Ok(Json(json!({ "status": status })))
}

async fn logs(
    State(state): State<ManagerState>,
    Path(container_id): Path<String>,
    _auth: ManagerAuth,
) -> Result<Response, ManagerError> {
    validate_container_id(&container_id)?;
    assert_managed_container(&state.docker, &container_id).await?;
    let options = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .timestamps(true)
        .tail("500")
        .build();
    let output = state
        .docker
        .logs(&container_id, Some(options))
        .map(|result| result.map(|line| line.to_string()))
        .try_collect::<Vec<_>>()
        .await?
        .join("");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        output,
    )
        .into_response())
}

async fn delete_runner(
    State(state): State<ManagerState>,
    Path(container_id): Path<String>,
    _auth: ManagerAuth,
) -> Result<Json<Value>, ManagerError> {
    validate_container_id(&container_id)?;
    assert_managed_container(&state.docker, &container_id).await?;
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

async fn ensure_image(docker: &Docker, image: &str) -> Result<(), ManagerError> {
    match docker.inspect_image(image).await {
        Ok(_) => Ok(()),
        Err(DockerError::DockerResponseServerError {
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

async fn assert_managed_container(docker: &Docker, id: &str) -> Result<(), ManagerError> {
    let details = docker.inspect_container(id, None).await?;
    let labels = details.config.and_then(|config| config.labels);
    if labels.as_ref().is_some_and(managed_labels) {
        return Ok(());
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
