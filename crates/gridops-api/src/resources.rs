use std::{
    collections::HashMap,
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
use futures_util::{StreamExt as _, TryStreamExt as _};
use gridops_core::{
    ConfigurationState, CreateRunnerPool, JitRequest, RunnerTarget, UpdateRunnerPool, now_millis,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{FromRow, Row as _};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::{
    auth::{
        AuthUser, OptionalAuth, assert_installation_admin, assert_same_origin, audit,
        require_system_admin,
    },
    error::{ApiError, ApiResult},
    oauth::control_token,
    state::AppState,
};

const MAX_ARCHIVED_LOG_BYTES: i64 = 100 * 1_024 * 1_024;
const MAX_ARCHIVED_LOG_VIEW_BYTES: u64 = 1_000_000;

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
    labels: String,
    image: String,
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
}

#[derive(Debug, FromRow)]
struct RunnerAccess {
    runner_id: String,
    runner_name: String,
    container_id: Option<String>,
    github_runner_id: Option<i64>,
    runner_status: String,
    busy: bool,
    ephemeral: bool,
    configuration_version: i64,
    last_job_id: Option<i64>,
    pool_id: String,
    pool_name: String,
    installation_id: i64,
    account_login: String,
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
    webhook_retention_days: i64,
    audit_retention_days: i64,
    reconcile_interval_seconds: i64,
    github_sync_interval_seconds: i64,
    auto_update_images: bool,
}

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.database).await?;
    Ok(Json(json!({
        "status": "ok",
        "service": "gridops-api",
        "database": "sqlite",
        "time": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    })))
}

pub async fn overview(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let configuration = configuration(&state).await?;
    let Some(user) = user else {
        return Ok(Json(json!({
            "authenticated": false,
            "configuration": configuration,
            "metrics": { "runners": 0, "online": 0, "busy": 0, "queuedJobs": 0, "successRate": null },
            "pools": [], "runs": [], "activity": [], "installations": 0,
        })));
    };
    let metrics = sqlx::query(
        r#"SELECT
          COUNT(DISTINCT CASE WHEN r.deleted_at IS NULL THEN r.id END) AS runners,
          COUNT(DISTINCT CASE WHEN r.deleted_at IS NULL AND r.status IN ('idle','busy','online') THEN r.id END) AS online,
          COUNT(DISTINCT CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN r.id END) AS busy,
          (SELECT COUNT(*) FROM workflow_jobs wj JOIN workflow_runs wr ON wr.id=wj.run_id
            JOIN repositories repo ON repo.id=wr.repository_id
            JOIN user_installations ui2 ON ui2.installation_id=repo.installation_id
            WHERE ui2.user_id=? AND wj.status='queued') AS queued_jobs,
          (SELECT COUNT(*) FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
            JOIN user_installations ui3 ON ui3.installation_id=repo.installation_id
            WHERE ui3.user_id=? AND wr.completed_at IS NOT NULL) AS completed_runs,
          (SELECT COUNT(*) FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
            JOIN user_installations ui4 ON ui4.installation_id=repo.installation_id
            WHERE ui4.user_id=? AND wr.conclusion='success') AS successful_runs
        FROM user_installations ui
        LEFT JOIN runner_pools p ON p.installation_id=ui.installation_id
        LEFT JOIN runners r ON r.pool_id=p.id
        WHERE ui.user_id=?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(&user.id)
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let completed = metrics.get::<i64, _>("completed_runs");
    let success_rate = (completed > 0).then(|| {
        ((metrics.get::<i64, _>("successful_runs") as f64 / completed as f64) * 1_000.0).round()
            / 10.0
    });

    let pool_rows = sqlx::query(
        r#"SELECT p.id,p.name,p.scope,p.desired_count,p.mode,p.state,p.paused,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('idle','busy','online') THEN 1 END) AS online,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN 1 END) AS busy,
          (SELECT COUNT(*) FROM workflow_jobs wj
            JOIN workflow_runs wr ON wr.id=wj.run_id
            JOIN repositories queued_repo ON queued_repo.id=wr.repository_id
            WHERE wj.status='queued' AND (
              queued_repo.id=p.repository_id OR
              (p.scope='organization' AND queued_repo.installation_id=p.installation_id)
            )) AS queued
        FROM runner_pools p JOIN user_installations ui ON ui.installation_id=p.installation_id
        LEFT JOIN runners r ON r.pool_id=p.id WHERE ui.user_id=?
        GROUP BY p.id ORDER BY p.created_at DESC LIMIT 8"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let pools = pool_rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"), "name": row.get::<String, _>("name"),
                "scope": row.get::<String, _>("scope"), "desired": row.get::<i64, _>("desired_count"),
                "online": row.get::<i64, _>("online"), "busy": row.get::<i64, _>("busy"),
                "queue": row.get::<i64, _>("queued"),
                "mode": row.get::<String, _>("mode"),
                "status": if row.get::<bool, _>("paused") { "paused".into() } else { row.get::<String, _>("state") },
            })
        })
        .collect::<Vec<_>>();
    let run_rows = sqlx::query(
        r#"SELECT wr.id,repo.full_name,wr.workflow_name,wr.head_branch,wr.status,wr.conclusion,
          wr.started_at,wr.completed_at,wr.html_url FROM workflow_runs wr
        JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id
        WHERE ui.user_id=? ORDER BY wr.github_created_at DESC LIMIT 6"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let runs = run_rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<i64, _>("id"), "repository": row.get::<String, _>("full_name"),
                "workflow": row.get::<String, _>("workflow_name"), "branch": row.try_get::<Option<String>, _>("head_branch").ok().flatten(),
                "status": row.get::<String, _>("status"), "conclusion": row.try_get::<Option<String>, _>("conclusion").ok().flatten(),
                "startedAt": iso_optional(row.try_get::<Option<i64>, _>("started_at").ok().flatten()),
                "completedAt": iso_optional(row.try_get::<Option<i64>, _>("completed_at").ok().flatten()),
                "htmlUrl": row.get::<String, _>("html_url"),
            })
        })
        .collect::<Vec<_>>();
    let activity_rows = sqlx::query(
        r#"SELECT re.id,re.level,re.event,re.message,re.created_at FROM runner_events re
        WHERE EXISTS (SELECT 1 FROM runner_pools p JOIN user_installations ui
          ON ui.installation_id=p.installation_id WHERE p.id=re.pool_id AND ui.user_id=?)
        ORDER BY re.created_at DESC LIMIT 8"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let activity = activity_rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"), "level": row.get::<String, _>("level"),
                "event": row.get::<String, _>("event"), "message": row.get::<String, _>("message"),
                "createdAt": iso(row.get::<i64, _>("created_at")),
            })
        })
        .collect::<Vec<_>>();
    let installations = sqlx::query(
        "SELECT COUNT(*) AS count FROM user_installations ui JOIN installations i ON i.id=ui.installation_id WHERE ui.user_id=? AND i.suspended_at IS NULL",
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?
    .get::<i64, _>("count");
    Ok(Json(json!({
        "authenticated": true,
        "configuration": configuration,
        "metrics": {
            "runners": metrics.get::<i64, _>("runners"), "online": metrics.get::<i64, _>("online"),
            "busy": metrics.get::<i64, _>("busy"), "queuedJobs": metrics.get::<i64, _>("queued_jobs"),
            "successRate": success_rate,
        },
        "pools": pools, "runs": runs, "activity": activity, "installations": installations,
    })))
}

pub async fn capacity_history(
    State(state): State<AppState>,
    Query(query): Query<CapacityQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let window = query.window.as_deref().unwrap_or("24h");
    let (window_millis, bucket_millis) = capacity_window(window)
        .ok_or_else(|| ApiError::BadRequest("Capacity window must be 24h, 7d, or 30d.".into()))?;
    let Some(user) = user else {
        return Ok(Json(json!({ "window": window, "points": [] })));
    };
    let cutoff = now_millis().saturating_sub(window_millis);
    let rows = sqlx::query(
        r#"SELECT bucket,SUM(available) AS available,SUM(busy) AS busy,SUM(queued) AS queued
          FROM (
            SELECT cs.pool_id,(cs.recorded_at / ?) * ? AS bucket,
              CAST(ROUND(AVG(cs.available)) AS INTEGER) AS available,
              CAST(ROUND(AVG(cs.busy)) AS INTEGER) AS busy,
              CAST(ROUND(AVG(cs.queued)) AS INTEGER) AS queued
            FROM capacity_samples cs
            JOIN user_installations ui ON ui.installation_id=cs.installation_id
            WHERE ui.user_id=? AND cs.recorded_at>=?
            GROUP BY cs.pool_id,bucket
          ) samples GROUP BY bucket ORDER BY bucket"#,
    )
    .bind(bucket_millis)
    .bind(bucket_millis)
    .bind(&user.id)
    .bind(cutoff)
    .fetch_all(&state.database)
    .await?;
    let points = rows
        .iter()
        .map(|row| {
            json!({
                "recordedAt": iso(row.get::<i64, _>("bucket")),
                "available": row.get::<i64, _>("available"),
                "busy": row.get::<i64, _>("busy"),
                "queued": row.get::<i64, _>("queued"),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "window": window, "points": points })))
}

pub async fn repositories(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT repo.id,repo.full_name,repo.private,repo.archived,repo.default_branch,
          repo.html_url,repo.permission,repo.last_synced_at,i.id AS installation_id,
          i.account_login,i.account_type,i.repository_selection,
          COUNT(DISTINCT p.id) AS pool_count,COUNT(DISTINCT wr.id) AS run_count,
          MAX(wr.github_updated_at) AS last_run_at
        FROM repositories repo JOIN user_installations ui ON ui.installation_id=repo.installation_id
        JOIN installations i ON i.id=repo.installation_id
        LEFT JOIN runner_pools p ON p.repository_id=repo.id LEFT JOIN workflow_runs wr ON wr.repository_id=repo.id
        WHERE ui.user_id=? GROUP BY repo.id ORDER BY repo.full_name"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<i64,_>("id"), "fullName": row.get::<String,_>("full_name"),
        "private": row.get::<bool,_>("private"), "archived": row.get::<bool,_>("archived"),
        "defaultBranch": row.get::<String,_>("default_branch"), "htmlUrl": row.get::<String,_>("html_url"),
        "permission": row.try_get::<Option<String>,_>("permission").ok().flatten(),
        "lastSyncedAt": iso(row.get::<i64,_>("last_synced_at")),
        "installationId": row.get::<i64,_>("installation_id"), "accountLogin": row.get::<String,_>("account_login"),
        "accountType": row.get::<String,_>("account_type"), "repositorySelection": row.get::<String,_>("repository_selection"),
        "poolCount": row.get::<i64,_>("pool_count"), "runCount": row.get::<i64,_>("run_count"),
        "lastRunAt": iso_optional(row.try_get::<Option<i64>,_>("last_run_at").ok().flatten()),
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let query = query.q.trim();
    if !(2..=100).contains(&query.len()) {
        return Err(ApiError::BadRequest(
            "Search requires 2-100 characters.".into(),
        ));
    }
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let rows = sqlx::query(
        r#"SELECT kind,id,title,subtitle,href FROM (
          SELECT 'repository' AS kind,CAST(repo.id AS TEXT) AS id,repo.full_name AS title,
            i.account_login AS subtitle,'/repositories' AS href,repo.full_name AS sort_value
          FROM repositories repo JOIN installations i ON i.id=repo.installation_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE ui.user_id=? AND repo.full_name LIKE ? ESCAPE '\'
          UNION ALL
          SELECT 'runner pool',p.id,p.name,COALESCE(repo.full_name,i.account_login),
            '/runner-pools/' || p.id,p.name
          FROM runner_pools p JOIN installations i ON i.id=p.installation_id
          JOIN user_installations ui ON ui.installation_id=p.installation_id
          LEFT JOIN repositories repo ON repo.id=p.repository_id
          WHERE ui.user_id=? AND p.name LIKE ? ESCAPE '\'
          UNION ALL
          SELECT 'runner',r.id,r.name,p.name,'/runners',r.name FROM runners r
          JOIN runner_pools p ON p.id=r.pool_id JOIN user_installations ui ON ui.installation_id=p.installation_id
          WHERE ui.user_id=? AND r.deleted_at IS NULL AND r.name LIKE ? ESCAPE '\'
          UNION ALL
          SELECT 'workflow run',CAST(wr.id AS TEXT),wr.workflow_name,repo.full_name,
            '/workflow-runs/' || wr.id,wr.workflow_name FROM workflow_runs wr
          JOIN repositories repo ON repo.id=wr.repository_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE ui.user_id=? AND (wr.workflow_name LIKE ? ESCAPE '\' OR repo.full_name LIKE ? ESCAPE '\')
        ) ORDER BY sort_value LIMIT 12"#,
    )
    .bind(&user.id).bind(&pattern).bind(&user.id).bind(&pattern)
    .bind(&user.id).bind(&pattern).bind(&user.id).bind(&pattern).bind(&pattern)
    .fetch_all(&state.database).await?;
    let items = rows
        .iter()
        .map(|row| {
            json!({
                "kind": row.get::<String,_>("kind"), "id": row.get::<String,_>("id"),
                "title": row.get::<String,_>("title"), "subtitle": row.get::<String,_>("subtitle"),
                "href": row.get::<String,_>("href"),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!(items)))
}

pub async fn runner_pools(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT p.id,p.name,p.scope,p.mode,p.labels,p.image,p.desired_count,p.min_count,
          p.max_count,p.cpu_limit,p.memory_limit_mb,p.paused,p.state,i.account_login,
          ui.permission AS installation_permission,
          repo.full_name AS repository,
          COUNT(CASE WHEN r.deleted_at IS NULL THEN 1 END) AS total_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('online','idle','busy') THEN 1 END) AS online_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN 1 END) AS busy_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status='failed' THEN 1 END) AS failed_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.configuration_version < p.configuration_version THEN 1 END) AS outdated_runners,
          p.created_at FROM runner_pools p
        JOIN user_installations ui ON ui.installation_id=p.installation_id AND ui.user_id=?
        JOIN installations i ON i.id=p.installation_id LEFT JOIN repositories repo ON repo.id=p.repository_id
        LEFT JOIN runners r ON r.pool_id=p.id GROUP BY p.id ORDER BY p.created_at DESC"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "name": row.get::<String,_>("name"), "scope": row.get::<String,_>("scope"),
        "mode": row.get::<String,_>("mode"), "labels": json_array(row.get::<&str,_>("labels")), "image": row.get::<String,_>("image"),
        "desiredCount": row.get::<i64,_>("desired_count"), "minCount": row.get::<i64,_>("min_count"), "maxCount": row.get::<i64,_>("max_count"),
        "cpuLimit": row.get::<f64,_>("cpu_limit"), "memoryLimitMb": row.get::<i64,_>("memory_limit_mb"),
        "paused": row.get::<bool,_>("paused"), "state": row.get::<String,_>("state"), "accountLogin": row.get::<String,_>("account_login"),
        "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(), "totalRunners": row.get::<i64,_>("total_runners"),
        "onlineRunners": row.get::<i64,_>("online_runners"), "busyRunners": row.get::<i64,_>("busy_runners"),
        "failedRunners": row.get::<i64,_>("failed_runners"), "outdatedRunners": row.get::<i64,_>("outdated_runners"),
        "canManage": user.role == "admin" || row.get::<String,_>("installation_permission") == "admin",
        "createdAt": iso(row.get::<i64,_>("created_at")),
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn runner_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let pool = pool_access(&state, &user, &pool_id).await?;
    let additional_labels = json_array(&pool.labels)
        .into_iter()
        .filter(|label| label != &pool.name)
        .collect::<Vec<_>>();
    let repository = pool
        .repository_owner
        .as_ref()
        .zip(pool.repository_name.as_ref())
        .map(|(owner, repository)| format!("{owner}/{repository}"));
    Ok(Json(json!({
        "id": pool_id,
        "installationId": pool.installation_id,
        "repositoryId": pool.repository_id,
        "repository": repository,
        "accountLogin": pool.account_login,
        "name": pool.name,
        "scope": pool.scope,
        "mode": pool.mode,
        "labels": additional_labels,
        "image": pool.image,
        "desiredCount": pool.desired_count,
        "minCount": pool.min_count,
        "maxCount": pool.max_count,
        "cpuLimit": pool.cpu_limit,
        "memoryLimitMb": pool.memory_limit_mb,
        "runnerGroupId": pool.runner_group_id,
        "paused": pool.paused,
        "state": pool.state,
        "autoscalingEnabled": pool.autoscaling_enabled,
        "queueScaleFactor": pool.queue_scale_factor,
        "idleTimeoutMinutes": pool.idle_timeout_minutes,
        "configurationVersion": pool.configuration_version,
        "canManage": user.role == "admin" || pool.installation_permission == "admin",
    })))
}

pub async fn update_runner_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<UpdateRunnerPool>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    input.validate().map_err(ApiError::BadRequest)?;
    let pool = pool_access(&state, &user, &pool_id).await?;
    assert_installation_admin(&state, &user, pool.installation_id).await?;
    let labels = normalized_pool_labels(&input.name, &input.labels)?;
    let encoded_labels =
        serde_json::to_string(&labels).map_err(|error| ApiError::Internal(error.into()))?;
    let runner_group_id = if pool.scope == "repository" {
        1
    } else {
        input.runner_group_id
    };
    let runtime_changed = pool.name != input.name
        || pool.mode != input.mode
        || pool.labels != encoded_labels
        || pool.image != input.image
        || (pool.cpu_limit - input.cpu_limit).abs() > f64::EPSILON
        || pool.memory_limit_mb != input.memory_limit_mb
        || pool.runner_group_id != runner_group_id;
    let version_increment = i64::from(runtime_changed);
    let now = now_millis();
    let result = sqlx::query(
        r#"UPDATE runner_pools SET name=?,mode=?,labels=?,image=?,desired_count=?,min_count=?,
          max_count=?,cpu_limit=?,memory_limit_mb=?,ephemeral=?,runner_group_id=?,
          autoscaling_enabled=?,queue_scale_factor=?,idle_timeout_minutes=?,
          configuration_version=configuration_version+?,
          state=CASE WHEN ?=1 AND paused=0 THEN 'updating' ELSE state END,updated_at=? WHERE id=?"#,
    )
    .bind(&input.name)
    .bind(&input.mode)
    .bind(&encoded_labels)
    .bind(&input.image)
    .bind(input.desired_count)
    .bind(input.min_count)
    .bind(input.max_count)
    .bind(input.cpu_limit)
    .bind(input.memory_limit_mb)
    .bind(input.mode == "ephemeral")
    .bind(runner_group_id)
    .bind(input.autoscaling_enabled)
    .bind(input.queue_scale_factor)
    .bind(input.idle_timeout_minutes)
    .bind(version_increment)
    .bind(runtime_changed)
    .bind(now)
    .bind(&pool_id)
    .execute(&state.database)
    .await;
    if let Err(sqlx::Error::Database(error)) = &result
        && error.is_unique_violation()
    {
        return Err(ApiError::Conflict(
            "A runner pool with this name already exists for the installation.".into(),
        ));
    }
    result?;
    audit(
        &state,
        &user,
        "runner_pool.updated",
        "runner_pool",
        Some(&pool_id),
        json!({
            "name": input.name,
            "desiredCount": input.desired_count,
            "runtimeConfigurationChanged": runtime_changed,
            "configurationVersion": pool.configuration_version + version_increment,
        }),
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "configurationVersion": pool.configuration_version + version_increment,
        "rollingReplacement": runtime_changed,
    })))
}

pub async fn runners(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT r.id,r.name,r.status,r.busy,r.ephemeral,r.os,r.architecture,r.container_id,
          r.github_runner_id,r.failure_reason,r.registered_at,r.last_heartbeat_at,r.created_at,
          p.id AS pool_id,p.name AS pool_name,p.paused AS pool_paused,i.account_login,
          ui.permission AS installation_permission,
          repo.full_name AS repository,wj.name AS current_job_name,wj.run_id AS current_run_id
        FROM runners r JOIN runner_pools p ON p.id=r.pool_id
        JOIN user_installations ui ON ui.installation_id=p.installation_id AND ui.user_id=?
        JOIN installations i ON i.id=p.installation_id LEFT JOIN repositories repo ON repo.id=p.repository_id
        LEFT JOIN workflow_jobs wj ON wj.id=r.current_job_id
        WHERE r.deleted_at IS NULL ORDER BY r.created_at DESC"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "name": row.get::<String,_>("name"), "status": row.get::<String,_>("status"),
        "busy": row.get::<bool,_>("busy"), "ephemeral": row.get::<bool,_>("ephemeral"), "os": row.get::<String,_>("os"),
        "architecture": row.get::<String,_>("architecture"), "containerId": row.try_get::<Option<String>,_>("container_id").ok().flatten(),
        "githubRunnerId": row.try_get::<Option<i64>,_>("github_runner_id").ok().flatten(),
        "failureReason": row.try_get::<Option<String>,_>("failure_reason").ok().flatten(),
        "registeredAt": iso_optional(row.try_get::<Option<i64>,_>("registered_at").ok().flatten()),
        "lastHeartbeatAt": iso_optional(row.try_get::<Option<i64>,_>("last_heartbeat_at").ok().flatten()), "createdAt": iso(row.get::<i64,_>("created_at")),
        "poolId": row.get::<String,_>("pool_id"), "poolName": row.get::<String,_>("pool_name"), "poolPaused": row.get::<bool,_>("pool_paused"),
        "accountLogin": row.get::<String,_>("account_login"), "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(),
        "currentJobName": row.try_get::<Option<String>,_>("current_job_name").ok().flatten(), "currentRunId": row.try_get::<Option<i64>,_>("current_run_id").ok().flatten(),
        "canManage": user.role == "admin" || row.get::<String,_>("installation_permission") == "admin",
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn workflow_runs(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT wr.id,wr.workflow_name,wr.run_number,wr.run_attempt,wr.event,wr.status,
          wr.conclusion,wr.head_branch,wr.head_sha,wr.actor_login,wr.html_url,wr.started_at,
          wr.completed_at,wr.github_created_at,repo.full_name,ui.permission AS installation_permission,
          COUNT(wj.id) AS job_count,COUNT(CASE WHEN wj.status='in_progress' THEN 1 END) AS active_jobs,
          COUNT(CASE WHEN wj.conclusion='failure' THEN 1 END) AS failed_jobs
        FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id AND ui.user_id=?
        LEFT JOIN workflow_jobs wj ON wj.run_id=wr.id GROUP BY wr.id ORDER BY wr.github_created_at DESC LIMIT 250"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let items = rows
        .iter()
        .map(|row| workflow_run_json(row, user.role == "admin"))
        .collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn workflow_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let run = sqlx::query(
        r#"SELECT wr.id,wr.workflow_name,wr.run_number,wr.run_attempt,wr.event,wr.status,
          wr.conclusion,wr.head_branch,wr.head_sha,wr.actor_login,wr.html_url,wr.started_at,
          wr.completed_at,wr.github_created_at,repo.full_name,ui.permission AS installation_permission
        FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id
        WHERE wr.id=? AND ui.user_id=?"#,
    )
    .bind(run_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Workflow run does not exist or is not accessible.".into())
    })?;
    let jobs = sqlx::query(
        r#"SELECT wj.id,wj.name,wj.status,wj.conclusion,wj.runner_name,wj.runner_group_name,
          wj.labels,wj.html_url,wj.started_at,wj.completed_at,
          (SELECT r.id FROM runners r WHERE r.current_job_id=wj.id AND r.deleted_at IS NULL
            ORDER BY r.updated_at DESC LIMIT 1) AS live_runner_id,
          (SELECT ls.id FROM log_streams ls WHERE ls.job_id=wj.id AND ls.complete=1
            ORDER BY ls.created_at DESC LIMIT 1) AS archived_log_id
          FROM workflow_jobs wj WHERE wj.run_id=? ORDER BY wj.created_at"#,
    )
    .bind(run_id)
    .fetch_all(&state.database)
    .await?;
    let jobs = jobs.iter().map(|row| json!({
        "id": row.get::<i64,_>("id"), "name": row.get::<String,_>("name"), "status": row.get::<String,_>("status"),
        "conclusion": row.try_get::<Option<String>,_>("conclusion").ok().flatten(), "runnerName": row.try_get::<Option<String>,_>("runner_name").ok().flatten(),
        "runnerGroupName": row.try_get::<Option<String>,_>("runner_group_name").ok().flatten(), "labels": json_array(row.get::<&str,_>("labels")),
        "htmlUrl": row.get::<String,_>("html_url"), "startedAt": iso_optional(row.try_get::<Option<i64>,_>("started_at").ok().flatten()),
        "completedAt": iso_optional(row.try_get::<Option<i64>,_>("completed_at").ok().flatten()),
        "liveRunnerId": row.try_get::<Option<String>,_>("live_runner_id").ok().flatten(),
        "archivedLogId": row.try_get::<Option<String>,_>("archived_log_id").ok().flatten(),
    })).collect::<Vec<_>>();
    Ok(Json(json!({
        "id": run.get::<i64,_>("id"), "workflowName": run.get::<String,_>("workflow_name"), "runNumber": run.get::<i64,_>("run_number"),
        "runAttempt": run.get::<i64,_>("run_attempt"), "event": run.get::<String,_>("event"), "status": run.get::<String,_>("status"),
        "conclusion": run.try_get::<Option<String>,_>("conclusion").ok().flatten(), "headBranch": run.try_get::<Option<String>,_>("head_branch").ok().flatten(),
        "headSha": run.get::<String,_>("head_sha"), "actorLogin": run.try_get::<Option<String>,_>("actor_login").ok().flatten(),
        "htmlUrl": run.get::<String,_>("html_url"), "startedAt": iso_optional(run.try_get::<Option<i64>,_>("started_at").ok().flatten()),
        "completedAt": iso_optional(run.try_get::<Option<i64>,_>("completed_at").ok().flatten()), "createdAt": iso(run.get::<i64,_>("github_created_at")),
        "repository": run.get::<String,_>("full_name"), "jobs": jobs,
        "canManage": user.role == "admin" || run.get::<String,_>("installation_permission") == "admin",
    })))
}

pub async fn webhook_deliveries(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT wd.id,wd.event,wd.action,wd.installation_id,wd.repository_id,
          wd.signature_valid,wd.status,wd.error,wd.received_at,wd.processed_at,
          CASE WHEN wd.installation_id IS NULL THEN ?='admin' ELSE EXISTS (
            SELECT 1 FROM user_installations manage WHERE manage.installation_id=wd.installation_id
              AND manage.user_id=? AND (manage.permission='admin' OR ?='admin')
          ) END AS can_retry,
          i.account_login,repo.full_name FROM webhook_deliveries wd
        LEFT JOIN installations i ON i.id=wd.installation_id LEFT JOIN repositories repo ON repo.id=wd.repository_id
        WHERE wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui
          WHERE ui.installation_id=wd.installation_id AND ui.user_id=?)
        ORDER BY wd.received_at DESC LIMIT 250"#,
    )
    .bind(&user.role)
    .bind(&user.id)
    .bind(&user.role)
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "event": row.get::<String,_>("event"), "action": row.try_get::<Option<String>,_>("action").ok().flatten(),
        "installationId": row.try_get::<Option<i64>,_>("installation_id").ok().flatten(), "repositoryId": row.try_get::<Option<i64>,_>("repository_id").ok().flatten(),
        "signatureValid": row.get::<bool,_>("signature_valid"), "status": row.get::<String,_>("status"), "error": row.try_get::<Option<String>,_>("error").ok().flatten(),
        "receivedAt": iso(row.get::<i64,_>("received_at")), "processedAt": iso_optional(row.try_get::<Option<i64>,_>("processed_at").ok().flatten()),
        "accountLogin": row.try_get::<Option<String>,_>("account_login").ok().flatten(), "repository": row.try_get::<Option<String>,_>("full_name").ok().flatten(),
        "canRetry": row.get::<bool,_>("can_retry"),
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn audit_events(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT id,actor_label,action,target_type,target_id,metadata,ip_address,created_at
        FROM audit_events WHERE actor_user_id=? OR actor_label='system'
        ORDER BY created_at DESC LIMIT 500"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "actorLabel": row.get::<String,_>("actor_label"), "action": row.get::<String,_>("action"),
        "targetType": row.get::<String,_>("target_type"), "targetId": row.try_get::<Option<String>,_>("target_id").ok().flatten(),
        "metadata": row.get::<String,_>("metadata"), "ipAddress": row.try_get::<Option<String>,_>("ip_address").ok().flatten(),
        "createdAt": iso(row.get::<i64,_>("created_at")),
    })).collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn log_targets(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(empty_page());
    };
    let rows = sqlx::query(
        r#"SELECT r.id,r.name,r.status,r.busy,r.container_id,r.updated_at,p.name AS pool_name,
          repo.full_name FROM runners r JOIN runner_pools p ON p.id=r.pool_id
        JOIN user_installations ui ON ui.installation_id=p.installation_id AND ui.user_id=?
        LEFT JOIN repositories repo ON repo.id=p.repository_id
        WHERE r.deleted_at IS NULL AND r.container_id IS NOT NULL ORDER BY r.busy DESC,r.updated_at DESC"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let mut items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "name": row.get::<String,_>("name"), "status": row.get::<String,_>("status"),
        "busy": row.get::<bool,_>("busy"), "containerId": row.get::<String,_>("container_id"), "updatedAt": iso(row.get::<i64,_>("updated_at")),
        "poolName": row.get::<String,_>("pool_name"), "repository": row.try_get::<Option<String>,_>("full_name").ok().flatten(), "kind": "live",
    })).collect::<Vec<_>>();
    let archives = sqlx::query(
        r#"SELECT ls.id,ls.runner_id,ls.runner_name,ls.pool_name,ls.repository,ls.size_bytes,
          ls.created_at FROM log_streams ls JOIN user_installations ui
          ON ui.installation_id=ls.installation_id AND ui.user_id=?
          WHERE ls.complete=1 ORDER BY ls.created_at DESC LIMIT 100"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    items.extend(archives.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "runnerId": row.try_get::<Option<String>,_>("runner_id").ok().flatten(),
        "name": row.try_get::<Option<String>,_>("runner_name").ok().flatten().unwrap_or_else(|| "Archived runner".into()),
        "status": "archived", "busy": false, "containerId": null,
        "updatedAt": iso(row.get::<i64,_>("created_at")),
        "poolName": row.try_get::<Option<String>,_>("pool_name").ok().flatten().unwrap_or_else(|| "Deleted pool".into()),
        "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(),
        "sizeBytes": row.get::<i64,_>("size_bytes"), "kind": "archive",
    })));
    Ok(Json(json!({ "authenticated": true, "items": items })))
}

pub async fn runner_pool_options(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(Json(
            json!({ "authenticated": false, "installations": [], "repositories": [], "runnerGroups": [], "installUrl": null, "defaults": null }),
        ));
    };
    let installation_rows = sqlx::query(
        r#"SELECT i.id,i.account_login,i.account_type FROM user_installations ui
           JOIN installations i ON i.id=ui.installation_id
           WHERE ui.user_id=? AND i.suspended_at IS NULL
             AND (ui.permission='admin' OR ?='admin') ORDER BY i.account_login"#,
    )
    .bind(&user.id)
    .bind(&user.role)
    .fetch_all(&state.database)
    .await?;
    let repository_rows = sqlx::query(
        r#"SELECT r.id,r.installation_id,r.full_name,r.private FROM repositories r
           JOIN user_installations ui ON ui.installation_id=r.installation_id
           JOIN installations i ON i.id=r.installation_id
           WHERE ui.user_id=? AND r.archived=0 AND i.suspended_at IS NULL
             AND (ui.permission='admin' OR ?='admin') ORDER BY r.full_name"#,
    )
    .bind(&user.id)
    .bind(&user.role)
    .fetch_all(&state.database)
    .await?;
    let installations = installation_rows.iter().map(|row| json!({
        "id": row.get::<i64,_>("id"), "accountLogin": row.get::<String,_>("account_login"), "accountType": row.get::<String,_>("account_type"),
    })).collect::<Vec<_>>();
    let repositories = repository_rows.iter().map(|row| json!({
        "id": row.get::<i64,_>("id"), "installationId": row.get::<i64,_>("installation_id"),
        "fullName": row.get::<String,_>("full_name"), "private": row.get::<bool,_>("private"),
    })).collect::<Vec<_>>();
    let mut runner_groups = Vec::new();
    for row in &installation_rows {
        if row.get::<String, _>("account_type") != "Organization" {
            continue;
        }
        let installation_id = row.get::<i64, _>("id");
        let organization = row.get::<String, _>("account_login");
        let token = match control_token(&state, &user.id, installation_id).await {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(installation_id, organization, error = ?error, "could not authorize runner-group discovery");
                continue;
            }
        };
        match state.github.runner_groups(&organization, &token).await {
            Ok(groups) => runner_groups.extend(groups.into_iter().map(|group| {
                json!({
                    "installationId": installation_id, "id": group.id, "name": group.name,
                    "visibility": group.visibility, "isDefault": group.is_default,
                })
            })),
            Err(error) => {
                tracing::warn!(installation_id, organization, error = ?error, "could not load GitHub runner groups");
            }
        }
    }
    let app_slug = state.github_app_slug().await.map_err(ApiError::Internal)?;
    Ok(Json(json!({
        "authenticated": true, "installations": installations, "repositories": repositories, "runnerGroups": runner_groups,
        "installUrl": format!("https://github.com/apps/{app_slug}/installations/new"),
        "defaults": {
            "image": state.config.runner_image(), "labels": ["gridops"], "cpuLimit": 2,
            "memoryLimitMb": 4096, "desiredCount": 0, "minCount": 0, "maxCount": 10,
            "autoscalingEnabled": true, "queueScaleFactor": 1, "idleTimeoutMinutes": 5, "runnerGroupId": 1,
        }
    })))
}

pub async fn create_runner_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<CreateRunnerPool>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    assert_same_origin(&state, &headers)?;
    input.validate().map_err(ApiError::BadRequest)?;
    assert_installation_admin(&state, &user, input.installation_id).await?;
    if let Some(repository_id) = input.repository_id {
        let repository =
            sqlx::query("SELECT installation_id,archived FROM repositories WHERE id=?")
                .bind(repository_id)
                .fetch_optional(&state.database)
                .await?;
        if repository.is_none_or(|row| {
            row.get::<i64, _>("installation_id") != input.installation_id
                || row.get::<bool, _>("archived")
        }) {
            return Err(ApiError::BadRequest(
                "The selected repository is unavailable for this installation.".into(),
            ));
        }
    }
    let pool_id = uuid::Uuid::new_v4().to_string();
    let labels = normalized_pool_labels(&input.name, &input.labels)?;
    let runner_group_id = if input.scope == "repository" {
        1
    } else {
        input.runner_group_id
    };
    let now = now_millis();
    let result = sqlx::query(
        r#"INSERT INTO runner_pools (
          id,installation_id,repository_id,name,scope,mode,labels,image,desired_count,min_count,
          max_count,cpu_limit,memory_limit_mb,ephemeral,paused,state,created_by,created_at,updated_at,
          runner_group_id,autoscaling_enabled,queue_scale_factor,idle_timeout_minutes
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,0,'active',?,?,?,?,?,?,?)"#,
    )
    .bind(&pool_id).bind(input.installation_id).bind(input.repository_id).bind(&input.name)
    .bind(&input.scope).bind(&input.mode).bind(serde_json::to_string(&labels).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(&input.image).bind(input.desired_count).bind(input.min_count).bind(input.max_count).bind(input.cpu_limit)
    .bind(input.memory_limit_mb).bind(input.mode == "ephemeral").bind(&user.id).bind(now).bind(now)
    .bind(runner_group_id).bind(input.autoscaling_enabled).bind(input.queue_scale_factor).bind(input.idle_timeout_minutes)
    .execute(&state.database).await;
    if let Err(sqlx::Error::Database(error)) = &result
        && error.is_unique_violation()
    {
        return Err(ApiError::Conflict(
            "A runner pool with this name already exists for the installation.".into(),
        ));
    }
    result?;
    audit(
        &state,
        &user,
        "runner_pool.created",
        "runner_pool",
        Some(&pool_id),
        json!({ "name": input.name, "scope": input.scope, "desiredCount": input.desired_count }),
    )
    .await?;
    let mut provisioned = Vec::new();
    for _ in 0..input.desired_count {
        match provision(&state, &user, &pool_id).await {
            Ok(runner) => provisioned.push(runner),
            Err(error) => provisioned
                .push(json!({ "runnerId": null, "status": "failed", "error": error.to_string() })),
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": pool_id, "provisioned": provisioned })),
    ))
}

pub async fn runner_pool_action(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<PoolAction>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    match input.action.as_str() {
        "pause" => set_pool_paused(&state, &user, &pool_id, true).await?,
        "resume" => set_pool_paused(&state, &user, &pool_id, false).await?,
        "reconcile" => return Ok(Json(reconcile_pool(&state, &user, &pool_id).await?)),
        "scale" => {
            let pool = pool_access(&state, &user, &pool_id).await?;
            assert_installation_admin(&state, &user, pool.installation_id).await?;
            let desired = input
                .desired_count
                .ok_or_else(|| ApiError::BadRequest("desiredCount is required.".into()))?;
            if desired < pool.min_count || desired > pool.max_count {
                return Err(ApiError::BadRequest(format!(
                    "Desired capacity must be between {} and {}.",
                    pool.min_count, pool.max_count
                )));
            }
            sqlx::query("UPDATE runner_pools SET desired_count=?,updated_at=? WHERE id=?")
                .bind(desired)
                .bind(now_millis())
                .bind(&pool_id)
                .execute(&state.database)
                .await?;
            let result = reconcile_pool(&state, &user, &pool_id).await?;
            audit(
                &state,
                &user,
                "runner_pool.scaled",
                "runner_pool",
                Some(&pool_id),
                json!({ "desiredCount": desired }),
            )
            .await?;
            return Ok(Json(result));
        }
        _ => {
            return Err(ApiError::BadRequest(
                "Runner pool action is invalid.".into(),
            ));
        }
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_runner_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    let pool = pool_access(&state, &user, &pool_id).await?;
    assert_installation_admin(&state, &user, pool.installation_id).await?;
    sqlx::query("UPDATE runner_pools SET paused=1,state='deleting',updated_at=? WHERE id=?")
        .bind(now_millis())
        .bind(&pool_id)
        .execute(&state.database)
        .await?;
    let runners = runners_for_pool(&state, &pool_id).await?;
    for runner in &runners {
        delete_runner_resources(&state, &user, runner).await?;
    }
    audit(
        &state,
        &user,
        "runner_pool.deleted",
        "runner_pool",
        Some(&pool_id),
        json!({ "runners": runners.len() }),
    )
    .await?;
    sqlx::query("DELETE FROM runner_pools WHERE id=?")
        .bind(&pool_id)
        .execute(&state.database)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn runner_action(
    State(state): State<AppState>,
    Path(runner_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<RunnerAction>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    let runner = runner_access(&state, &user, &runner_id).await?;
    assert_installation_admin(&state, &user, runner.installation_id).await?;
    match input.action.as_str() {
        "delete" => delete_runner_resources(&state, &user, &runner).await?,
        "rebuild" => {
            delete_runner_resources(&state, &user, &runner).await?;
            provision(&state, &user, &runner.pool_id).await?;
        }
        "start" | "stop" | "pause" | "resume" | "restart" => {
            if runner.ephemeral && matches!(input.action.as_str(), "start" | "restart") {
                return Err(ApiError::Conflict(
                    "Ephemeral runners cannot be started or restarted; rebuild the runner instead."
                        .into(),
                ));
            }
            let container_id = runner
                .container_id
                .as_deref()
                .ok_or_else(|| ApiError::Conflict("Runner has no managed container.".into()))?;
            manager_json(
                &state,
                Method::POST,
                &format!("v1/runners/{container_id}/{}", input.action),
                None,
            )
            .await?;
            let status = match input.action.as_str() {
                "stop" => "stopped",
                "pause" => "paused",
                _ => "online",
            };
            sqlx::query("UPDATE runners SET status=?,updated_at=? WHERE id=?")
                .bind(status)
                .bind(now_millis())
                .bind(&runner_id)
                .execute(&state.database)
                .await?;
        }
        _ => return Err(ApiError::BadRequest("Runner action is invalid.".into())),
    }
    audit(
        &state,
        &user,
        &format!("runner.{}", input.action),
        "runner",
        Some(&runner_id),
        json!({ "poolId": runner.pool_id }),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn runner_logs(
    State(state): State<AppState>,
    Path(runner_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let runner = runner_access(&state, &user, &runner_id).await?;
    let container_id = runner
        .container_id
        .ok_or_else(|| ApiError::Conflict("Runner has no active container log stream.".into()))?;
    let logs = manager_text(&state, &format!("v1/runners/{container_id}/logs")).await?;
    Ok(Json(
        json!({ "runnerId": runner_id, "name": runner.runner_name, "logs": logs }),
    ))
}

pub async fn runner_log_stream(
    State(state): State<AppState>,
    Path(runner_id): Path<String>,
    Query(query): Query<LogStreamQuery>,
    user: AuthUser,
) -> ApiResult<Response> {
    let runner = runner_access(&state, &user, &runner_id).await?;
    let container_id = runner
        .container_id
        .ok_or_else(|| ApiError::Conflict("Runner has no active container log stream.".into()))?;
    let tail = query.tail.as_deref().unwrap_or("500");
    if tail != "0"
        && !tail
            .parse::<u32>()
            .is_ok_and(|lines| (1..=5_000).contains(&lines))
    {
        return Err(ApiError::BadRequest(
            "Log stream tail must be from 0 to 5000.".into(),
        ));
    }
    let response = manager_get(
        &state,
        &format!("v1/runners/{container_id}/logs?follow=true&tail={tail}"),
    )
    .await?;
    let stream = response
        .bytes_stream()
        .take_until(tokio::time::sleep(Duration::from_secs(25)))
        .map_err(std::io::Error::other);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub async fn archived_logs(
    State(state): State<AppState>,
    Path(stream_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        r#"SELECT ls.path,ls.runner_name FROM log_streams ls
          JOIN user_installations ui ON ui.installation_id=ls.installation_id
          WHERE ls.id=? AND ui.user_id=? AND ls.complete=1"#,
    )
    .bind(&stream_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("Archived runner log does not exist.".into()))?;
    let filename = row.get::<String, _>("path");
    let path = safe_log_path(&state, &filename)?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let size = file
        .metadata()
        .await
        .map_err(|error| ApiError::Internal(error.into()))?
        .len();
    let start = size.saturating_sub(MAX_ARCHIVED_LOG_VIEW_BYTES);
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let mut bytes = Vec::with_capacity((size - start) as usize);
    file.read_to_end(&mut bytes)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let prefix = if start > 0 {
        "[GridOps is showing the final 1 MB of this retained log.]\n"
    } else {
        ""
    };
    Ok(Json(json!({
        "streamId": stream_id,
        "name": row.try_get::<Option<String>,_>("runner_name").ok().flatten().unwrap_or_else(|| "Archived runner".into()),
        "logs": format!("{prefix}{}", String::from_utf8_lossy(&bytes)),
    })))
}

pub async fn workflow_run_action(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<WorkflowAction>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    let run = sqlx::query(
        r#"SELECT repo.owner,repo.name,repo.installation_id FROM workflow_runs wr
        JOIN repositories repo ON repo.id=wr.repository_id JOIN user_installations ui ON ui.installation_id=repo.installation_id
        WHERE wr.id=? AND ui.user_id=?"#,
    ).bind(run_id).bind(&user.id).fetch_optional(&state.database).await?
        .ok_or_else(|| ApiError::NotFound("Workflow run does not exist or is not accessible.".into()))?;
    let endpoint = match input.action.as_str() {
        "cancel" => "cancel",
        "rerun" => "rerun",
        "rerun-failed" => "rerun-failed-jobs",
        _ => return Err(ApiError::BadRequest("Workflow action is invalid.".into())),
    };
    assert_installation_admin(&state, &user, run.get("installation_id")).await?;
    let token = control_token(&state, &user.id, run.get("installation_id")).await?;
    state
        .github
        .post_empty(
            &format!(
                "/repos/{}/{}/actions/runs/{run_id}/{endpoint}",
                run.get::<String, _>("owner"),
                run.get::<String, _>("name")
            ),
            &token,
            json!({}),
        )
        .await
        .map_err(ApiError::Internal)?;
    audit(
        &state,
        &user,
        &format!("workflow_run.{}", input.action),
        "workflow_run",
        Some(&run_id.to_string()),
        json!({}),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn workflow_run_logs(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Response> {
    let run = sqlx::query(
        r#"SELECT repo.owner,repo.name,repo.installation_id FROM workflow_runs wr
          JOIN repositories repo ON repo.id=wr.repository_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE wr.id=? AND ui.user_id=?"#,
    )
    .bind(run_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Workflow run does not exist or is not accessible.".into())
    })?;
    let owner = run.get::<String, _>("owner");
    let repository = run.get::<String, _>("name");
    let token = control_token(&state, &user.id, run.get("installation_id")).await?;
    let response = state
        .http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repository}/actions/runs/{run_id}/logs"
        ))
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(ApiError::ServiceUnavailable(format!(
            "GitHub log download failed ({status}): {}",
            detail.chars().take(300).collect::<String>()
        )));
    }
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{repository}-{run_id}-logs.zip\""),
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub async fn settings(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(Json(json!({ "authenticated": false, "data": null })));
    };
    let rows = sqlx::query("SELECT key,value FROM settings")
        .fetch_all(&state.database)
        .await?;
    let stored = rows
        .iter()
        .map(|row| {
            (
                row.get::<String, _>("key"),
                serde_json::from_str::<Value>(row.get::<&str, _>("value")).unwrap_or(Value::Null),
            )
        })
        .collect::<HashMap<_, _>>();
    let manager = match manager_json(&state, Method::GET, "v1/health", None).await {
        Ok(value) => {
            json!({ "ok": true, "dockerVersion": value.get("dockerVersion"), "apiVersion": value.get("apiVersion") })
        }
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    };
    Ok(Json(json!({ "authenticated": true, "data": {
        "configuration": configuration(&state).await?, "manager": manager,
        "settings": {
            "logRetentionDays": stored_i64(&stored, "logRetentionDays", 30),
            "webhookRetentionDays": stored_i64(&stored, "webhookRetentionDays", 90),
            "auditRetentionDays": stored_i64(&stored, "auditRetentionDays", 365),
            "reconcileIntervalSeconds": stored_i64(&stored, "reconcileIntervalSeconds", 30),
            "githubSyncIntervalSeconds": stored_i64(&stored, "githubSyncIntervalSeconds", 60),
            "autoUpdateImages": stored.get("autoUpdateImages").and_then(Value::as_bool).unwrap_or(false),
        }, "user": { "login": user.login, "role": user.role }
    }})))
}

pub async fn save_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<SystemSettings>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    require_system_admin(&user)?;
    if !(1..=3_650).contains(&input.log_retention_days)
        || !(1..=3_650).contains(&input.webhook_retention_days)
        || !(1..=3_650).contains(&input.audit_retention_days)
        || !(5..=3_600).contains(&input.reconcile_interval_seconds)
        || !(30..=3_600).contains(&input.github_sync_interval_seconds)
    {
        return Err(ApiError::BadRequest(
            "Retention or reconciliation settings are outside the supported range.".into(),
        ));
    }
    let values = [
        ("logRetentionDays", json!(input.log_retention_days)),
        ("webhookRetentionDays", json!(input.webhook_retention_days)),
        ("auditRetentionDays", json!(input.audit_retention_days)),
        (
            "reconcileIntervalSeconds",
            json!(input.reconcile_interval_seconds),
        ),
        (
            "githubSyncIntervalSeconds",
            json!(input.github_sync_interval_seconds),
        ),
        ("autoUpdateImages", json!(input.auto_update_images)),
    ];
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    for (key, value) in values {
        sqlx::query("INSERT INTO settings (key,value,updated_by,updated_at) VALUES (?,?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_by=excluded.updated_by,updated_at=excluded.updated_at")
            .bind(key).bind(value.to_string()).bind(&user.id).bind(now).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    audit(&state, &user, "settings.updated", "system", Some("gridops"), json!({
        "logRetentionDays": input.log_retention_days, "webhookRetentionDays": input.webhook_retention_days,
        "auditRetentionDays": input.audit_retention_days, "reconcileIntervalSeconds": input.reconcile_interval_seconds,
        "githubSyncIntervalSeconds": input.github_sync_interval_seconds,
        "autoUpdateImages": input.auto_update_images,
    })).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn database_backup(State(state): State<AppState>, user: AuthUser) -> ApiResult<Response> {
    require_system_admin(&user)?;
    let backup_name = format!(
        "gridops-backup-{}.sqlite",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let backup_path = state.config.database_path().with_file_name(format!(
        "{}.{}",
        backup_name,
        uuid::Uuid::new_v4()
    ));
    sqlx::query("VACUUM INTO ?")
        .bind(backup_path.to_string_lossy().as_ref())
        .execute(&state.database)
        .await?;
    let bytes = tokio::fs::read(&backup_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    tokio::fs::remove_file(&backup_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/vnd.sqlite3"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{backup_name}\""),
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        Body::from(bytes),
    )
        .into_response())
}

async fn provision(state: &AppState, user: &AuthUser, pool_id: &str) -> ApiResult<Value> {
    let pool = pool_access(state, user, pool_id).await?;
    assert_installation_admin(state, user, pool.installation_id).await?;
    if pool.paused {
        return Err(ApiError::Conflict("Runner pool is paused.".into()));
    }
    let runner_id = uuid::Uuid::new_v4().to_string();
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let runner_name = format!("{}-{suffix}", pool.name);
    let now = now_millis();
    sqlx::query("INSERT INTO runners (id,pool_id,name,status,ephemeral,configuration_version,created_at,updated_at) VALUES (?,?,?,'starting',?,?,?,?)")
        .bind(&runner_id).bind(pool_id).bind(&runner_name).bind(pool.ephemeral)
        .bind(pool.configuration_version).bind(now).bind(now).execute(&state.database).await?;
    let result = async {
        let token = control_token(state, &user.id, pool.installation_id).await?;
        let labels = json_array(&pool.labels);
        let target = match (&pool.repository_owner, &pool.repository_name) {
            (Some(owner), Some(repository)) => RunnerTarget::Repository { owner, repository },
            _ => RunnerTarget::Organization { organization: &pool.account_login },
        };
        let mut request = json!({
            "runnerId": runner_id, "poolId": pool_id, "name": runner_name, "image": pool.image,
            "mode": pool.mode, "labels": labels, "cpuLimit": pool.cpu_limit,
            "memoryLimitMb": pool.memory_limit_mb, "network": state.config.runner_network(),
            "pullImage": setting_bool(state, "autoUpdateImages", false).await,
        });
        let github_runner_id = if pool.ephemeral {
            let jit = state.github.generate_jit_config(target, &token, &JitRequest {
                name: runner_name.clone(), runner_group_id: pool.runner_group_id,
                labels: json_array(&pool.labels), work_folder: "_work".into(),
            }).await.map_err(ApiError::Internal)?;
            request["jitConfig"] = Value::String(jit.encoded_jit_config);
            Some(jit.runner.id)
        } else {
            let registration = state.github.generate_registration_token(target, &token)
                .await.map_err(ApiError::Internal)?;
            request["registrationToken"] = Value::String(registration.token);
            request["registrationUrl"] = Value::String(runner_registration_url(&pool));
            if pool.repository_owner.is_none() && pool.runner_group_id != 1 {
                let group = state.github.runner_group_name(
                    &pool.account_login, pool.runner_group_id, &token,
                ).await.map_err(ApiError::Internal)?;
                request["runnerGroup"] = Value::String(group);
            }
            None
        };
        let manager = manager_json(state, Method::POST, "v1/runners", Some(request)).await?;
        let container_id = manager.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::ServiceUnavailable("Runner manager returned an invalid container identifier.".into()))?;
        let status = if manager.get("state").and_then(Value::as_str) == Some("running") { "online" } else { "starting" };
        let updated = now_millis();
        sqlx::query("UPDATE runners SET github_runner_id=?,container_id=?,container_name=?,status=?,registered_at=?,last_heartbeat_at=?,updated_at=? WHERE id=?")
            .bind(github_runner_id).bind(container_id).bind(manager.get("name").and_then(Value::as_str)).bind(status)
            .bind(updated).bind(updated).bind(updated).bind(&runner_id).execute(&state.database).await?;
        sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,event,message,metadata,created_at) VALUES (?,?,?,'Runner started',?,?,?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&runner_id).bind(pool_id).bind(format!("{runner_name} started in pool {}", pool.name))
            .bind(json!({ "containerId": container_id, "githubRunnerId": github_runner_id, "mode": pool.mode }).to_string()).bind(updated).execute(&state.database).await?;
        audit(state, user, "runner.provisioned", "runner", Some(&runner_id), json!({ "poolId": pool_id, "containerId": container_id, "githubRunnerId": github_runner_id, "mode": pool.mode })).await?;
        Ok::<Value, ApiError>(json!({ "runnerId": runner_id, "status": status }))
    }.await;
    if let Err(error) = &result {
        let message = error.to_string().chars().take(2_000).collect::<String>();
        sqlx::query("UPDATE runners SET status='failed',failure_reason=?,updated_at=? WHERE id=?")
            .bind(&message)
            .bind(now_millis())
            .bind(&runner_id)
            .execute(&state.database)
            .await?;
        sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,level,event,message,created_at) VALUES (?,?,?,'error','Runner provisioning failed',?,?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&runner_id).bind(pool_id).bind(message).bind(now_millis()).execute(&state.database).await?;
    }
    result
}

async fn set_pool_paused(
    state: &AppState,
    user: &AuthUser,
    pool_id: &str,
    paused: bool,
) -> ApiResult<()> {
    let pool = pool_access(state, user, pool_id).await?;
    assert_installation_admin(state, user, pool.installation_id).await?;
    sqlx::query("UPDATE runner_pools SET paused=?,state=?,updated_at=? WHERE id=?")
        .bind(paused)
        .bind(if paused { "draining" } else { "active" })
        .bind(now_millis())
        .bind(pool_id)
        .execute(&state.database)
        .await?;
    if paused {
        for runner in runners_for_pool(state, pool_id)
            .await?
            .iter()
            .filter(|runner| !runner.busy)
        {
            delete_runner_resources(state, user, runner).await?;
        }
    } else {
        reconcile_pool(state, user, pool_id).await?;
    }
    audit(
        state,
        user,
        if paused {
            "runner_pool.paused"
        } else {
            "runner_pool.resumed"
        },
        "runner_pool",
        Some(pool_id),
        json!({}),
    )
    .await?;
    Ok(())
}

async fn reconcile_pool(state: &AppState, user: &AuthUser, pool_id: &str) -> ApiResult<Value> {
    let pool = pool_access(state, user, pool_id).await?;
    assert_installation_admin(state, user, pool.installation_id).await?;
    let known = runners_for_pool(state, pool_id).await?;
    let managed = match manager_json(state, Method::GET, "v1/runners", None).await {
        Ok(value) => value,
        Err(error) if known.iter().all(|runner| runner.container_id.is_none()) => {
            tracing::warn!(pool_id, error = ?error, "runner manager unavailable before first provision");
            json!({ "runners": [] })
        }
        Err(error) => return Err(error),
    };
    let states = managed
        .get("runners")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|runner| {
            Some((
                runner.get("id")?.as_str()?.to_owned(),
                runner.get("state")?.as_str()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    for runner in &known {
        if let Some(container_id) = &runner.container_id {
            let docker_state = states
                .get(container_id)
                .map(String::as_str)
                .unwrap_or("missing");
            let status = match docker_state {
                "running" if runner.busy => "busy",
                "running" => "online",
                "paused" => "paused",
                "exited" | "dead" if runner.runner_status == "stopped" => "stopped",
                "exited" | "dead" | "missing" => "failed",
                other => other,
            };
            let heartbeat = now_millis();
            sqlx::query(
                "UPDATE runners SET status=?,last_heartbeat_at=?,updated_at=CASE WHEN status<>? THEN ? ELSE updated_at END WHERE id=?",
            )
                .bind(status)
                .bind(heartbeat)
                .bind(status)
                .bind(heartbeat)
                .bind(&runner.runner_id)
                .execute(&state.database)
                .await?;
        }
    }
    let failed = runners_for_pool(state, pool_id)
        .await?
        .into_iter()
        .filter(|runner| runner.runner_status == "failed")
        .collect::<Vec<_>>();
    for runner in &failed {
        delete_runner_resources(state, user, runner).await?;
    }
    let mut rotated = 0;
    if let Some(stale) = runners_for_pool(state, pool_id)
        .await?
        .into_iter()
        .find(|runner| !runner.busy && runner.configuration_version < pool.configuration_version)
    {
        delete_runner_resources(state, user, &stale).await?;
        rotated = 1;
    }
    if pool.paused {
        let idle = runners_for_pool(state, pool_id)
            .await?
            .into_iter()
            .filter(|runner| !runner.busy && active_status(&runner.runner_status))
            .collect::<Vec<_>>();
        let removed = idle.len();
        for runner in &idle {
            delete_runner_resources(state, user, runner).await?;
        }
        let active = runners_for_pool(state, pool_id)
            .await?
            .into_iter()
            .filter(|runner| active_status(&runner.runner_status))
            .count();
        sqlx::query("UPDATE runner_pools SET state=?,updated_at=? WHERE id=?")
            .bind(if active == 0 { "paused" } else { "draining" })
            .bind(now_millis())
            .bind(pool_id)
            .execute(&state.database)
            .await?;
        return Ok(json!({
            "ok": true, "desired": pool.desired_count, "active": active,
            "provisioned": 0, "removed": removed,
        }));
    }
    let refreshed = runners_for_pool(state, pool_id).await?;
    let active = refreshed
        .iter()
        .filter(|runner| active_status(&runner.runner_status))
        .collect::<Vec<_>>();
    let mut provisioned = 0;
    let mut removed = 0;
    if active.len() < pool.desired_count as usize {
        for _ in active.len()..pool.desired_count as usize {
            provision(state, user, pool_id).await?;
            provisioned += 1;
        }
    } else if active.len() > pool.desired_count as usize {
        let count = active.len() - pool.desired_count as usize;
        for runner in active.into_iter().filter(|runner| !runner.busy).take(count) {
            delete_runner_resources(state, user, runner).await?;
            removed += 1;
        }
    }
    let final_runners = runners_for_pool(state, pool_id).await?;
    let final_active = final_runners
        .iter()
        .filter(|runner| active_status(&runner.runner_status))
        .count();
    let outdated = final_runners
        .iter()
        .filter(|runner| runner.configuration_version < pool.configuration_version)
        .count();
    sqlx::query("UPDATE runner_pools SET state=?,updated_at=? WHERE id=?")
        .bind(if outdated > 0 {
            "updating"
        } else if final_active > pool.desired_count as usize {
            "draining"
        } else {
            "active"
        })
        .bind(now_millis())
        .bind(pool_id)
        .execute(&state.database)
        .await?;
    Ok(
        json!({ "ok": true, "desired": pool.desired_count, "active": final_active, "provisioned": provisioned, "removed": removed, "rotated": rotated, "outdated": outdated }),
    )
}

async fn delete_runner_resources(
    state: &AppState,
    user: &AuthUser,
    runner: &RunnerAccess,
) -> ApiResult<()> {
    if runner.container_id.is_some()
        && let Err(error) = archive_runner_logs(state, runner).await
    {
        tracing::warn!(runner_id = %runner.runner_id, error = ?error, "could not archive runner logs");
    }
    let github_cleanup = cleanup_github_runner(state, user, runner).await;
    let github_cleanup_error = github_cleanup
        .err()
        .map(|error| error.to_string().chars().take(2_000).collect::<String>());
    if let Some(error) = &github_cleanup_error {
        tracing::warn!(runner_id = %runner.runner_id, error, "GitHub runner cleanup deferred");
        let now = now_millis();
        sqlx::query(
            r#"INSERT INTO github_runner_cleanup (
              id,installation_id,target_owner,target_repository,github_runner_id,runner_name,
              attempts,last_error,next_attempt_at,created_at,updated_at
            ) VALUES (?,?,?,?,?,?,0,?,?,?,?) ON CONFLICT(id) DO UPDATE SET
              installation_id=excluded.installation_id,target_owner=excluded.target_owner,
              target_repository=excluded.target_repository,
              github_runner_id=COALESCE(excluded.github_runner_id,github_runner_cleanup.github_runner_id),
              runner_name=excluded.runner_name,last_error=excluded.last_error,
              next_attempt_at=excluded.next_attempt_at,updated_at=excluded.updated_at"#,
        )
        .bind(&runner.runner_id)
        .bind(runner.installation_id)
        .bind(
            runner
                .repository_owner
                .as_deref()
                .unwrap_or(&runner.account_login),
        )
        .bind(&runner.repository_name)
        .bind(runner.github_runner_id)
        .bind(&runner.runner_name)
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&state.database)
        .await?;
    } else {
        sqlx::query("DELETE FROM github_runner_cleanup WHERE id=?")
            .bind(&runner.runner_id)
            .execute(&state.database)
            .await?;
    }
    if let Some(container_id) = &runner.container_id {
        if let Err(error) = manager_json(
            state,
            Method::DELETE,
            &format!("v1/runners/{container_id}"),
            None,
        )
        .await
        {
            if !matches!(error, ApiError::NotFound(_)) {
                return Err(error);
            }
        }
    }
    let now = now_millis();
    sqlx::query("UPDATE runners SET status='deleted',busy=0,deleted_at=?,updated_at=? WHERE id=?")
        .bind(now)
        .bind(now)
        .bind(&runner.runner_id)
        .execute(&state.database)
        .await?;
    sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,level,event,message,metadata,created_at) VALUES (?,?,?,?, 'Runner deleted',?,?,?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(&runner.runner_id).bind(&runner.pool_id)
        .bind(if github_cleanup_error.is_some() { "warn" } else { "info" })
        .bind(if github_cleanup_error.is_some() {
            format!("{} was removed locally; GitHub cleanup will retry", runner.runner_name)
        } else {
            format!("{} was removed", runner.runner_name)
        })
        .bind(json!({
            "githubCleanup": if github_cleanup_error.is_some() { "deferred" } else { "complete" },
            "error": github_cleanup_error,
        }).to_string())
        .bind(now).execute(&state.database).await?;
    Ok(())
}

async fn cleanup_github_runner(
    state: &AppState,
    user: &AuthUser,
    runner: &RunnerAccess,
) -> ApiResult<()> {
    let token = control_token(state, &user.id, runner.installation_id).await?;
    let target = match (&runner.repository_owner, &runner.repository_name) {
        (Some(owner), Some(repository)) => RunnerTarget::Repository { owner, repository },
        _ => RunnerTarget::Organization {
            organization: &runner.account_login,
        },
    };
    let github_runner_id = match runner.github_runner_id {
        Some(id) => Some(id),
        None => state
            .github
            .runner_by_name(target, &token, &runner.runner_name)
            .await
            .map_err(ApiError::Internal)?
            .map(|runner| runner.id),
    };
    if let Some(github_runner_id) = github_runner_id {
        let path = match (&runner.repository_owner, &runner.repository_name) {
            (Some(owner), Some(repository)) => {
                format!("/repos/{owner}/{repository}/actions/runners/{github_runner_id}")
            }
            _ => format!(
                "/orgs/{}/actions/runners/{github_runner_id}",
                runner.account_login
            ),
        };
        state
            .github
            .delete(&path, &token)
            .await
            .map_err(ApiError::Internal)?;
    }
    Ok(())
}

async fn pool_access(state: &AppState, user: &AuthUser, pool_id: &str) -> ApiResult<PoolAccess> {
    sqlx::query_as::<_, PoolAccess>(r#"SELECT p.installation_id,ui.permission AS installation_permission,i.account_login,
      p.repository_id,repo.owner AS repository_owner,repo.name AS repository_name,p.name,p.scope,
      p.mode,p.labels,p.image,p.desired_count,p.min_count,p.max_count,p.cpu_limit,p.memory_limit_mb,
      p.runner_group_id,p.ephemeral,p.paused,p.state,p.autoscaling_enabled,p.queue_scale_factor,
      p.idle_timeout_minutes,p.configuration_version
      FROM runner_pools p JOIN user_installations ui ON ui.installation_id=p.installation_id
      JOIN installations i ON i.id=p.installation_id LEFT JOIN repositories repo ON repo.id=p.repository_id
      WHERE p.id=? AND ui.user_id=?"#)
        .bind(pool_id).bind(&user.id).fetch_optional(&state.database).await?
        .ok_or_else(|| ApiError::NotFound("Runner pool does not exist or is not accessible.".into()))
}

async fn runner_access(
    state: &AppState,
    user: &AuthUser,
    runner_id: &str,
) -> ApiResult<RunnerAccess> {
    sqlx::query_as::<_, RunnerAccess>(r#"SELECT r.id AS runner_id,r.name AS runner_name,r.container_id,
      r.github_runner_id,r.status AS runner_status,r.busy,r.ephemeral,r.configuration_version,
      r.last_job_id,p.id AS pool_id,p.name AS pool_name,p.installation_id,i.account_login,
      repo.owner AS repository_owner,repo.name AS repository_name FROM runners r
      JOIN runner_pools p ON p.id=r.pool_id JOIN user_installations ui ON ui.installation_id=p.installation_id
      JOIN installations i ON i.id=p.installation_id LEFT JOIN repositories repo ON repo.id=p.repository_id
      WHERE r.id=? AND ui.user_id=? AND r.deleted_at IS NULL"#)
        .bind(runner_id).bind(&user.id).fetch_optional(&state.database).await?
        .ok_or_else(|| ApiError::NotFound("Runner does not exist or is not accessible.".into()))
}

async fn runners_for_pool(state: &AppState, pool_id: &str) -> ApiResult<Vec<RunnerAccess>> {
    Ok(sqlx::query_as::<_, RunnerAccess>(r#"SELECT r.id AS runner_id,r.name AS runner_name,r.container_id,
      r.github_runner_id,r.status AS runner_status,r.busy,r.ephemeral,r.configuration_version,
      r.last_job_id,p.id AS pool_id,p.name AS pool_name,p.installation_id,i.account_login,
      repo.owner AS repository_owner,repo.name AS repository_name FROM runners r
      JOIN runner_pools p ON p.id=r.pool_id JOIN installations i ON i.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=p.repository_id WHERE p.id=? AND r.deleted_at IS NULL ORDER BY r.created_at DESC"#)
        .bind(pool_id).fetch_all(&state.database).await?)
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
        } else if status == StatusCode::CONFLICT {
            ApiError::Conflict(message)
        } else {
            ApiError::ServiceUnavailable(message)
        });
    }
    serde_json::from_str(&text).map_err(|error| ApiError::Internal(error.into()))
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
        "SELECT id FROM log_streams WHERE runner_id=? AND source='docker' AND complete=1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&runner.runner_id)
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
        ) VALUES (?,?,?,?,?,?,?,'docker',?, ?,1,?,?,?,?)"#,
    )
    .bind(&stream_id)
    .bind(&runner.runner_id)
    .bind(runner.last_job_id)
    .bind(runner.installation_id)
    .bind(&runner.runner_name)
    .bind(&runner.pool_name)
    .bind(repository)
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
        webhook_url: state
            .config
            .base_url()
            .join("/api/webhooks/github")
            .map_or_else(|_| "/api/webhooks/github".into(), |url| url.to_string()),
    })
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

fn empty_page() -> Json<Value> {
    Json(json!({ "authenticated": false, "items": [] }))
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

fn capacity_window(window: &str) -> Option<(i64, i64)> {
    match window {
        "24h" => Some((86_400_000, 5 * 60_000)),
        "7d" => Some((7 * 86_400_000, 30 * 60_000)),
        "30d" => Some((30 * 86_400_000, 2 * 60 * 60_000)),
        _ => None,
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

fn runner_registration_url(pool: &PoolAccess) -> String {
    match (&pool.repository_owner, &pool.repository_name) {
        (Some(owner), Some(repository)) => format!("https://github.com/{owner}/{repository}"),
        _ => format!("https://github.com/{}", pool.account_login),
    }
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
}
