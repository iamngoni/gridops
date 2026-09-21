use std::{
    collections::{HashMap, HashSet},
    io::SeekFrom,
    path::{Path as FilePath, PathBuf},
    time::Duration,
};

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{SecondsFormat, Utc};
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use gridops_core::{
    ConfigurationState, CreateRunnerPool, GitHubRepository, GitHubWorkflowJob, GitHubWorkflowStep,
    JitRequest, RepositoryCapacity, RepositoryPage, RunnerTarget, UpdateRunnerPool,
    compatible_runner_provider, effective_runner_labels, next_runner_provider,
    next_runner_repository, now_millis,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{FromRow, Row as _};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::{
    auth::{
        AuthUser, OptionalAuth, assert_installation_admin, assert_pool_admin, assert_same_origin,
        audit, require_system_admin,
    },
    error::{ApiError, ApiResult},
    oauth::{control_token, upsert_repository},
    state::AppState,
};

#[path = "resources/backups.rs"]
mod backups;
#[path = "resources/logs.rs"]
mod logs;
#[path = "resources/observability.rs"]
mod observability;
#[path = "resources/overview.rs"]
mod overview;
#[path = "resources/repositories.rs"]
mod repositories;
#[path = "resources/runner_pools.rs"]
mod runner_pools;
#[path = "resources/runners.rs"]
mod runners;
#[path = "resources/settings.rs"]
mod settings;
#[path = "resources/webhooks.rs"]
mod webhooks;
#[path = "resources/workflows.rs"]
mod workflows;

pub use backups::database_backup;
pub use logs::{
    archived_logs, runner_log_stream, runner_logs, workflow_job_log_view, workflow_run_logs,
};
pub use observability::{audit_events, log_targets};
pub use overview::{capacity_history, health, overview};
pub use repositories::{
    installation_repositories, installation_runner_groups, repositories, runner_pool_options,
    runner_pool_repository_options, search,
};
pub use runner_pools::{
    create_runner_pool, delete_runner_pool, runner_pool, runner_pool_action, runner_pool_events,
    runner_pools, update_runner_pool,
};
pub use runners::{runner_action, runners};
pub use settings::{bitbucket_connections, create_bitbucket_connection, save_settings, settings};
pub use webhooks::{webhook_deliveries, webhook_delivery};
pub use workflows::{workflow_run, workflow_run_action, workflow_runs};

const MAX_ARCHIVED_LOG_BYTES: i64 = 100 * 1_024 * 1_024;
const MAX_ARCHIVED_LOG_VIEW_BYTES: u64 = 1_000_000;
const MAX_STRUCTURED_LOG_BYTES: usize = 25 * 1_024 * 1_024;
const DEFAULT_PAGE_SIZE: i64 = 25;
const FALLBACK_MANAGER_CPU_LIMIT: i64 = 2;
const FALLBACK_MANAGER_MEMORY_LIMIT_MB: i64 = 2_048;

#[derive(Debug, FromRow)]
struct PoolAccess {
    installation_id: i64,
    installation_permission: String,
    account_login: String,
    repository_id: Option<i64>,
    repository_owner: Option<String>,
    repository_name: Option<String>,
    name: String,
    scope: String,
    mode: String,
    provider: String,
    providers: String,
    labels: String,
    image: String,
    docker_image: String,
    tart_image: String,
    macos_runtime: String,
    desired_count: i64,
    min_count: i64,
    max_count: i64,
    cpu_limit: f64,
    memory_limit_mb: i64,
    runner_group_id: i64,
    ephemeral: bool,
    paused: bool,
    state: String,
    autoscaling_enabled: bool,
    queue_scale_factor: i64,
    idle_timeout_minutes: i64,
    configuration_version: i64,
    provision_failure_count: i64,
    provision_retry_at: Option<i64>,
    provision_circuit_open: bool,
}

#[derive(Debug, FromRow)]
struct RunnerAccess {
    runner_id: String,
    runner_name: String,
    container_id: Option<String>,
    github_runner_id: Option<i64>,
    ci_platform: String,
    bitbucket_connection_id: Option<String>,
    bitbucket_runner_uuid: Option<String>,
    provider: String,
    runner_status: String,
    busy: bool,
    ephemeral: bool,
    configuration_version: i64,
    last_job_id: Option<i64>,
    pool_id: String,
    pool_name: String,
    installation_id: i64,
    account_login: String,
    target_repository_id: Option<i64>,
    repository_owner: Option<String>,
    repository_name: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PoolAction {
    action: String,
    #[serde(rename = "desiredCount")]
    desired_count: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct RunnerAction {
    action: String,
}

#[derive(Deserialize)]
pub(crate) struct WorkflowAction {
    action: String,
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    q: String,
}

#[derive(Deserialize)]
pub(crate) struct RepositoryQuery {
    q: Option<String>,
    page: Option<i64>,
    #[serde(rename = "perPage")]
    per_page: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct PaginationQuery {
    page: Option<i64>,
    #[serde(rename = "perPage")]
    per_page: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct LogTargetsQuery {
    page: Option<i64>,
    #[serde(rename = "perPage")]
    per_page: Option<i64>,
    target: Option<String>,
}

#[derive(Clone)]
struct InstallationAccess {
    id: i64,
    account_login: String,
    account_type: String,
    repository_selection: String,
}

struct AvailableRepository {
    installation: InstallationAccess,
    repository: GitHubRepository,
}

struct RepositoryStats {
    last_synced_at: i64,
    pool_count: i64,
    run_count: i64,
    last_run_at: Option<i64>,
}

#[derive(FromRow)]
struct BitbucketPoolConnection {
    id: String,
    workspace: String,
    workspace_uuid: String,
    access_token_key: String,
}

#[derive(Deserialize)]
pub(crate) struct CapacityQuery {
    window: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct LogStreamQuery {
    tail: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSettings {
    log_retention_days: i64,
    log_storage_budget_mb: i64,
    webhook_retention_days: i64,
    audit_retention_days: i64,
    reconcile_interval_seconds: i64,
    github_sync_interval_seconds: i64,
    auto_update_images: bool,
    provisioning_paused: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBitbucketConnection {
    name: String,
    workspace: String,
    access_token: String,
}

async fn manager_json(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> ApiResult<Value> {
    let token = state.manager_token().ok_or_else(|| {
        ApiError::ServiceUnavailable("Runner manager authentication is not configured.".into())
    })?;
    let mut request = state
        .http
        .request(method, state.manager_url(path).map_err(ApiError::Internal)?)
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| format!("Runner manager request failed ({status})."));
        return Err(if status == StatusCode::NOT_FOUND {
            ApiError::NotFound(message)
        } else if matches!(status, StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS) {
            ApiError::Conflict(message)
        } else {
            ApiError::ServiceUnavailable(message)
        });
    }
    serde_json::from_str(&text).map_err(|error| ApiError::Internal(error.into()))
}

async fn reserve_runner_capacity(
    state: &AppState,
    runner_id: &str,
    pool_id: &str,
    provider: &str,
    cpu_limit: f64,
    memory_limit_mb: i64,
) -> ApiResult<String> {
    let response = manager_json(
        state,
        Method::POST,
        "v1/admissions",
        Some(json!({
            "runnerId": runner_id,
            "poolId": pool_id,
            "provider": provider,
            "cpuLimit": cpu_limit,
            "memoryLimitMb": memory_limit_mb,
        })),
    )
    .await?;
    response
        .get("leaseId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ApiError::ServiceUnavailable(
                "Runner manager returned an invalid capacity reservation.".into(),
            )
        })
}

async fn release_runner_capacity(state: &AppState, lease_id: &str) {
    if let Err(error) = manager_json(
        state,
        Method::DELETE,
        &format!("v1/admissions/{lease_id}"),
        None,
    )
    .await
    {
        tracing::warn!(lease_id, error = ?error, "could not release runner capacity reservation");
    }
}

async fn manager_text(state: &AppState, path: &str) -> ApiResult<String> {
    Ok(manager_get(state, path).await?.text().await?)
}

async fn manager_get(state: &AppState, path: &str) -> ApiResult<reqwest::Response> {
    let token = state.manager_token().ok_or_else(|| {
        ApiError::ServiceUnavailable("Runner manager authentication is not configured.".into())
    })?;
    let response = state
        .http
        .get(state.manager_url(path).map_err(ApiError::Internal)?)
        .bearer_auth(token)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(ApiError::ServiceUnavailable(format!(
            "Runner manager request failed ({status}): {}",
            text.chars().take(300).collect::<String>()
        )));
    }
    Ok(response)
}

async fn archive_runner_logs(state: &AppState, runner: &RunnerAccess) -> ApiResult<Option<String>> {
    let Some(container_id) = runner.container_id.as_deref() else {
        return Ok(None);
    };
    if let Some(id) = sqlx::query_scalar::<_, String>(
        "SELECT id FROM log_streams WHERE runner_id=? AND source=? AND complete=1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&runner.runner_id)
    .bind(&runner.provider)
    .fetch_optional(&state.database)
    .await?
    {
        return Ok(Some(id));
    }

    tokio::fs::create_dir_all(state.config.log_directory())
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let stream_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{stream_id}.log");
    let path = safe_log_path(state, &filename)?;
    let response = manager_get(
        state,
        &format!("v1/runners/{container_id}/logs?tail=100000"),
    )
    .await?;
    let write_result = async {
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        let mut stream = response.bytes_stream();
        let mut checksum = Sha256::new();
        let mut size_bytes = 0_i64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let remaining = usize::try_from(MAX_ARCHIVED_LOG_BYTES - size_bytes).unwrap_or(0);
            if remaining == 0 {
                break;
            }
            let retained = &chunk[..chunk.len().min(remaining)];
            checksum.update(retained);
            size_bytes =
                size_bytes.saturating_add(i64::try_from(retained.len()).unwrap_or(i64::MAX));
            file.write_all(retained)
                .await
                .map_err(|error| ApiError::Internal(error.into()))?;
        }
        file.flush()
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        ApiResult::<(i64, String)>::Ok((size_bytes, hex::encode(checksum.finalize())))
    }
    .await;
    let (size_bytes, checksum) = match write_result {
        Ok(result) => result,
        Err(error) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error);
        }
    };
    let now = now_millis();
    let retention_days = setting_i64_value(state, "logRetentionDays", 30).await;
    let repository = runner
        .repository_owner
        .as_ref()
        .zip(runner.repository_name.as_ref())
        .map(|(owner, repository)| format!("{owner}/{repository}"));
    let inserted = sqlx::query(
        r#"INSERT INTO log_streams (
          id,runner_id,job_id,installation_id,runner_name,pool_name,repository,source,path,
          size_bytes,complete,checksum,expires_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,?,?, ?,1,?,?,?,?)"#,
    )
    .bind(&stream_id)
    .bind(&runner.runner_id)
    .bind(runner.last_job_id)
    .bind(runner.installation_id)
    .bind(&runner.runner_name)
    .bind(&runner.pool_name)
    .bind(repository)
    .bind(&runner.provider)
    .bind(&filename)
    .bind(size_bytes)
    .bind(checksum)
    .bind(now + retention_days * 86_400_000)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await;
    if let Err(error) = inserted {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    Ok(Some(stream_id))
}

fn safe_log_path(state: &AppState, filename: &str) -> ApiResult<PathBuf> {
    let candidate = FilePath::new(filename);
    if candidate.is_absolute()
        || candidate.components().count() != 1
        || candidate.file_name().and_then(|value| value.to_str()) != Some(filename)
        || filename.len() > 128
        || !filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ApiError::BadRequest("Archived log path is invalid.".into()));
    }
    Ok(state.config.log_directory().join(filename))
}

async fn configuration(state: &AppState) -> ApiResult<ConfigurationState> {
    Ok(ConfigurationState {
        github_oauth: state
            .github_oauth_credentials()
            .await
            .map_err(ApiError::Internal)?
            .is_some(),
        github_app_control: state
            .github_app_credentials()
            .await
            .map_err(ApiError::Internal)?
            .is_some(),
        webhook_active: state.config.webhook_delivery_active(),
        webhook_verification: state
            .github_webhook_secret()
            .await
            .map_err(ApiError::Internal)?
            .is_some(),
        secure_storage: state.config.session_secret().is_some()
            && state.config.encryption_key().is_some(),
        runner_manager: state.config.manager_token().is_some(),
        installation_tokens: state
            .github_app_credentials()
            .await
            .map_err(ApiError::Internal)?
            .is_some(),
        callback_url: state
            .config
            .base_url()
            .join("/auth/github/callback")
            .map_or_else(|_| "/auth/github/callback".into(), |url| url.to_string()),
        webhook_url: effective_webhook_url(
            state.config.base_url(),
            state.config.github_webhook_url(),
        ),
    })
}

fn effective_webhook_url(base_url: &url::Url, override_url: Option<&url::Url>) -> String {
    override_url.cloned().map_or_else(
        || {
            base_url
                .join("/api/webhooks/github")
                .map_or_else(|_| "/api/webhooks/github".into(), |url| url.to_string())
        },
        |url| url.to_string(),
    )
}

fn workflow_run_json(row: &sqlx::sqlite::SqliteRow, system_admin: bool) -> Value {
    json!({
        "id": row.get::<i64,_>("id"), "workflowName": row.get::<String,_>("workflow_name"), "runNumber": row.get::<i64,_>("run_number"),
        "runAttempt": row.get::<i64,_>("run_attempt"), "event": row.get::<String,_>("event"), "status": row.get::<String,_>("status"),
        "conclusion": row.try_get::<Option<String>,_>("conclusion").ok().flatten(), "headBranch": row.try_get::<Option<String>,_>("head_branch").ok().flatten(),
        "headSha": row.get::<String,_>("head_sha"), "actorLogin": row.try_get::<Option<String>,_>("actor_login").ok().flatten(),
        "htmlUrl": row.get::<String,_>("html_url"), "startedAt": iso_optional(row.try_get::<Option<i64>,_>("started_at").ok().flatten()),
        "completedAt": iso_optional(row.try_get::<Option<i64>,_>("completed_at").ok().flatten()), "createdAt": iso(row.get::<i64,_>("github_created_at")),
        "repository": row.get::<String,_>("full_name"), "jobCount": row.get::<i64,_>("job_count"),
        "activeJobs": row.get::<i64,_>("active_jobs"), "failedJobs": row.get::<i64,_>("failed_jobs"),
        "canManage": system_admin || row.get::<String,_>("installation_permission") == "admin",
    })
}

async fn diagnose_queued_job(
    state: &AppState,
    user: &AuthUser,
    repository_id: i64,
    installation_id: i64,
    requested_labels: &[String],
) -> ApiResult<Value> {
    let rows = sqlx::query(
        r#"SELECT p.id,p.name,p.labels,p.providers,p.paused,p.provision_circuit_open,p.max_count,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('starting','online','idle','busy','paused','stopped') THEN 1 END) AS active_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN 1 END) AS busy_runners
          FROM runner_pools p
          LEFT JOIN runners r ON r.pool_id=p.id
          WHERE (
            EXISTS (SELECT 1 FROM runner_pool_repositories membership
              WHERE membership.pool_id=p.id AND membership.repository_id=?)
            OR p.repository_id=?
            OR (p.scope='organization' AND p.installation_id=?)
          )
          AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
          GROUP BY p.id ORDER BY p.name"#,
    )
    .bind(repository_id)
    .bind(repository_id)
    .bind(installation_id)
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let mut compatible = 0_i64;
    let mut ready = 0_i64;
    let candidates = rows
        .iter()
        .map(|row| {
            let pool_name = row.get::<String, _>("name");
            let mut configured_labels = json_array(row.get::<&str, _>("labels"));
            if !configured_labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(&pool_name))
            {
                configured_labels.push(pool_name.clone());
            }
            let providers = json_array(row.get::<&str, _>("providers"));
            let matching_provider =
                compatible_runner_provider(&providers, requested_labels, &configured_labels);
            let active = row.get::<i64, _>("active_runners");
            let maximum = row.get::<i64, _>("max_count");
            let paused = row.get::<bool, _>("paused");
            let status = if paused {
                "paused"
            } else if matching_provider.is_none() {
                "incompatible"
            } else if row.get::<bool, _>("provision_circuit_open") {
                "circuit_open"
            } else if active >= maximum {
                "at_capacity"
            } else {
                "ready"
            };
            if matching_provider.is_some() && !paused && status != "circuit_open" {
                compatible += 1;
                if status == "ready" {
                    ready += 1;
                }
            }
            let available_capacity = maximum.saturating_sub(active);
            json!({
                "id": row.get::<String, _>("id"), "name": pool_name,
                "status": status, "provider": matching_provider,
                "activeRunners": active, "busyRunners": row.get::<i64, _>("busy_runners"),
                "maximumRunners": maximum, "availableCapacity": available_capacity,
            })
        })
        .collect::<Vec<_>>();
    let summary = if candidates.is_empty() {
        "No accessible runner pool is assigned to this repository.".to_owned()
    } else if compatible == 0 {
        if candidates
            .iter()
            .any(|candidate| candidate["status"] == "circuit_open")
        {
            "A matching pool needs a provisioning retry before GridOps can start another runner."
                .to_owned()
        } else {
            "No assigned pool matches every requested runner label.".to_owned()
        }
    } else if ready == 0 {
        "A matching pool exists, but every compatible pool is currently at its configured limit."
            .to_owned()
    } else {
        "A matching pool has configured headroom; GridOps will attempt capacity admission on its next reconcile."
            .to_owned()
    };
    Ok(json!({
        "summary": summary,
        "requestedLabels": requested_labels,
        "candidates": candidates,
    }))
}

fn workflow_action_endpoint(action: &str) -> ApiResult<&'static str> {
    match action {
        "cancel" => Ok("cancel"),
        "force-cancel" => Ok("force-cancel"),
        "rerun" => Ok("rerun"),
        "rerun-failed" => Ok("rerun-failed-jobs"),
        _ => Err(ApiError::BadRequest("Workflow action is invalid.".into())),
    }
}

fn empty_paginated_page(page: i64, per_page: i64) -> Json<Value> {
    Json(json!({
        "authenticated": false,
        "items": [],
        "total": 0,
        "page": page,
        "perPage": per_page,
    }))
}

fn paginated_page(items: &[Value], total: i64, page: i64, per_page: i64) -> Json<Value> {
    Json(json!({
        "authenticated": true,
        "items": items,
        "total": total,
        "page": page,
        "perPage": per_page,
    }))
}

fn pagination(page: Option<i64>, per_page: Option<i64>) -> (i64, i64) {
    let page = query_page(page);
    let per_page = per_page.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, 100);
    (page, per_page)
}

fn bounded_pagination(requested_page: i64, total: i64, per_page: i64) -> (i64, i64) {
    let total_pages = total.saturating_add(per_page - 1) / per_page;
    let page = requested_page.min(total_pages.max(1));
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, offset)
}

fn query_page(page: Option<i64>) -> i64 {
    page.unwrap_or(1).clamp(1, 1_000_000)
}

fn like_pattern(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn json_array(value: &str) -> Vec<String> {
    serde_json::from_str(value).unwrap_or_default()
}

fn normalized_pool_labels(name: &str, additional: &[String]) -> ApiResult<Vec<String>> {
    let mut labels = additional.to_vec();
    labels.push(name.to_owned());
    labels.sort();
    labels.dedup();
    if labels.len() > 20 {
        return Err(ApiError::BadRequest(
            "A runner pool can have at most 20 labels including its pool-name label.".into(),
        ));
    }
    Ok(labels)
}

fn runner_pool_defaults(image: &str, max_cpu_limit: i64, max_memory_limit_mb: i64) -> Value {
    let tart_image = default_tart_image();
    json!({
        "provider": "docker", "providers": ["docker"], "image": image,
        "dockerImage": image, "tartImage": tart_image, "macosRuntime": "vm",
        "labels": ["gridops"], "cpuLimit": 2,
        "memoryLimitMb": 2048, "desiredCount": 1, "minCount": 0, "maxCount": 10,
        "autoscalingEnabled": true, "queueScaleFactor": 1, "idleTimeoutMinutes": 5,
        "runnerGroupId": 1, "maxCpuLimit": max_cpu_limit, "maxMemoryLimitMb": max_memory_limit_mb,
    })
}

fn default_tart_image() -> String {
    std::env::var("GRIDOPS_TART_RUNNER_IMAGE").unwrap_or_else(|_| "gridops-macos-tahoe-base".into())
}

async fn manager_resource_capacity(state: &AppState) -> (i64, i64) {
    let manager = manager_json(state, Method::GET, "v1/health", None)
        .await
        .ok();
    let cpu = manager
        .as_ref()
        .and_then(|value| {
            value
                .get("capacity")
                .and_then(|capacity| capacity.get("cpuBudget"))
                .and_then(Value::as_f64)
                .map(|cpus| cpus.floor() as i64)
                .filter(|cpus| *cpus > 0)
        })
        .unwrap_or(FALLBACK_MANAGER_CPU_LIMIT)
        .clamp(1, FALLBACK_MANAGER_CPU_LIMIT);
    let memory = manager
        .as_ref()
        .and_then(|value| {
            value
                .get("capacity")
                .and_then(|capacity| capacity.get("memoryBudgetMb"))
                .and_then(Value::as_i64)
                .filter(|memory| *memory >= 256)
        })
        .unwrap_or(FALLBACK_MANAGER_MEMORY_LIMIT_MB)
        .clamp(256, FALLBACK_MANAGER_MEMORY_LIMIT_MB);
    (cpu, memory)
}

fn capacity_window(window: &str) -> Option<(i64, i64)> {
    match window {
        "24h" => Some((86_400_000, 5 * 60_000)),
        "7d" => Some((7 * 86_400_000, 30 * 60_000)),
        "30d" => Some((30 * 86_400_000, 2 * 60 * 60_000)),
        _ => None,
    }
}

fn percentile_millis(values: &[i64], percentile: usize) -> Option<i64> {
    let mut values = values.to_vec();
    values.sort_unstable();
    let index = values
        .len()
        .saturating_mul(percentile.clamp(1, 100))
        .saturating_add(99)
        / 100;
    let index = index.checked_sub(1)?;
    values.get(index).copied()
}

fn millis_to_seconds(value: i64) -> i64 {
    value.saturating_add(999) / 1_000
}

fn format_duration_millis(value: i64) -> String {
    let seconds = millis_to_seconds(value);
    if seconds >= 3_600 {
        format!("{}h {}m", seconds / 3_600, (seconds % 3_600) / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn iso(value: i64) -> String {
    iso_optional(Some(value)).unwrap_or_default()
}

fn iso_optional(value: Option<i64>) -> Option<String> {
    value
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn active_status(status: &str) -> bool {
    matches!(
        status,
        "starting" | "online" | "idle" | "busy" | "paused" | "stopped"
    )
}

fn stored_i64(values: &HashMap<String, Value>, key: &str, fallback: i64) -> i64 {
    values.get(key).and_then(Value::as_i64).unwrap_or(fallback)
}

async fn setting_bool(state: &AppState, key: &str, fallback: bool) -> bool {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(&state.database)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<bool>(&value).ok())
        .unwrap_or(fallback)
}

async fn setting_i64_value(state: &AppState, key: &str, fallback: i64) -> i64 {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(&state.database)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<i64>(&value).ok())
        .unwrap_or(fallback)
}

fn runner_registration_url(account_login: &str, repository: Option<&RepositoryCapacity>) -> String {
    match repository {
        Some(repository) => format!(
            "https://github.com/{}/{}",
            repository.owner, repository.name
        ),
        None => format!("https://github.com/{account_login}"),
    }
}

fn github_installation_settings_url(account_type: &str, account_login: &str, id: i64) -> String {
    if account_type == "Organization" {
        format!("https://github.com/organizations/{account_login}/settings/installations/{id}")
    } else {
        format!("https://github.com/settings/installations/{id}")
    }
}

#[derive(Debug)]
struct ParsedJobLog {
    steps: Vec<StructuredLogStep>,
    annotations: Vec<LogAnnotation>,
    line_count: usize,
    hidden_diagnostic_lines: usize,
}

#[derive(Debug)]
struct StructuredLogStep {
    number: i64,
    name: String,
    status: String,
    conclusion: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    lines: Vec<CleanLogLine>,
}

#[derive(Clone, Debug)]
struct CleanLogLine {
    timestamp: Option<String>,
    timestamp_millis: Option<i64>,
    text: String,
    level: &'static str,
}

#[derive(Debug)]
struct LogAnnotation {
    level: &'static str,
    message: String,
    step_number: i64,
    step_name: String,
}

fn structure_job_log(raw: &str, metadata: &[GitHubWorkflowStep], local: bool) -> ParsedJobLog {
    let (lines, hidden_diagnostic_lines) = clean_job_lines(raw, local);
    let mut steps = metadata
        .iter()
        .map(|step| StructuredLogStep {
            number: step.number,
            name: step.name.clone(),
            status: step.status.clone(),
            conclusion: step.conclusion.clone(),
            started_at: step.started_at.clone(),
            completed_at: step.completed_at.clone(),
            lines: Vec::new(),
        })
        .collect::<Vec<_>>();
    if steps.is_empty() {
        steps.push(StructuredLogStep {
            number: 1,
            name: "Job output".into(),
            status: "completed".into(),
            conclusion: None,
            started_at: None,
            completed_at: None,
            lines: Vec::new(),
        });
    }
    let starts = steps
        .iter()
        .map(|step| github_date_millis(step.started_at.as_deref()))
        .collect::<Vec<_>>();
    for line in lines {
        let index = line.timestamp_millis.map_or(0, |timestamp| {
            starts
                .iter()
                .enumerate()
                .filter_map(|(index, start)| start.map(|start| (index, start)))
                .filter(|(_, start)| *start <= timestamp)
                .map(|(index, _)| index)
                .next_back()
                .unwrap_or(0)
        });
        steps[index].lines.push(line);
    }
    // GitHub exposes step timestamps with second precision. The final error for
    // a failed step can therefore appear a few milliseconds after cleanup steps
    // report the same start second. Keep that error with the step GitHub marked
    // as failed instead of presenting it under "Complete job".
    let failed_ranges = steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.conclusion.as_deref() == Some("failure"))
        .map(|(index, step)| {
            (
                index,
                github_date_millis(step.started_at.as_deref()),
                github_date_millis(step.completed_at.as_deref()),
            )
        })
        .collect::<Vec<_>>();
    let mut reassigned_errors = Vec::new();
    for step in &mut steps {
        if step.conclusion.as_deref() == Some("failure") {
            continue;
        }
        let mut retained = Vec::new();
        for line in std::mem::take(&mut step.lines) {
            let target = if line.level == "error" {
                line.timestamp_millis.and_then(|timestamp| {
                    failed_ranges
                        .iter()
                        .filter(|(_, started_at, completed_at)| {
                            started_at.is_none_or(|started_at| started_at <= timestamp)
                                && completed_at.is_none_or(|completed_at| {
                                    timestamp <= completed_at.saturating_add(1_000)
                                })
                        })
                        .map(|(index, _, _)| *index)
                        .next_back()
                })
            } else {
                None
            };
            if let Some(target) = target {
                reassigned_errors.push((target, line));
            } else {
                retained.push(line);
            }
        }
        step.lines = retained;
    }
    for (target, line) in reassigned_errors {
        steps[target].lines.push(line);
    }
    for step in &mut steps {
        step.lines
            .sort_by_key(|line| line.timestamp_millis.unwrap_or(i64::MIN));
    }
    let mut annotations = Vec::new();
    let mut annotation_keys = HashSet::new();
    for step in &steps {
        for line in &step.lines {
            if !matches!(line.level, "error" | "warning") {
                continue;
            }
            let key = format!("{}:{}:{}", line.level, step.number, line.text);
            if annotation_keys.insert(key) {
                annotations.push(LogAnnotation {
                    level: line.level,
                    message: line.text.clone(),
                    step_number: step.number,
                    step_name: step.name.clone(),
                });
            }
        }
        if step.conclusion.as_deref() == Some("failure")
            && !annotations.iter().any(|annotation| {
                annotation.step_number == step.number && annotation.level == "error"
            })
        {
            let message = step
                .lines
                .iter()
                .rev()
                .find(|line| line.text.to_ascii_lowercase().contains("error"))
                .map_or_else(
                    || "Step concluded with failure.".into(),
                    |line| line.text.clone(),
                );
            annotations.push(LogAnnotation {
                level: "error",
                message,
                step_number: step.number,
                step_name: step.name.clone(),
            });
        }
    }
    let line_count = steps.iter().map(|step| step.lines.len()).sum();
    ParsedJobLog {
        steps,
        annotations,
        line_count,
        hidden_diagnostic_lines,
    }
}

fn clean_job_lines(raw: &str, local: bool) -> (Vec<CleanLogLine>, usize) {
    let mut lines = Vec::new();
    let mut seen = HashSet::new();
    let mut hidden = 0;
    for physical_line in raw.lines() {
        let without_ansi = strip_ansi_sequences(physical_line.trim_end_matches('\r'));
        let (outer_timestamp, outer_body) = split_log_timestamp(&without_ansi);
        let mut body = outer_body.trim_start_matches('\u{feff}');
        let mut timestamp = outer_timestamp;
        if local {
            let (inner_timestamp, inner_body) = split_log_timestamp(body);
            let Some(inner_timestamp) = inner_timestamp else {
                hidden += usize::from(!body.trim().is_empty());
                continue;
            };
            timestamp = Some(inner_timestamp);
            body = inner_body;
        }
        if body.starts_with("[RUNNER ") || body.starts_with("[WORKER ") {
            hidden += 1;
            continue;
        }
        if let Some(index) = body
            .find("[WORKER ")
            .into_iter()
            .chain(body.find("[RUNNER "))
            .min()
        {
            body = body[..index].trim_end();
            hidden += 1;
        }
        let Some((level, text)) = classify_console_line(body) else {
            hidden += 1;
            continue;
        };
        if text.is_empty() && level != "output" {
            continue;
        }
        let timestamp_string = iso_optional(timestamp);
        let dedupe_timestamp = timestamp;
        let key = format!("{dedupe_timestamp:?}:{level}:{text}");
        if !seen.insert(key) {
            continue;
        }
        lines.push(CleanLogLine {
            timestamp: timestamp_string,
            timestamp_millis: timestamp,
            text,
            level,
        });
    }
    lines.sort_by_key(|line| line.timestamp_millis.unwrap_or(i64::MIN));
    (lines, hidden)
}

fn classify_console_line(value: &str) -> Option<(&'static str, String)> {
    let value = value.trim_end();
    if value == "##[endgroup]" || value.starts_with("##[debug]") {
        return None;
    }
    for (command, level) in [
        ("group", "group"),
        ("error", "error"),
        ("warning", "warning"),
        ("notice", "notice"),
        ("command", "command"),
        ("section", "group"),
    ] {
        if let Some(text) = github_command_message(value, command) {
            return Some((level, text.to_owned()));
        }
    }
    Some(("output", value.to_owned()))
}

fn github_command_message<'a>(value: &'a str, command: &str) -> Option<&'a str> {
    let rest = value.strip_prefix(&format!("##[{command}"))?;
    let (_, message) = rest.split_once(']')?;
    Some(message)
}

fn split_log_timestamp(value: &str) -> (Option<i64>, &str) {
    let value = value.trim_start_matches('\u{feff}');
    let Some((candidate, rest)) = value.split_once(' ') else {
        return (None, value);
    };
    let timestamp = chrono::DateTime::parse_from_rfc3339(candidate)
        .ok()
        .map(|date| date.timestamp_millis());
    match timestamp {
        Some(timestamp) => (Some(timestamp), rest),
        None => (None, value),
    }
}

fn github_date_millis(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|date| date.timestamp_millis())
}

fn strip_ansi_sequences(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        if characters.next_if_eq(&'[').is_none() {
            continue;
        }
        for next in characters.by_ref() {
            if next.is_ascii_alphabetic() {
                break;
            }
        }
    }
    output
}

fn structured_step_json(step: &StructuredLogStep) -> Value {
    json!({
        "number": step.number,
        "name": step.name,
        "status": step.status,
        "conclusion": step.conclusion,
        "startedAt": step.started_at,
        "completedAt": step.completed_at,
        "lines": step.lines.iter().map(|line| json!({
            "timestamp": line.timestamp,
            "text": line.text,
            "level": line.level,
        })).collect::<Vec<_>>(),
    })
}

fn log_annotation_json(annotation: &LogAnnotation) -> Value {
    json!({
        "level": annotation.level,
        "message": annotation.message,
        "stepNumber": annotation.step_number,
        "stepName": annotation.step_name,
    })
}

#[allow(dead_code)]
fn backup_path(database: &PathBuf, name: &str) -> PathBuf {
    database.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_windows_have_bounded_chart_resolution() {
        assert_eq!(capacity_window("24h"), Some((86_400_000, 300_000)));
        assert_eq!(capacity_window("7d"), Some((604_800_000, 1_800_000)));
        assert_eq!(capacity_window("30d"), Some((2_592_000_000, 7_200_000)));
        assert_eq!(capacity_window("1y"), None);
    }

    #[test]
    fn slo_percentiles_use_sorted_observations_and_seconds_round_up() {
        let values = [9_900, 100, 4_000, 2_001];
        assert_eq!(percentile_millis(&values, 50), Some(2_001));
        assert_eq!(percentile_millis(&values, 95), Some(9_900));
        assert_eq!(percentile_millis(&[], 95), None);
        assert_eq!(millis_to_seconds(2_001), 3);
        assert_eq!(format_duration_millis(61_001), "1m 2s");
    }

    #[test]
    fn pool_labels_include_name_and_enforce_total_limit() {
        assert_eq!(
            normalized_pool_labels("linux", &["docker".into(), "docker".into()]).ok(),
            Some(vec!["docker".into(), "linux".into()])
        );
        let too_many = (0..20)
            .map(|index| format!("label-{index}"))
            .collect::<Vec<_>>();
        assert!(normalized_pool_labels("linux", &too_many).is_err());
    }

    #[test]
    fn new_runner_pools_start_with_one_runner_but_can_scale_to_zero() {
        let defaults = runner_pool_defaults("runner:latest", 64, 262_144);

        assert_eq!(defaults["desiredCount"], 1);
        assert_eq!(defaults["minCount"], 0);
    }

    #[test]
    fn workflow_actions_include_force_cancellation() {
        assert_eq!(workflow_action_endpoint("cancel").ok(), Some("cancel"));
        assert_eq!(
            workflow_action_endpoint("force-cancel").ok(),
            Some("force-cancel")
        );
        assert_eq!(workflow_action_endpoint("rerun").ok(), Some("rerun"));
        assert_eq!(
            workflow_action_endpoint("rerun-failed").ok(),
            Some("rerun-failed-jobs")
        );
        assert!(workflow_action_endpoint("delete").is_err());
    }

    #[test]
    fn repository_search_escapes_like_wildcards() {
        assert_eq!(like_pattern(r"a%b_c\d"), r"%a\%b\_c\\d%");
    }

    #[test]
    fn repository_pages_are_one_based_and_bounded() {
        assert_eq!(query_page(None), 1);
        assert_eq!(query_page(Some(-4)), 1);
        assert_eq!(query_page(Some(7)), 7);
        assert_eq!(query_page(Some(i64::MAX)), 1_000_000);
    }

    #[test]
    fn collection_pagination_is_bounded_and_uses_stable_offsets() {
        assert_eq!(pagination(None, None), (1, DEFAULT_PAGE_SIZE));
        assert_eq!(pagination(Some(3), Some(10)), (3, 10));
        assert_eq!(pagination(Some(-1), Some(500)), (1, 100));
        assert_eq!(bounded_pagination(9, 42, 10), (5, 40));
        assert_eq!(bounded_pagination(3, 0, 25), (1, 0));
    }

    #[test]
    fn settings_report_the_effective_webhook_url() -> anyhow::Result<()> {
        let base_url = url::Url::parse("https://private.example.com")?;
        let public_webhook = url::Url::parse("https://hooks.example.com/api/webhooks/github")?;

        assert_eq!(
            effective_webhook_url(&base_url, None),
            "https://private.example.com/api/webhooks/github"
        );
        assert_eq!(
            effective_webhook_url(&base_url, Some(&public_webhook)),
            "https://hooks.example.com/api/webhooks/github"
        );
        Ok(())
    }

    #[test]
    fn github_installation_settings_urls_follow_account_ownership() {
        assert_eq!(
            github_installation_settings_url("User", "octocat", 12),
            "https://github.com/settings/installations/12"
        );
        assert_eq!(
            github_installation_settings_url("Organization", "octo-org", 34),
            "https://github.com/organizations/octo-org/settings/installations/34"
        );
    }

    #[test]
    fn structured_job_logs_remove_runner_noise_and_surface_the_failed_step() {
        let steps = vec![
            GitHubWorkflowStep {
                name: "Run checkout".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
                number: 1,
                started_at: Some("2026-07-21T19:38:58.000Z".into()),
                completed_at: Some("2026-07-21T19:39:00.000Z".into()),
            },
            GitHubWorkflowStep {
                name: "Install pnpm".into(),
                status: "completed".into(),
                conclusion: Some("failure".into()),
                number: 2,
                started_at: Some("2026-07-21T19:39:00.000Z".into()),
                completed_at: Some("2026-07-21T19:39:02.000Z".into()),
            },
        ];
        let raw = concat!(
            "2026-07-21T19:38:57.000000000Z [WORKER 2026-07-21 19:38:57Z INFO JobServerQueue] Uploading logs\n",
            "2026-07-21T19:38:58.000000000Z [GRIDOPS JOB LOG page.log]\n",
            "2026-07-21T19:38:58.100000000Z 2026-07-21T19:38:58.0104000Z ##[group]Run actions/checkout@v5\n",
            "2026-07-21T19:38:58.200000000Z 2026-07-21T19:38:58.2000000Z Checked out repository\n",
            "2026-07-21T19:39:00.100000000Z 2026-07-21T19:39:00.0104000Z ##[group]Run pnpm/action-setup@v4\n",
            "2026-07-21T19:39:01.100000000Z 2026-07-21T19:39:01.0104000Z Error: No pnpm version is specified.\n",
            "2026-07-21T19:39:01.200000000Z 2026-07-21T19:39:01.0204000Z ##[error]Process completed with exit code 1.\n",
            "2026-07-21T19:39:01.300000000Z 2026-07-21T19:39:01.0204999Z ##[error]Process completed with exit code 1.\n",
        );

        let parsed = structure_job_log(raw, &steps, true);

        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.steps[0].lines.len(), 2);
        assert_eq!(parsed.steps[1].lines.len(), 3);
        assert_eq!(parsed.annotations.len(), 1);
        assert_eq!(parsed.annotations[0].step_name, "Install pnpm");
        assert_eq!(
            parsed.annotations[0].message,
            "Process completed with exit code 1."
        );
        assert!(parsed.hidden_diagnostic_lines >= 2);
        assert!(
            parsed
                .steps
                .iter()
                .flat_map(|step| &step.lines)
                .all(|line| {
                    !line.text.contains("JobServerQueue") && !line.text.contains("GRIDOPS JOB LOG")
                })
        );
    }

    #[test]
    fn github_job_logs_keep_clean_console_lines_and_remove_ansi_sequences() {
        let steps = vec![GitHubWorkflowStep {
            name: "Tests".into(),
            status: "completed".into(),
            conclusion: Some("success".into()),
            number: 1,
            started_at: Some("2026-07-21T19:40:00.000Z".into()),
            completed_at: Some("2026-07-21T19:40:01.000Z".into()),
        }];
        let raw = "2026-07-21T19:40:00.100Z \u{1b}[32m22 tests passed\u{1b}[0m\n";

        let parsed = structure_job_log(raw, &steps, false);

        assert_eq!(parsed.line_count, 1);
        assert_eq!(parsed.steps[0].lines[0].text, "22 tests passed");
    }

    #[test]
    fn final_error_stays_with_the_failed_step_when_cleanup_shares_its_second() {
        let steps = vec![
            GitHubWorkflowStep {
                name: "Verify runner toolchain".into(),
                status: "completed".into(),
                conclusion: Some("failure".into()),
                number: 4,
                started_at: Some("2026-07-21T19:39:01Z".into()),
                completed_at: Some("2026-07-21T19:39:48Z".into()),
            },
            GitHubWorkflowStep {
                name: "Complete job".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
                number: 17,
                started_at: Some("2026-07-21T19:39:48Z".into()),
                completed_at: Some("2026-07-21T19:39:48Z".into()),
            },
        ];
        let raw = concat!(
            "2026-07-21T19:39:47.900Z rustup could not install the toolchain\n",
            "2026-07-21T19:39:48.385Z ##[error]Process completed with exit code 1.\n",
            "2026-07-21T19:39:48.667Z Cleaning up orphan processes\n",
        );

        let parsed = structure_job_log(raw, &steps, false);

        assert_eq!(parsed.annotations.len(), 1);
        assert_eq!(parsed.annotations[0].step_name, "Verify runner toolchain");
        assert_eq!(parsed.steps[0].lines.len(), 2);
        assert_eq!(parsed.steps[1].lines.len(), 1);
        assert_eq!(parsed.steps[0].lines[1].level, "error");
    }
}
