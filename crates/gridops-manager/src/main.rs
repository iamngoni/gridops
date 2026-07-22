use std::{
    collections::HashMap,
    env,
    path::Path as FsPath,
    process::Command,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use bollard::{
    API_DEFAULT_VERSION, Docker,
    errors::Error as DockerError,
    models::{ContainerCreateBody, HostConfig, HostConfigLogConfig, NetworkCreateRequest},
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
use tokio::{io::AsyncWriteExt as _, sync::Mutex};
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};
use url::Url;

#[derive(Clone)]
struct ManagerState {
    docker: Docker,
    tart: Option<TartAgent>,
    token: SecretString,
    limits: ManagerLimits,
    reservations: Arc<Mutex<HashMap<String, CapacityReservation>>>,
    provisioning_paused: Arc<AtomicBool>,
    provision_lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct TartAgent {
    base_url: Url,
    token: SecretString,
    http: reqwest::Client,
}

#[derive(Clone, Debug)]
struct ManagerLimits {
    available_cpus: f64,
    total_memory_mb: i64,
    cpu_budget: f64,
    memory_budget_mb: i64,
    max_runners: usize,
    min_free_disk_mb: u64,
    min_free_disk_percent: u64,
    runner_network: String,
    log_max_size: String,
    log_max_files: u64,
    runner_pids_limit: i64,
}

#[derive(Clone, Debug)]
struct CapacityReservation {
    runner_id: String,
    pool_id: String,
    provider: String,
    cpu_limit: f64,
    memory_limit_mb: i64,
    expires_at: Instant,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapacityUsage {
    active_runners: usize,
    cpu: f64,
    memory_mb: i64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct DiskCapacity {
    total_mb: u64,
    available_mb: u64,
    minimum_free_mb: u64,
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
    Guardrail {
        code: &'static str,
        message: String,
    },
    NotFound(String),
    Upstream {
        status: StatusCode,
        code: String,
        message: String,
    },
    Internal(anyhow::Error),
}

impl IntoResponse for ManagerError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized".into(),
            ),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, "forbidden", message),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            Self::Conflict(message) => (StatusCode::CONFLICT, "conflict", message),
            Self::Guardrail { code, message } => (StatusCode::TOO_MANY_REQUESTS, code, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, "not_found", message),
            Self::Upstream {
                status,
                code,
                message,
            } => return (status, Json(json!({ "code": code, "error": message }))).into_response(),
            Self::Internal(error) => {
                tracing::error!(error = ?error, "runner manager request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Runner manager request failed.".into(),
                )
            }
        };
        (status, Json(json!({ "code": code, "error": message }))).into_response()
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRunner {
    runner_id: String,
    pool_id: String,
    name: String,
    image: String,
    mode: String,
    #[serde(default = "default_provider")]
    provider: String,
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
    capacity_lease: String,
}

impl ProvisionRunner {
    fn validate(&self) -> Result<(), ManagerError> {
        if self.runner_id.is_empty()
            || self.pool_id.is_empty()
            || self.capacity_lease.is_empty()
            || self.capacity_lease.len() > 128
        {
            return Err(ManagerError::BadRequest(
                "Runner, pool, and capacity lease identifiers are required.".into(),
            ));
        }
        if !valid_docker_name(&self.name)
            || (self.provider == "docker" && !valid_docker_name(&self.network))
        {
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
        if !matches!(self.provider.as_str(), "docker" | "tart") {
            return Err(ManagerError::BadRequest(
                "Runner provider must be Docker or Tart.".into(),
            ));
        }
        if self.provider == "tart" && self.mode != "ephemeral" {
            return Err(ManagerError::BadRequest(
                "Tart runner pools must be ephemeral.".into(),
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityRequest {
    runner_id: String,
    pool_id: String,
    #[serde(default = "default_provider")]
    provider: String,
    cpu_limit: f64,
    memory_limit_mb: i64,
}

impl CapacityRequest {
    fn validate(&self) -> Result<(), ManagerError> {
        if self.runner_id.is_empty()
            || self.runner_id.len() > 128
            || self.pool_id.is_empty()
            || self.pool_id.len() > 128
        {
            return Err(ManagerError::BadRequest(
                "Runner and pool identifiers are invalid.".into(),
            ));
        }
        if !(0.25..=64.0).contains(&self.cpu_limit)
            || !(256..=262_144).contains(&self.memory_limit_mb)
        {
            return Err(ManagerError::BadRequest(
                "Runner resource limits are outside the supported range.".into(),
            ));
        }
        if !matches!(self.provider.as_str(), "docker" | "tart") {
            return Err(ManagerError::BadRequest(
                "Runner provider must be Docker or Tart.".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagerPolicy {
    provisioning_paused: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedRunner {
    id: String,
    names: Vec<String>,
    image: String,
    state: String,
    status: String,
    labels: HashMap<String, String>,
    created_at: String,
    provider: String,
}

fn default_provider() -> String {
    "docker".into()
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    follow: bool,
    tail: Option<String>,
}

impl ManagerLimits {
    fn from_environment(available_cpus: i64, total_memory_bytes: i64) -> Result<Self> {
        let available_cpus = available_cpus.max(1) as f64;
        let total_memory_mb = (total_memory_bytes / 1_024 / 1_024).max(512);
        let default_cpu_budget = (available_cpus * 0.75).floor().max(1.0);
        let default_memory_budget_mb = (total_memory_mb * 3 / 4).max(512);
        let cpu_budget = optional_env("GRIDOPS_RUNNER_CPU_BUDGET")?.unwrap_or(default_cpu_budget);
        let memory_budget_mb =
            optional_env("GRIDOPS_RUNNER_MEMORY_BUDGET_MB")?.unwrap_or(default_memory_budget_mb);
        anyhow::ensure!(
            cpu_budget > 0.0 && cpu_budget <= available_cpus,
            "GRIDOPS_RUNNER_CPU_BUDGET must be positive and no greater than the Docker host CPU count"
        );
        anyhow::ensure!(
            memory_budget_mb >= 256 && memory_budget_mb <= total_memory_mb,
            "GRIDOPS_RUNNER_MEMORY_BUDGET_MB must be between 256 MB and Docker host memory"
        );
        let default_max_runners = usize::try_from(
            ((cpu_budget / 2.0).floor() as i64)
                .min(memory_budget_mb / 2_048)
                .max(1),
        )?;
        let max_runners =
            optional_env("GRIDOPS_MAX_MANAGED_RUNNERS")?.unwrap_or(default_max_runners);
        anyhow::ensure!(
            (1..=1_000).contains(&max_runners),
            "GRIDOPS_MAX_MANAGED_RUNNERS must be between 1 and 1000"
        );
        let min_free_disk_mb = optional_env("GRIDOPS_MIN_FREE_DISK_MB")?.unwrap_or(25_600);
        let min_free_disk_percent = optional_env("GRIDOPS_MIN_FREE_DISK_PERCENT")?.unwrap_or(15);
        anyhow::ensure!(
            min_free_disk_percent <= 95,
            "GRIDOPS_MIN_FREE_DISK_PERCENT must be between 0 and 95"
        );
        let runner_pids_limit = optional_env("GRIDOPS_RUNNER_PIDS_LIMIT")?.unwrap_or(1_024);
        anyhow::ensure!(
            (64..=32_768).contains(&runner_pids_limit),
            "GRIDOPS_RUNNER_PIDS_LIMIT must be between 64 and 32768"
        );
        let log_max_size = env::var("GRIDOPS_RUNNER_LOG_MAX_SIZE").unwrap_or_else(|_| "20m".into());
        anyhow::ensure!(
            valid_log_size(&log_max_size),
            "GRIDOPS_RUNNER_LOG_MAX_SIZE must use Docker's positive k, m, or g size syntax"
        );
        let log_max_files = optional_env("GRIDOPS_RUNNER_LOG_MAX_FILES")?.unwrap_or(5);
        anyhow::ensure!(
            (1..=100).contains(&log_max_files),
            "GRIDOPS_RUNNER_LOG_MAX_FILES must be between 1 and 100"
        );
        let runner_network =
            env::var("GRIDOPS_RUNNER_NETWORK").unwrap_or_else(|_| "gridops-runners".into());
        anyhow::ensure!(
            valid_docker_name(&runner_network),
            "GRIDOPS_RUNNER_NETWORK is invalid"
        );
        Ok(Self {
            available_cpus,
            total_memory_mb,
            cpu_budget,
            memory_budget_mb,
            max_runners,
            min_free_disk_mb,
            min_free_disk_percent,
            runner_network,
            log_max_size,
            log_max_files,
            runner_pids_limit,
        })
    }
}

fn optional_env<T>(name: &str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<T>()
                .with_context(|| format!("{name} is invalid"))
        })
        .transpose()
}

fn tart_agent_from_environment() -> Result<Option<TartAgent>> {
    let url = env::var("GRIDOPS_TART_AGENT_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let token = env::var("GRIDOPS_TART_AGENT_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    match (url, token) {
        (None, None) => Ok(None),
        (Some(url), Some(token)) => {
            let base_url = Url::parse(&url).context("GRIDOPS_TART_AGENT_URL is invalid")?;
            anyhow::ensure!(
                matches!(base_url.scheme(), "http" | "https"),
                "GRIDOPS_TART_AGENT_URL must use HTTP or HTTPS"
            );
            let http = reqwest::Client::builder()
                .user_agent("GridOps manager/0.1")
                .timeout(Duration::from_mins(45))
                .build()?;
            Ok(Some(TartAgent {
                base_url,
                token: SecretString::from(token),
                http,
            }))
        }
        _ => anyhow::bail!(
            "GRIDOPS_TART_AGENT_URL and GRIDOPS_TART_AGENT_TOKEN must be configured together"
        ),
    }
}

async fn tart_agent_response(
    agent: &TartAgent,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::Response, ManagerError> {
    let url = agent
        .base_url
        .join(path.trim_start_matches('/'))
        .map_err(|error| ManagerError::Internal(error.into()))?;
    let mut request = agent
        .http
        .request(method, url)
        .bearer_auth(agent.token.expose_secret());
    if let Some(body) = body {
        request = request.json(&body);
    }
    request
        .send()
        .await
        .map_err(|error| ManagerError::Upstream {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "tart_agent_unavailable".into(),
            message: format!("Tart agent is unavailable: {error}"),
        })
}

async fn tart_agent_value(
    agent: &TartAgent,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, ManagerError> {
    let response = tart_agent_response(agent, method, path, body).await?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| ManagerError::Internal(error.into()))?;
    if !status.is_success() {
        let payload = serde_json::from_str::<Value>(&text).ok();
        return Err(ManagerError::Upstream {
            status,
            code: payload
                .as_ref()
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("tart_agent_error")
                .to_owned(),
            message: payload
                .as_ref()
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .unwrap_or(&text)
                .chars()
                .take(2_000)
                .collect(),
        });
    }
    serde_json::from_str(&text).map_err(|error| ManagerError::Internal(error.into()))
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
    let info = docker
        .info()
        .await
        .context("could not inspect Docker capacity")?;
    let limits = ManagerLimits::from_environment(
        info.ncpu.unwrap_or(1),
        info.mem_total.unwrap_or(512 * 1_024 * 1_024),
    )?;
    let tart = tart_agent_from_environment()?;
    let provisioning_paused = env::var("GRIDOPS_PROVISIONING_PAUSED")
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    tracing::info!(
        cpu_budget = limits.cpu_budget,
        memory_budget_mb = limits.memory_budget_mb,
        max_runners = limits.max_runners,
        tart_enabled = tart.is_some(),
        min_free_disk_mb = limits.min_free_disk_mb,
        provisioning_paused,
        "runner host guardrails initialized"
    );
    let state = ManagerState {
        docker,
        tart,
        token: SecretString::from(token),
        limits,
        reservations: Arc::new(Mutex::new(HashMap::new())),
        provisioning_paused: Arc::new(AtomicBool::new(provisioning_paused)),
        provision_lock: Arc::new(Mutex::new(())),
    };

    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/policy", put(update_policy))
        .route("/v1/admissions", post(reserve_capacity))
        .route("/v1/admissions/{lease_id}", delete(release_capacity))
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
    let mut active = docker_active_capacity(&state.docker).await?;
    let reserved = reserved_capacity(&state).await;
    let disk = disk_capacity(&state.limits)?;
    let (status, tart) = match &state.tart {
        Some(agent) => match tart_agent_value(agent, reqwest::Method::GET, "v1/health", None).await
        {
            Ok(health) => {
                if let Some(value) = health
                    .get("capacity")
                    .and_then(|capacity| capacity.get("active"))
                    .cloned()
                    && let Ok(usage) = serde_json::from_value::<CapacityUsage>(value)
                {
                    active.active_runners =
                        active.active_runners.saturating_add(usage.active_runners);
                    active.cpu += usage.cpu;
                    active.memory_mb = active.memory_mb.saturating_add(usage.memory_mb);
                }
                ("ok", json!({ "available": true, "health": health }))
            }
            Err(error) => {
                tracing::warn!(error = ?error, "Tart agent health check failed");
                (
                    "degraded",
                    json!({ "available": false, "error": "Tart agent is unavailable." }),
                )
            }
        },
        None => ("ok", json!({ "available": false, "configured": false })),
    };
    Ok(Json(json!({
        "status": status,
        "dockerVersion": version.version,
        "apiVersion": version.api_version,
        "providers": { "docker": { "available": true }, "tart": tart },
        "availableCpus": state.limits.available_cpus,
        "totalMemoryMb": state.limits.total_memory_mb,
        "provisioningPaused": state.provisioning_paused.load(Ordering::Relaxed),
        "capacity": {
            "cpuBudget": state.limits.cpu_budget,
            "memoryBudgetMb": state.limits.memory_budget_mb,
            "maxRunners": state.limits.max_runners,
            "active": active,
            "reserved": reserved,
        },
        "disk": disk,
    })))
}

async fn update_policy(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
    Json(input): Json<ManagerPolicy>,
) -> Json<Value> {
    state
        .provisioning_paused
        .store(input.provisioning_paused, Ordering::Relaxed);
    Json(json!({ "provisioningPaused": input.provisioning_paused }))
}

async fn reserve_capacity(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
    Json(input): Json<CapacityRequest>,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    input.validate()?;
    let _guard = state.provision_lock.lock().await;
    ensure_provisioning_enabled(&state)?;
    let active = active_capacity(&state).await?;
    let disk = disk_capacity(&state.limits)?;
    let mut reservations = state.reservations.lock().await;
    reservations.retain(|_, reservation| reservation.expires_at > Instant::now());
    let reserved =
        reservations
            .values()
            .fold(CapacityUsage::default(), |mut usage, reservation| {
                usage.active_runners += 1;
                usage.cpu += reservation.cpu_limit;
                usage.memory_mb = usage.memory_mb.saturating_add(reservation.memory_limit_mb);
                usage
            });
    enforce_capacity(
        &state.limits,
        active,
        reserved,
        CapacityUsage {
            active_runners: 1,
            cpu: input.cpu_limit,
            memory_mb: input.memory_limit_mb,
        },
        disk,
    )?;
    let lease_id = uuid::Uuid::new_v4().to_string();
    reservations.insert(
        lease_id.clone(),
        CapacityReservation {
            runner_id: input.runner_id,
            pool_id: input.pool_id,
            provider: input.provider,
            cpu_limit: input.cpu_limit,
            memory_limit_mb: input.memory_limit_mb,
            expires_at: Instant::now() + Duration::from_mins(5),
        },
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({ "leaseId": lease_id, "expiresInSeconds": 300 })),
    ))
}

async fn release_capacity(
    State(state): State<ManagerState>,
    Path(lease_id): Path<String>,
    _auth: ManagerAuth,
) -> Json<Value> {
    let released = state.reservations.lock().await.remove(&lease_id).is_some();
    Json(json!({ "released": released }))
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
    let mut runners = containers
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
            provider: "docker".into(),
        })
        .collect::<Vec<_>>();
    if let Some(agent) = &state.tart {
        let response = tart_agent_value(agent, reqwest::Method::GET, "v1/runners", None).await?;
        let tart_runners = response.get("runners").cloned().ok_or_else(|| {
            ManagerError::Internal(anyhow::anyhow!("Tart agent runner list is invalid"))
        })?;
        runners.extend(
            serde_json::from_value::<Vec<ManagedRunner>>(tart_runners)
                .map_err(|error| ManagerError::Internal(error.into()))?,
        );
    }
    Ok(Json(json!({ "runners": runners })))
}

async fn provision_runner(
    State(state): State<ManagerState>,
    _auth: ManagerAuth,
    Json(mut input): Json<ProvisionRunner>,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    input.validate()?;
    let _guard = state.provision_lock.lock().await;
    ensure_provisioning_enabled(&state)?;
    if input.network != state.limits.runner_network {
        return Err(ManagerError::Forbidden(
            "Runner containers may only join the configured GridOps runner network.".into(),
        ));
    }
    let lease_id = input.capacity_lease.clone();
    validate_capacity_lease(&state, &input).await?;
    if input.provider == "tart" {
        let result = match &state.tart {
            Some(agent) => {
                let body = serde_json::to_value(&input)
                    .map_err(|error| ManagerError::Internal(error.into()))?;
                tart_agent_value(agent, reqwest::Method::POST, "v1/runners", Some(body))
                    .await
                    .map(|value| (StatusCode::CREATED, Json(value)))
            }
            None => Err(ManagerError::Upstream {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "tart_provider_disabled".into(),
                message: "The Tart macOS runner provider is not configured.".into(),
            }),
        };
        state.reservations.lock().await.remove(&lease_id);
        return result;
    }
    let result = provision_runner_inner(&state, &mut input).await;
    state.reservations.lock().await.remove(&lease_id);
    result
}

async fn provision_runner_inner(
    state: &ManagerState,
    input: &mut ProvisionRunner,
) -> Result<(StatusCode, Json<Value>), ManagerError> {
    let bootstrap_secret = take_bootstrap_secret(input)?;
    ensure_network(&state.docker, &input.network).await?;
    ensure_image(&state.docker, &input.image, input.pull_image).await?;
    ensure_disk_capacity(&state.limits)?;

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
    let (cmd, env) = runner_command(input)?;
    let labels = HashMap::from([
        ("io.gridops.managed".into(), "true".into()),
        ("io.gridops.runner-id".into(), input.runner_id.clone()),
        ("io.gridops.pool-id".into(), input.pool_id.clone()),
        ("io.gridops.mode".into(), input.mode.clone()),
    ]);
    let body = ContainerCreateBody {
        image: Some(input.image.clone()),
        cmd: Some(cmd),
        env: Some(env),
        attach_stdin: Some(true),
        open_stdin: Some(true),
        labels: Some(labels),
        host_config: Some(HostConfig {
            auto_remove: Some(false),
            network_mode: Some(input.network.clone()),
            nano_cpus: Some((input.cpu_limit * 1_000_000_000.0) as i64),
            memory: Some(memory_bytes),
            memory_swap: Some(memory_bytes),
            pids_limit: Some(state.limits.runner_pids_limit),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            oom_score_adj: Some(500),
            log_config: Some(HostConfigLogConfig {
                typ: Some("json-file".into()),
                config: Some(HashMap::from([
                    ("max-size".into(), state.limits.log_max_size.clone()),
                    ("max-file".into(), state.limits.log_max_files.to_string()),
                ])),
            }),
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
    if is_tart_id(&container_id) {
        let agent = state.tart.as_ref().ok_or_else(|| ManagerError::Upstream {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "tart_provider_disabled".into(),
            message: "The Tart macOS runner provider is not configured.".into(),
        })?;
        return tart_agent_value(
            agent,
            reqwest::Method::POST,
            &format!("v1/runners/{container_id}/{action}"),
            None,
        )
        .await
        .map(Json);
    }
    validate_container_id(&container_id)?;
    let labels = managed_container_labels(&state.docker, &container_id).await?;
    if matches!(action.as_str(), "start" | "restart")
        && labels.get("io.gridops.mode").map(String::as_str) == Some("ephemeral")
    {
        return Err(ManagerError::Conflict(
            "Ephemeral runners cannot be started or restarted; rebuild the runner instead.".into(),
        ));
    }
    let _guard = if matches!(action.as_str(), "start" | "resume" | "restart") {
        Some(state.provision_lock.lock().await)
    } else {
        None
    };
    if matches!(action.as_str(), "start" | "resume" | "restart") {
        ensure_provisioning_enabled(&state)?;
    }
    if action == "start" {
        let details = state.docker.inspect_container(&container_id, None).await?;
        let current_state = details
            .state
            .as_ref()
            .and_then(|value| value.status.as_ref())
            .map_or_else(String::new, ToString::to_string);
        if !active_container_state(&current_state) {
            let host = details.host_config.unwrap_or_default();
            let requested = CapacityUsage {
                active_runners: 1,
                cpu: host.nano_cpus.unwrap_or_default() as f64 / 1_000_000_000.0,
                memory_mb: host.memory.unwrap_or_default() / 1_024 / 1_024,
            };
            enforce_capacity(
                &state.limits,
                active_capacity(&state).await?,
                reserved_capacity(&state).await,
                requested,
                disk_capacity(&state.limits)?,
            )?;
        }
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
    let tail = query.tail.unwrap_or_else(|| "500".into());
    if !valid_log_tail(&tail) {
        return Err(ManagerError::BadRequest(
            "Log tail must be all or a number from 0 to 100000.".into(),
        ));
    }
    if is_tart_id(&container_id) {
        let agent = state.tart.as_ref().ok_or_else(|| ManagerError::Upstream {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "tart_provider_disabled".into(),
            message: "The Tart macOS runner provider is not configured.".into(),
        })?;
        let path = format!(
            "v1/runners/{container_id}/logs?tail={tail}&follow={}",
            query.follow
        );
        let response = tart_agent_response(agent, reqwest::Method::GET, &path, None).await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(ManagerError::Upstream {
                status,
                code: "tart_log_error".into(),
                message: text.chars().take(2_000).collect(),
            });
        }
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::HeaderName::from_static("x-accel-buffering"), "no"),
            ],
            Body::from_stream(response.bytes_stream()),
        )
            .into_response());
    }
    validate_container_id(&container_id)?;
    managed_container_labels(&state.docker, &container_id).await?;
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
    if is_tart_id(&container_id) {
        let agent = state.tart.as_ref().ok_or_else(|| ManagerError::Upstream {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "tart_provider_disabled".into(),
            message: "The Tart macOS runner provider is not configured.".into(),
        })?;
        return tart_agent_value(
            agent,
            reqwest::Method::DELETE,
            &format!("v1/runners/{container_id}"),
            None,
        )
        .await
        .map(Json);
    }
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

fn ensure_provisioning_enabled(state: &ManagerState) -> Result<(), ManagerError> {
    if state.provisioning_paused.load(Ordering::Relaxed) {
        return Err(ManagerError::Guardrail {
            code: "provisioning_paused",
            message: "Runner provisioning is globally paused.".into(),
        });
    }
    Ok(())
}

async fn validate_capacity_lease(
    state: &ManagerState,
    input: &ProvisionRunner,
) -> Result<(), ManagerError> {
    let mut reservations = state.reservations.lock().await;
    reservations.retain(|_, reservation| reservation.expires_at > Instant::now());
    let reservation =
        reservations
            .get(&input.capacity_lease)
            .ok_or_else(|| ManagerError::Guardrail {
                code: "capacity_lease_invalid",
                message: "Runner capacity reservation is missing or expired.".into(),
            })?;
    if reservation.runner_id != input.runner_id
        || reservation.pool_id != input.pool_id
        || reservation.provider != input.provider
        || (reservation.cpu_limit - input.cpu_limit).abs() > f64::EPSILON
        || reservation.memory_limit_mb != input.memory_limit_mb
    {
        return Err(ManagerError::Forbidden(
            "Runner capacity reservation does not match the provisioning request.".into(),
        ));
    }
    Ok(())
}

async fn docker_active_capacity(docker: &Docker) -> Result<CapacityUsage, ManagerError> {
    let filters = HashMap::from([(
        "label".to_owned(),
        vec!["io.gridops.managed=true".to_owned()],
    )]);
    let containers = docker
        .list_containers(Some(
            ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&filters)
                .build(),
        ))
        .await?;
    let active_ids = containers
        .into_iter()
        .filter(|container| {
            container
                .state
                .as_ref()
                .is_some_and(|state| active_container_state(state.as_ref()))
        })
        .filter_map(|container| container.id)
        .collect::<Vec<_>>();
    let details = futures_util::stream::iter(active_ids)
        .map(|id| async move { docker.inspect_container(&id, None).await })
        .buffer_unordered(16)
        .try_collect::<Vec<_>>()
        .await?;
    let mut usage = CapacityUsage::default();
    for details in details {
        let host = details.host_config.unwrap_or_default();
        usage.active_runners += 1;
        usage.cpu += host.nano_cpus.unwrap_or_default() as f64 / 1_000_000_000.0;
        usage.memory_mb = usage
            .memory_mb
            .saturating_add(host.memory.unwrap_or_default() / 1_024 / 1_024);
    }
    Ok(usage)
}

async fn active_capacity(state: &ManagerState) -> Result<CapacityUsage, ManagerError> {
    let mut usage = docker_active_capacity(&state.docker).await?;
    if let Some(agent) = &state.tart {
        let health = tart_agent_value(agent, reqwest::Method::GET, "v1/health", None).await?;
        let tart_usage = health
            .get("capacity")
            .and_then(|capacity| capacity.get("active"))
            .cloned()
            .ok_or_else(|| {
                ManagerError::Internal(anyhow::anyhow!("Tart agent health omitted active capacity"))
            })?;
        let tart_usage = serde_json::from_value::<CapacityUsage>(tart_usage)
            .map_err(|error| ManagerError::Internal(error.into()))?;
        usage.active_runners = usage
            .active_runners
            .saturating_add(tart_usage.active_runners);
        usage.cpu += tart_usage.cpu;
        usage.memory_mb = usage.memory_mb.saturating_add(tart_usage.memory_mb);
    }
    Ok(usage)
}

async fn reserved_capacity(state: &ManagerState) -> CapacityUsage {
    let mut reservations = state.reservations.lock().await;
    reservations.retain(|_, reservation| reservation.expires_at > Instant::now());
    reservations
        .values()
        .fold(CapacityUsage::default(), |mut usage, reservation| {
            usage.active_runners += 1;
            usage.cpu += reservation.cpu_limit;
            usage.memory_mb = usage.memory_mb.saturating_add(reservation.memory_limit_mb);
            usage
        })
}

fn active_container_state(state: &str) -> bool {
    matches!(state, "created" | "running" | "restarting" | "paused")
}

fn disk_capacity(limits: &ManagerLimits) -> Result<DiskCapacity, ManagerError> {
    let root = FsPath::new("/");
    let output = Command::new("df")
        .args(["-Pk", root.as_os_str().to_string_lossy().as_ref()])
        .output()
        .map_err(|error| ManagerError::Internal(error.into()))?;
    if !output.status.success() {
        return Err(ManagerError::Internal(anyhow::anyhow!(
            "could not inspect runner-host disk capacity"
        )));
    }
    let stdout =
        String::from_utf8(output.stdout).map_err(|error| ManagerError::Internal(error.into()))?;
    let (total_mb, available_mb) = parse_df_capacity(&stdout).ok_or_else(|| {
        ManagerError::Internal(anyhow::anyhow!("runner-host disk capacity was invalid"))
    })?;
    let percent_floor = total_mb.saturating_mul(limits.min_free_disk_percent) / 100;
    Ok(DiskCapacity {
        total_mb,
        available_mb,
        minimum_free_mb: limits.min_free_disk_mb.max(percent_floor),
    })
}

fn parse_df_capacity(output: &str) -> Option<(u64, u64)> {
    let row = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .nth(1)?;
    let columns = row.split_whitespace().collect::<Vec<_>>();
    let total_kb = columns.get(1)?.parse::<u64>().ok()?;
    let available_kb = columns.get(3)?.parse::<u64>().ok()?;
    Some((total_kb / 1_024, available_kb / 1_024))
}

fn ensure_disk_capacity(limits: &ManagerLimits) -> Result<(), ManagerError> {
    let disk = disk_capacity(limits)?;
    if disk.available_mb < disk.minimum_free_mb {
        return Err(ManagerError::Guardrail {
            code: "host_disk_guardrail",
            message: format!(
                "Runner provisioning stopped because only {} MB disk space remains; {} MB is reserved.",
                disk.available_mb, disk.minimum_free_mb
            ),
        });
    }
    Ok(())
}

fn enforce_capacity(
    limits: &ManagerLimits,
    active: CapacityUsage,
    reserved: CapacityUsage,
    requested: CapacityUsage,
    disk: DiskCapacity,
) -> Result<(), ManagerError> {
    if disk.available_mb < disk.minimum_free_mb {
        return Err(ManagerError::Guardrail {
            code: "host_disk_guardrail",
            message: format!(
                "Runner provisioning stopped because only {} MB disk space remains; {} MB is reserved.",
                disk.available_mb, disk.minimum_free_mb
            ),
        });
    }
    let projected_runners = active
        .active_runners
        .saturating_add(reserved.active_runners)
        .saturating_add(requested.active_runners);
    if projected_runners > limits.max_runners {
        return Err(ManagerError::Guardrail {
            code: "host_capacity_exhausted",
            message: format!(
                "Host runner limit reached ({projected_runners}/{} including reservations).",
                limits.max_runners
            ),
        });
    }
    let projected_cpu = active.cpu + reserved.cpu + requested.cpu;
    if projected_cpu > limits.cpu_budget + f64::EPSILON {
        return Err(ManagerError::Guardrail {
            code: "host_capacity_exhausted",
            message: format!(
                "Host CPU budget would be exceeded ({projected_cpu:.2}/{:.2} CPUs including reservations).",
                limits.cpu_budget
            ),
        });
    }
    let projected_memory = active
        .memory_mb
        .saturating_add(reserved.memory_mb)
        .saturating_add(requested.memory_mb);
    if projected_memory > limits.memory_budget_mb {
        return Err(ManagerError::Guardrail {
            code: "host_capacity_exhausted",
            message: format!(
                "Host memory budget would be exceeded ({projected_memory}/{} MB including reservations).",
                limits.memory_budget_mb
            ),
        });
    }
    Ok(())
}

fn valid_log_size(value: &str) -> bool {
    let Some((number, suffix)) = value.split_at_checked(value.len().saturating_sub(1)) else {
        return false;
    };
    matches!(suffix, "k" | "m" | "g") && number.parse::<u64>().is_ok_and(|size| size > 0)
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
            vec![],
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
                ],
            ))
        }
        _ => Err(ManagerError::BadRequest("Runner mode is invalid.".into())),
    }
}

fn instrumented_runner_script(setup: &str, launch: &str) -> String {
    format!(
        r#"{setup}
# The runner listener writes verbose transport diagnostics to stdout/stderr even
# when its explicit diagnostic-output flag is absent. Keep those diagnostics inside the
# container; GridOps' user-facing stream is populated exclusively from the
# secret-masked job console pages below.
runner_diagnostics=/tmp/gridops-runner-diagnostics.log
{launch} >"$runner_diagnostics" 2>&1 &
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

fn is_tart_id(id: &str) -> bool {
    id.strip_prefix("tart-").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
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
            provider: "docker".into(),
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
            capacity_lease: "capacity-lease-1".into(),
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
    fn enforces_aggregate_runner_budgets() {
        let limits = ManagerLimits {
            available_cpus: 10.0,
            total_memory_mb: 8_192,
            cpu_budget: 6.0,
            memory_budget_mb: 6_144,
            max_runners: 3,
            min_free_disk_mb: 1_024,
            min_free_disk_percent: 10,
            runner_network: "gridops-runners".into(),
            log_max_size: "20m".into(),
            log_max_files: 5,
            runner_pids_limit: 1_024,
        };
        let disk = DiskCapacity {
            total_mb: 100_000,
            available_mb: 50_000,
            minimum_free_mb: 10_000,
        };
        let active = CapacityUsage {
            active_runners: 2,
            cpu: 4.0,
            memory_mb: 4_096,
        };
        let requested = CapacityUsage {
            active_runners: 1,
            cpu: 2.0,
            memory_mb: 2_048,
        };
        assert!(
            enforce_capacity(&limits, active, CapacityUsage::default(), requested, disk).is_ok()
        );
        let over_budget = CapacityUsage {
            active_runners: 1,
            cpu: 2.0,
            memory_mb: 2_049,
        };
        assert!(matches!(
            enforce_capacity(&limits, active, CapacityUsage::default(), over_budget, disk),
            Err(ManagerError::Guardrail {
                code: "host_capacity_exhausted",
                ..
            })
        ));
    }

    #[test]
    fn validates_explicit_log_rotation_sizes() {
        assert!(valid_log_size("20m"));
        assert!(valid_log_size("1g"));
        assert!(!valid_log_size("20"));
        assert!(!valid_log_size("0m"));
        assert!(!valid_log_size("twentym"));
    }

    #[test]
    fn parses_portable_df_capacity_output() {
        let output = "Filesystem 1024-blocks Used Available Capacity Mounted on\noverlay 104857600 52428800 52428800 50% /\n";
        assert_eq!(parse_df_capacity(output), Some((102_400, 51_200)));
        assert_eq!(parse_df_capacity("Filesystem 1024-blocks\ninvalid"), None);
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
        assert!(metadata.contains("gridops-runner-diagnostics.log"));
        assert!(!metadata.contains("ACTIONS_RUNNER_PRINT_LOG_TO_STDOUT"));
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
        assert!(metadata.contains("gridops-runner-diagnostics.log"));
        assert!(!metadata.contains("ACTIONS_RUNNER_PRINT_LOG_TO_STDOUT"));
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
