use std::{
    collections::HashMap,
    env,
    path::{Path as FsPath, PathBuf},
    process::Stdio,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequestParts, Path, Query, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use chrono::{SecondsFormat, Utc};
use futures_util::{StreamExt as _, stream};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq as _;
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _},
    process::Command,
    sync::{Mutex, RwLock},
};
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

#[derive(Clone)]
struct AgentState {
    token: SecretString,
    tart_binary: PathBuf,
    home: PathBuf,
    runner_root: String,
    network_mode: NetworkMode,
    limits: AgentLimits,
    records: Arc<RwLock<HashMap<String, TartRecord>>>,
    provision_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum NetworkMode {
    Nat,
    Softnet,
}

#[derive(Clone, Debug)]
struct AgentLimits {
    cpu_budget: f64,
    memory_budget_mb: i64,
    max_runners: usize,
    min_free_disk_mb: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TartRecord {
    id: String,
    runner_id: String,
    pool_id: String,
    name: String,
    vm_name: String,
    image: String,
    cpu_limit: f64,
    memory_limit_mb: i64,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(non_snake_case)]
struct TartVm {
    Name: String,
    State: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisionRunner {
    runner_id: String,
    pool_id: String,
    name: String,
    image: String,
    mode: String,
    provider: String,
    jit_config: Option<String>,
    cpu_limit: f64,
    memory_limit_mb: i64,
}

impl ProvisionRunner {
    fn validate(&self) -> Result<(), AgentError> {
        if self.provider != "tart" || self.mode != "ephemeral" {
            return Err(AgentError::BadRequest(
                "The Tart provider supports ephemeral macOS runners only.".into(),
            ));
        }
        if self.runner_id.is_empty()
            || self.runner_id.len() > 128
            || self.pool_id.is_empty()
            || self.pool_id.len() > 128
            || !valid_name(&self.name)
            || self.image.trim().is_empty()
            || self.image.len() > 512
        {
            return Err(AgentError::BadRequest(
                "Runner identity or Tart image is invalid.".into(),
            ));
        }
        if self.jit_config.as_ref().is_none_or(|value| {
            !(20..=900_000).contains(&value.len()) || value.contains(['\n', '\r'])
        }) {
            return Err(AgentError::BadRequest(
                "Runner JIT configuration is invalid.".into(),
            ));
        }
        if !self.cpu_limit.is_finite()
            || self.cpu_limit <= 0.0
            || self.cpu_limit.fract() != 0.0
            || self.memory_limit_mb <= 0
        {
            return Err(AgentError::BadRequest(
                "Tart runners require whole CPU cores and positive memory.".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    #[serde(default)]
    follow: bool,
    tail: Option<String>,
}

struct AgentAuth;

impl FromRequestParts<AgentState> for AgentAuth {
    type Rejection = AgentError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AgentState,
    ) -> Result<Self, Self::Rejection> {
        let supplied = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(AgentError::Unauthorized)?;
        let expected = state.token.expose_secret();
        if supplied.len() != expected.len()
            || !bool::from(supplied.as_bytes().ct_eq(expected.as_bytes()))
        {
            return Err(AgentError::Unauthorized);
        }
        Ok(Self)
    }
}

#[derive(Debug)]
enum AgentError {
    Unauthorized,
    BadRequest(String),
    Conflict(String),
    Guardrail { code: &'static str, message: String },
    NotFound(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AgentError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized".into(),
            ),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            Self::Conflict(message) => (StatusCode::CONFLICT, "conflict", message),
            Self::Guardrail { code, message } => (StatusCode::TOO_MANY_REQUESTS, code, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, "not_found", message),
            Self::Internal(error) => {
                tracing::error!(error = ?error, "Tart agent request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Tart agent request failed.".into(),
                )
            }
        };
        (status, Json(json!({ "code": code, "error": message }))).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gridops_tart_agent=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    anyhow::ensure!(
        env::consts::OS == "macos",
        "gridops-tart-agent must run on macOS"
    );
    let token = load_agent_token().await?;
    let tart_binary = env::var_os("GRIDOPS_TART_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/tart"));
    let home = env::var_os("GRIDOPS_TART_AGENT_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".gridops/tart-agent")
        });
    let runner_root = env::var("GRIDOPS_TART_RUNNER_ROOT")
        .unwrap_or_else(|_| "/Users/admin/actions-runner".into());
    anyhow::ensure!(
        valid_guest_path(&runner_root),
        "GRIDOPS_TART_RUNNER_ROOT is invalid"
    );
    let network_mode = match env::var("GRIDOPS_TART_NETWORK_MODE")
        .unwrap_or_else(|_| "softnet".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "nat" => NetworkMode::Nat,
        "softnet" => NetworkMode::Softnet,
        _ => anyhow::bail!("GRIDOPS_TART_NETWORK_MODE must be nat or softnet"),
    };
    let limits = AgentLimits {
        cpu_budget: env_value("GRIDOPS_TART_CPU_BUDGET", 4.0)?,
        memory_budget_mb: env_value("GRIDOPS_TART_MEMORY_BUDGET_MB", 6_144)?,
        max_runners: env_value("GRIDOPS_TART_MAX_RUNNERS", 1)?,
        min_free_disk_mb: env_value("GRIDOPS_TART_MIN_FREE_DISK_MB", 40_960)?,
    };
    anyhow::ensure!(
        limits.cpu_budget >= 1.0,
        "GRIDOPS_TART_CPU_BUDGET must be positive"
    );
    anyhow::ensure!(
        limits.memory_budget_mb >= 2_048,
        "GRIDOPS_TART_MEMORY_BUDGET_MB is too small"
    );
    anyhow::ensure!(
        (1..=100).contains(&limits.max_runners),
        "GRIDOPS_TART_MAX_RUNNERS must be 1-100"
    );

    fs::create_dir_all(home.join("records")).await?;
    fs::create_dir_all(home.join("logs")).await?;
    let version = tart_output(&tart_binary, ["--version"]).await?;
    let records = load_records(&home).await?;
    let state = AgentState {
        token: SecretString::from(token),
        tart_binary,
        home,
        runner_root,
        network_mode,
        limits,
        records: Arc::new(RwLock::new(records)),
        provision_lock: Arc::new(Mutex::new(())),
    };
    let bind = env::var("GRIDOPS_TART_AGENT_BIND").unwrap_or_else(|_| "127.0.0.1:8790".into());
    tracing::info!(%bind, tart_version = version.trim(), ?network_mode, "GridOps Tart agent starting");

    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/runners", get(list_runners).post(provision_runner))
        .route("/v1/runners/{runner_id}", delete(delete_runner))
        .route("/v1/runners/{runner_id}/logs", get(logs))
        .route("/v1/runners/{runner_id}/{action}", post(control_runner))
        .with_state(state)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_mins(45),
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn load_agent_token() -> Result<String> {
    if let Ok(token) = env::var("GRIDOPS_TART_AGENT_TOKEN")
        && !token.trim().is_empty()
    {
        anyhow::ensure!(
            token.len() >= 32,
            "Tart agent token must be at least 32 characters"
        );
        return Ok(token);
    }
    let path = env::var_os("GRIDOPS_TART_AGENT_TOKEN_FILE")
        .map(PathBuf::from)
        .context("GRIDOPS_TART_AGENT_TOKEN or GRIDOPS_TART_AGENT_TOKEN_FILE is required")?;
    let token = fs::read_to_string(&path)
        .await
        .with_context(|| format!("could not read {}", path.display()))?;
    let token = token.trim().to_owned();
    anyhow::ensure!(
        token.len() >= 32,
        "Tart agent token must be at least 32 characters"
    );
    Ok(token)
}

fn env_value<T>(name: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map_or(Ok(default), |value| {
            value.parse().with_context(|| format!("{name} is invalid"))
        })
}

async fn health(
    State(state): State<AgentState>,
    _auth: AgentAuth,
) -> Result<Json<Value>, AgentError> {
    let version = tart_output(&state.tart_binary, ["--version"])
        .await
        .map_err(AgentError::Internal)?;
    let vms = tart_vms(&state).await?;
    let active = active_capacity(&state, &vms).await;
    let disk = disk_capacity(&state).await?;
    Ok(Json(json!({
        "status": "ok",
        "provider": "tart",
        "tartVersion": version.trim(),
        "networkMode": state.network_mode,
        "capacity": {
            "cpuBudget": state.limits.cpu_budget,
            "memoryBudgetMb": state.limits.memory_budget_mb,
            "maxRunners": state.limits.max_runners,
            "active": active,
        },
        "disk": disk,
    })))
}

async fn list_runners(
    State(state): State<AgentState>,
    _auth: AgentAuth,
) -> Result<Json<Value>, AgentError> {
    let vms = tart_vms(&state).await?;
    let records = state.records.read().await;
    let runners = records
        .values()
        .map(|record| {
            let vm_state = vms
                .get(&record.vm_name)
                .map(String::as_str)
                .unwrap_or("missing");
            let runtime_state = match vm_state {
                "running" => "running",
                "suspended" => "paused",
                "stopped" => "exited",
                _ => "missing",
            };
            json!({
                "id": record.id,
                "names": [record.name],
                "image": record.image,
                "state": runtime_state,
                "status": vm_state,
                "labels": {
                    "io.gridops.managed": "true",
                    "io.gridops.runner-id": record.runner_id,
                    "io.gridops.pool-id": record.pool_id,
                    "io.gridops.mode": "ephemeral",
                    "io.gridops.provider": "tart",
                },
                "createdAt": record.created_at,
                "provider": "tart",
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "runners": runners })))
}

async fn provision_runner(
    State(state): State<AgentState>,
    _auth: AgentAuth,
    Json(mut input): Json<ProvisionRunner>,
) -> Result<(StatusCode, Json<Value>), AgentError> {
    input.validate()?;
    let _guard = state.provision_lock.lock().await;
    let vms = tart_vms(&state).await?;
    enforce_capacity(&state, &vms, input.cpu_limit, input.memory_limit_mb).await?;
    if state
        .records
        .read()
        .await
        .values()
        .any(|record| record.name == input.name)
    {
        return Err(AgentError::Conflict(
            "A Tart runner with this name already exists.".into(),
        ));
    }

    let id = format!("tart-{}", uuid::Uuid::new_v4().simple());
    let vm_name = format!("gridops-{}", input.name);
    if vms.contains_key(&vm_name) {
        return Err(AgentError::Conflict(
            "The Tart VM name is already in use.".into(),
        ));
    }
    let record = TartRecord {
        id: id.clone(),
        runner_id: input.runner_id.clone(),
        pool_id: input.pool_id.clone(),
        name: input.name.clone(),
        vm_name: vm_name.clone(),
        image: input.image.clone(),
        cpu_limit: input.cpu_limit,
        memory_limit_mb: input.memory_limit_mb,
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    };

    let result = async {
        tart_status(
            &state.tart_binary,
            ["clone", input.image.as_str(), vm_name.as_str()],
        )
        .await?;
        let cpu = format!("{:.0}", input.cpu_limit);
        let memory = input.memory_limit_mb.to_string();
        tart_status(
            &state.tart_binary,
            [
                "set",
                vm_name.as_str(),
                "--cpu",
                cpu.as_str(),
                "--memory",
                memory.as_str(),
            ],
        )
        .await?;
        persist_record(&state, &record).await?;
        state
            .records
            .write()
            .await
            .insert(id.clone(), record.clone());
        start_vm(&state, &record)?;
        wait_for_guest_agent(&state, &record).await?;
        let jit_config = SecretString::from(
            input
                .jit_config
                .take()
                .context("validated JIT configuration disappeared")?,
        );
        start_runner(&state, &record, &jit_config).await?;
        Result::<()>::Ok(())
    }
    .await;

    if let Err(error) = result {
        tracing::error!(runner_id = %id, vm = %vm_name, error = ?error, "Tart runner provisioning failed");
        cleanup_vm(&state, &vm_name).await;
        state.records.write().await.remove(&id);
        let _ = fs::remove_file(record_path(&state, &id)).await;
        return Err(AgentError::Internal(error));
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": input.name,
            "state": "running",
            "createdAt": record.created_at,
        })),
    ))
}

async fn control_runner(
    State(state): State<AgentState>,
    Path((runner_id, action)): Path<(String, String)>,
    _auth: AgentAuth,
) -> Result<Json<Value>, AgentError> {
    let record = record(&state, &runner_id).await?;
    match action.as_str() {
        "stop" => {
            let _ = tart_status(&state.tart_binary, ["stop", record.vm_name.as_str()]).await;
            Ok(Json(json!({ "status": "stopped" })))
        }
        "start" | "restart" | "pause" | "resume" => Err(AgentError::Conflict(
            "Ephemeral Tart runners cannot be restarted or suspended; rebuild the runner instead."
                .into(),
        )),
        _ => Err(AgentError::BadRequest("Runner action is invalid.".into())),
    }
}

async fn delete_runner(
    State(state): State<AgentState>,
    Path(runner_id): Path<String>,
    _auth: AgentAuth,
) -> Result<Json<Value>, AgentError> {
    let record = record(&state, &runner_id).await?;
    cleanup_vm(&state, &record.vm_name).await;
    state.records.write().await.remove(&runner_id);
    match fs::remove_file(record_path(&state, &runner_id)).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AgentError::Internal(error.into())),
    }
    Ok(Json(json!({ "status": "deleted" })))
}

async fn logs(
    State(state): State<AgentState>,
    Path(runner_id): Path<String>,
    Query(query): Query<LogsQuery>,
    _auth: AgentAuth,
) -> Result<Response, AgentError> {
    record(&state, &runner_id).await?;
    let tail = query.tail.unwrap_or_else(|| "500".into());
    if tail != "all" && !tail.parse::<usize>().is_ok_and(|value| value <= 100_000) {
        return Err(AgentError::BadRequest(
            "Log tail must be all or a number from 0 to 100000.".into(),
        ));
    }
    let path = log_path(&state, &runner_id);
    let initial = read_log_tail(&path, &tail).await?;
    if !query.follow {
        return Ok(text_response(Body::from(initial)));
    }
    let offset = fs::metadata(&path)
        .await
        .map_or(0, |metadata| metadata.len());
    let follow = stream::unfold((path, offset), |(path, mut offset)| async move {
        loop {
            if let Ok(mut file) = File::open(&path).await
                && file.seek(std::io::SeekFrom::Start(offset)).await.is_ok()
            {
                let mut bytes = Vec::new();
                if file.read_to_end(&mut bytes).await.is_ok() && !bytes.is_empty() {
                    offset = offset.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                    return Some((Ok::<_, std::io::Error>(bytes), (path, offset)));
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    let initial_stream = stream::once(async move { Ok::<_, std::io::Error>(initial) });
    Ok(text_response(Body::from_stream(
        initial_stream.chain(follow),
    )))
}

fn text_response(body: Body) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response()
}

fn start_vm(state: &AgentState, record: &TartRecord) -> Result<()> {
    let runtime_log = state
        .home
        .join("logs")
        .join(format!("{}.runtime.log", record.id));
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&runtime_log)?;
    let stderr = stdout.try_clone()?;
    let mut command = Command::new(&state.tart_binary);
    command
        .arg("run")
        .arg("--no-graphics")
        .arg("--no-audio")
        .arg("--no-clipboard");
    if matches!(state.network_mode, NetworkMode::Softnet) {
        command.arg("--net-softnet");
    }
    let mut child = command
        .arg(&record.vm_name)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let vm_name = record.vm_name.clone();
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::info!(%vm_name, %status, "Tart VM process exited"),
            Err(error) => {
                tracing::warn!(%vm_name, error = ?error, "Tart VM process wait failed");
            }
        }
    });
    Ok(())
}

async fn wait_for_guest_agent(state: &AgentState, record: &TartRecord) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_mins(3);
    loop {
        let ready = Command::new(&state.tart_binary)
            .args(["exec", record.vm_name.as_str(), "/usr/bin/true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());
        if ready {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("Tart guest agent did not become ready within 180 seconds");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn start_runner(
    state: &AgentState,
    record: &TartRecord,
    jit_config: &SecretString,
) -> Result<()> {
    let script = runner_script(&state.runner_root);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(state, &record.id))?;
    let error_log = log.try_clone()?;
    let mut child = Command::new(&state.tart_binary)
        .args([
            "exec",
            "-i",
            record.vm_name.as_str(),
            "/bin/bash",
            "-lc",
            script.as_str(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log))
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .context("Tart runner stdin was unavailable")?;
    stdin
        .write_all(jit_config.expose_secret().as_bytes())
        .await?;
    stdin.write_all(b"\n").await?;
    stdin.shutdown().await?;
    let tart_binary = state.tart_binary.clone();
    let vm_name = record.vm_name.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        tracing::info!(%vm_name, ?status, "macOS runner process exited");
        let _ = Command::new(tart_binary)
            .args(["stop", vm_name.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    });
    Ok(())
}

fn runner_script(runner_root: &str) -> String {
    format!(
        r#"set -euo pipefail
runner_root='{runner_root}'
if [ ! -x "$runner_root/run.sh" ]; then
  printf 'GridOps macOS runner image is missing %s/run.sh. Prepare the Tart base image first.\n' "$runner_root" >&2
  exit 78
fi
cd "$runner_root"
IFS= read -r GRIDOPS_BOOTSTRAP_SECRET
[ -n "$GRIDOPS_BOOTSTRAP_SECRET" ]
runner_args=(--jitconfig "$GRIDOPS_BOOTSTRAP_SECRET")
unset GRIDOPS_BOOTSTRAP_SECRET
runner_diagnostics="$runner_root/_diag/gridops-runner-diagnostics.log"
mkdir -p "$runner_root/_diag"
./run.sh "${{runner_args[@]}}" >"$runner_diagnostics" 2>&1 &
runner_pid=$!
trap 'kill -TERM "$runner_pid" 2>/dev/null || true' TERM INT
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
      if [ "$offset" -eq 0 ]; then printf '\n[GRIDOPS JOB LOG %s]\n' "$(basename "$job_log")"; fi
      head -c "$size" "$job_log" 2>/dev/null | tail -c "+$((offset + 1))" || true
      printf '%s\n' "$size" > "$state_file"
    fi
  done
}}
(
  while kill -0 "$runner_pid" 2>/dev/null; do flush_job_logs; sleep 0.1; done
  flush_job_logs
) &
forwarder_pid=$!
set +e
wait "$runner_pid"
runner_status=$?
set -e
wait "$forwarder_pid" 2>/dev/null || true
exit "$runner_status""#,
    )
}

async fn enforce_capacity(
    state: &AgentState,
    vms: &HashMap<String, String>,
    cpu: f64,
    memory_mb: i64,
) -> Result<(), AgentError> {
    let active = active_capacity(state, vms).await;
    let disk = disk_capacity(state).await?;
    if disk.available_mb < disk.minimum_free_mb {
        return Err(AgentError::Guardrail {
            code: "host_disk_guardrail",
            message: format!(
                "Tart provisioning stopped because only {} MB disk space remains; {} MB is reserved.",
                disk.available_mb, disk.minimum_free_mb
            ),
        });
    }
    if active.active_runners.saturating_add(1) > state.limits.max_runners
        || active.cpu + cpu > state.limits.cpu_budget + f64::EPSILON
        || active.memory_mb.saturating_add(memory_mb) > state.limits.memory_budget_mb
    {
        return Err(AgentError::Guardrail {
            code: "host_capacity_exhausted",
            message: format!(
                "Tart host budget would be exceeded (active: {} runners, {:.0} CPU, {} MB; requested: {:.0} CPU, {} MB).",
                active.active_runners, active.cpu, active.memory_mb, cpu, memory_mb
            ),
        });
    }
    Ok(())
}

async fn active_capacity(state: &AgentState, vms: &HashMap<String, String>) -> CapacityUsage {
    state
        .records
        .read()
        .await
        .values()
        .filter(|record| {
            vms.get(&record.vm_name)
                .is_some_and(|status| matches!(status.as_str(), "running" | "suspended"))
        })
        .fold(CapacityUsage::default(), |mut usage, record| {
            usage.active_runners += 1;
            usage.cpu += record.cpu_limit;
            usage.memory_mb = usage.memory_mb.saturating_add(record.memory_limit_mb);
            usage
        })
}

async fn tart_vms(state: &AgentState) -> Result<HashMap<String, String>, AgentError> {
    let output = Command::new(&state.tart_binary)
        .args(["list", "--source", "local", "--format", "json"])
        .output()
        .await
        .map_err(|error| AgentError::Internal(error.into()))?;
    if !output.status.success() {
        return Err(AgentError::Internal(anyhow::anyhow!(
            "tart list failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1_000)
                .collect::<String>()
        )));
    }
    let vms = serde_json::from_slice::<Vec<TartVm>>(&output.stdout)
        .map_err(|error| AgentError::Internal(error.into()))?;
    Ok(vms.into_iter().map(|vm| (vm.Name, vm.State)).collect())
}

async fn disk_capacity(state: &AgentState) -> Result<DiskCapacity, AgentError> {
    let output = Command::new("/bin/df")
        .args(["-Pk", state.home.to_string_lossy().as_ref()])
        .output()
        .await
        .map_err(|error| AgentError::Internal(error.into()))?;
    if !output.status.success() {
        return Err(AgentError::Internal(anyhow::anyhow!(
            "could not inspect Tart disk capacity"
        )));
    }
    let stdout =
        String::from_utf8(output.stdout).map_err(|error| AgentError::Internal(error.into()))?;
    let row = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .nth(1)
        .ok_or_else(|| AgentError::Internal(anyhow::anyhow!("Tart disk capacity was invalid")))?;
    let columns = row.split_whitespace().collect::<Vec<_>>();
    let total_mb = columns
        .get(1)
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value / 1_024)
        .ok_or_else(|| {
            AgentError::Internal(anyhow::anyhow!("Tart total disk capacity was invalid"))
        })?;
    let available_mb = columns
        .get(3)
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| value / 1_024)
        .ok_or_else(|| {
            AgentError::Internal(anyhow::anyhow!("Tart available disk capacity was invalid"))
        })?;
    Ok(DiskCapacity {
        total_mb,
        available_mb,
        minimum_free_mb: state.limits.min_free_disk_mb,
    })
}

async fn cleanup_vm(state: &AgentState, vm_name: &str) {
    let _ = tart_status(&state.tart_binary, ["stop", vm_name]).await;
    let _ = tart_status(&state.tart_binary, ["delete", vm_name]).await;
}

async fn record(state: &AgentState, id: &str) -> Result<TartRecord, AgentError> {
    if !valid_id(id) {
        return Err(AgentError::BadRequest(
            "Tart runner identifier is invalid.".into(),
        ));
    }
    state
        .records
        .read()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| AgentError::NotFound("Managed Tart runner was not found.".into()))
}

async fn load_records(home: &FsPath) -> Result<HashMap<String, TartRecord>> {
    let mut records = HashMap::new();
    let mut entries = fs::read_dir(home.join("records")).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let record = serde_json::from_slice::<TartRecord>(&fs::read(entry.path()).await?)?;
        if valid_id(&record.id) {
            records.insert(record.id.clone(), record);
        }
    }
    Ok(records)
}

async fn persist_record(state: &AgentState, record: &TartRecord) -> Result<()> {
    let path = record_path(state, &record.id);
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(record)?).await?;
    fs::rename(temporary, path).await?;
    Ok(())
}

fn record_path(state: &AgentState, id: &str) -> PathBuf {
    state.home.join("records").join(format!("{id}.json"))
}

fn log_path(state: &AgentState, id: &str) -> PathBuf {
    state.home.join("logs").join(format!("{id}.log"))
}

async fn read_log_tail(path: &FsPath, tail: &str) -> Result<Vec<u8>, AgentError> {
    let bytes = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(AgentError::Internal(error.into())),
    };
    if tail == "all" {
        return Ok(bytes);
    }
    let lines = tail.parse::<usize>().unwrap_or(500);
    if lines == 0 {
        return Ok(Vec::new());
    }
    let start = bytes
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(lines)
        .map_or(0, |(index, _)| index + 1);
    Ok(bytes[start..].to_vec())
}

async fn tart_status<const N: usize>(binary: &FsPath, args: [&str; N]) -> Result<()> {
    let output = Command::new(binary).args(args).output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "Tart command failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(2_000)
                .collect::<String>()
        );
    }
    Ok(())
}

async fn tart_output<const N: usize>(binary: &FsPath, args: [&str; N]) -> Result<String> {
    let output = Command::new(binary).args(args).output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "Tart command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_id(value: &str) -> bool {
    value.strip_prefix("tart-").is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn valid_guest_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tart_runtime_identifiers() {
        assert!(valid_id("tart-0123456789abcdef0123456789abcdef"));
        assert!(!valid_id("0123456789abcdef"));
        assert!(valid_name("macos-ci-1234abcd"));
        assert!(!valid_name("macos ci"));
    }

    #[test]
    fn generated_runner_script_keeps_jit_secret_off_command_line() {
        let script = runner_script("/Users/admin/actions-runner");
        assert!(script.contains("read -r GRIDOPS_BOOTSTRAP_SECRET"));
        assert!(script.contains("mkdir -p \"$runner_root/_diag\""));
        assert!(script.contains("_diag/pages/*.log"));
        assert!(!script.contains("fake-jit"));
    }

    #[tokio::test]
    async fn tails_complete_lines() -> Result<()> {
        let path = env::temp_dir().join(format!("gridops-tart-log-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"one\ntwo\nthree\n").await?;
        let Ok(tail) = read_log_tail(&path, "2").await else {
            anyhow::bail!("log tail failed")
        };
        assert_eq!(tail, b"two\nthree\n");
        fs::remove_file(path).await?;
        Ok(())
    }
}
