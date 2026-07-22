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
    effective_runner_labels, next_runner_repository, now_millis,
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
        FROM runner_pools p
        LEFT JOIN runners r ON r.pool_id=p.id
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )"#,
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
              EXISTS (SELECT 1 FROM runner_pool_repositories membership
                WHERE membership.pool_id=p.id AND membership.repository_id=queued_repo.id) OR
              queued_repo.id=p.repository_id OR
              (p.scope='organization' AND queued_repo.installation_id=p.installation_id)
            )) AS queued
        FROM runner_pools p LEFT JOIN runners r ON r.pool_id=p.id
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
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
        r#"SELECT re.id,re.level,re.event,re.message,re.runner_id,re.pool_id,re.created_at FROM runner_events re
        WHERE EXISTS (
          SELECT 1 FROM runner_pools p WHERE p.id=re.pool_id
            AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                AND NOT EXISTS (SELECT 1 FROM user_installations access
                  WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
            )
        )
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
                "runnerId": row.try_get::<Option<String>, _>("runner_id").ok().flatten(),
                "poolId": row.try_get::<Option<String>, _>("pool_id").ok().flatten(),
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
            FROM capacity_samples cs JOIN runner_pools p ON p.id=cs.pool_id
            WHERE cs.recorded_at>=?
              AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
              AND NOT EXISTS (
                SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                  AND NOT EXISTS (SELECT 1 FROM user_installations access
                    WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
              )
            GROUP BY cs.pool_id,bucket
          ) samples GROUP BY bucket ORDER BY bucket"#,
    )
    .bind(bucket_millis)
    .bind(bucket_millis)
    .bind(cutoff)
    .bind(&user.id)
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
    Query(query): Query<RepositoryQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let requested_page = query_page(query.page);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let query = query.q.unwrap_or_default().trim().to_owned();
    if query.len() > 100 {
        return Err(ApiError::BadRequest(
            "Repository search is limited to 100 characters.".into(),
        ));
    }
    let Some(user) = user else {
        return Ok(Json(json!({
            "authenticated": false, "items": [], "total": 0, "page": requested_page,
            "perPage": per_page, "query": query,
        })));
    };
    let (_, mut available) = available_repositories(&state, &user, false).await?;
    if !query.is_empty() {
        let needle = query.to_lowercase();
        available.retain(|item| item.repository.full_name.to_lowercase().contains(&needle));
    }
    available.sort_by(|left, right| {
        left.repository
            .full_name
            .to_lowercase()
            .cmp(&right.repository.full_name.to_lowercase())
    });
    let total = i64::try_from(available.len()).unwrap_or(i64::MAX);
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let limit = usize::try_from(per_page).unwrap_or(100);
    let stored_rows = sqlx::query(
        r#"SELECT repo.id,repo.last_synced_at,
          (SELECT COUNT(*) FROM runner_pool_repositories membership
            WHERE membership.repository_id=repo.id) AS pool_count,
          COUNT(DISTINCT wr.id) AS run_count,MAX(wr.github_updated_at) AS last_run_at
        FROM repositories repo JOIN user_installations ui ON ui.installation_id=repo.installation_id
        LEFT JOIN workflow_runs wr ON wr.repository_id=repo.id
        WHERE ui.user_id=? GROUP BY repo.id"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let stored = stored_rows
        .iter()
        .map(|row| {
            (
                row.get::<i64, _>("id"),
                RepositoryStats {
                    last_synced_at: row.get::<i64, _>("last_synced_at"),
                    pool_count: row.get::<i64, _>("pool_count"),
                    run_count: row.get::<i64, _>("run_count"),
                    last_run_at: row.try_get::<Option<i64>, _>("last_run_at").ok().flatten(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let fetched_at = now_millis();
    let items = available
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|item| {
            let repository = item.repository;
            let stats = stored.get(&repository.id);
            let permission = repository.permissions.as_ref().and_then(|permissions| {
                permissions
                    .iter()
                    .find_map(|(name, allowed)| allowed.then_some(name))
            });
            json!({
                "id": repository.id, "fullName": repository.full_name,
                "private": repository.private, "archived": repository.archived,
                "defaultBranch": repository.default_branch, "htmlUrl": repository.html_url,
                "permission": permission, "connected": stats.is_some(),
                "lastSyncedAt": iso(stats.map_or(fetched_at, |value| value.last_synced_at)),
                "installationId": item.installation.id, "accountLogin": item.installation.account_login,
                "accountType": item.installation.account_type,
                "repositorySelection": item.installation.repository_selection,
                "poolCount": stats.map_or(0, |value| value.pool_count),
                "runCount": stats.map_or(0, |value| value.run_count),
                "lastRunAt": iso_optional(stats.and_then(|value| value.last_run_at)),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "authenticated": true, "items": items, "total": total, "page": page,
        "perPage": per_page, "query": query,
    })))
}

async fn available_repositories(
    state: &AppState,
    user: &AuthUser,
    require_admin: bool,
) -> ApiResult<(Vec<InstallationAccess>, Vec<AvailableRepository>)> {
    let installations = available_installations(state, user, require_admin).await?;
    let groups = stream::iter(installations.iter().cloned())
        .map(|installation| async move {
            let repositories =
                available_repositories_for_installation(state, user, &installation).await?;
            Ok::<Vec<AvailableRepository>, ApiError>(
                repositories
                    .into_iter()
                    .map(|repository| AvailableRepository {
                        installation: installation.clone(),
                        repository,
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .buffered(4)
        .try_collect::<Vec<_>>()
        .await?;
    let mut available = Vec::new();
    for group in groups {
        available.extend(group);
    }
    Ok((installations, available))
}

async fn available_installations(
    state: &AppState,
    user: &AuthUser,
    require_admin: bool,
) -> ApiResult<Vec<InstallationAccess>> {
    let rows = sqlx::query(
        r#"SELECT i.id,i.account_login,i.account_type,i.repository_selection
           FROM user_installations ui JOIN installations i ON i.id=ui.installation_id
           WHERE ui.user_id=? AND i.suspended_at IS NULL
             AND (?=0 OR ui.permission='admin' OR ?='admin')
           ORDER BY i.account_login"#,
    )
    .bind(&user.id)
    .bind(require_admin)
    .bind(&user.role)
    .fetch_all(&state.database)
    .await?;
    Ok(rows
        .iter()
        .map(|row| InstallationAccess {
            id: row.get::<i64, _>("id"),
            account_login: row.get::<String, _>("account_login"),
            account_type: row.get::<String, _>("account_type"),
            repository_selection: row.get::<String, _>("repository_selection"),
        })
        .collect::<Vec<_>>())
}

async fn available_repositories_for_installation(
    state: &AppState,
    user: &AuthUser,
    installation: &InstallationAccess,
) -> ApiResult<Vec<GitHubRepository>> {
    let token = control_token(state, &user.id, installation.id).await?;
    let first_page: RepositoryPage = state
        .github
        .get("/installation/repositories?per_page=100&page=1", &token)
        .await
        .map_err(ApiError::Internal)?;
    let expected_total = first_page.total_count;
    let mut repositories = first_page.repositories;
    let last_page = expected_total
        .saturating_add(99)
        .div_euclid(100)
        .clamp(1, 100);
    if last_page > 1 {
        let github = &state.github;
        let token = &token;
        let remaining = stream::iter(2..=last_page)
            .map(|page| async move {
                github
                    .get::<RepositoryPage>(
                        &format!("/installation/repositories?per_page=100&page={page}"),
                        token,
                    )
                    .await
                    .map_err(ApiError::Internal)
            })
            .buffered(4)
            .try_collect::<Vec<_>>()
            .await?;
        for page in remaining {
            repositories.extend(page.repositories);
        }
    }
    let loaded = i64::try_from(repositories.len()).unwrap_or(i64::MAX);
    if loaded < expected_total {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "GitHub returned {loaded} of {expected_total} repositories for installation {}",
            installation.id
        )));
    }
    Ok(repositories)
}

async fn selected_available_repositories(
    state: &AppState,
    user: &AuthUser,
    repository_ids: &[i64],
) -> ApiResult<Vec<AvailableRepository>> {
    let requested = repository_ids.iter().copied().collect::<HashSet<_>>();
    let (_, available) = available_repositories(state, user, true).await?;
    let mut selected = available
        .into_iter()
        .filter(|item| requested.contains(&item.repository.id) && !item.repository.archived)
        .collect::<Vec<_>>();
    if selected.len() != requested.len() {
        return Err(ApiError::BadRequest(
            "One or more selected repositories are unavailable or you cannot administer their GitHub App installation."
                .into(),
        ));
    }
    selected.sort_by_key(|item| item.repository.id);
    Ok(selected)
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
    let pattern = like_pattern(query);
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
          LEFT JOIN repositories repo ON repo.id=p.repository_id
          WHERE p.name LIKE ? ESCAPE '\' AND EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
          ) AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
          UNION ALL
          SELECT 'runner',r.id,r.name,p.name,'/runners',r.name FROM runners r
          JOIN runner_pools p ON p.id=r.pool_id
          WHERE r.deleted_at IS NULL AND r.name LIKE ? ESCAPE '\' AND EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
          ) AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
          UNION ALL
          SELECT 'workflow run',CAST(wr.id AS TEXT),wr.workflow_name,repo.full_name,
            '/workflow-runs/' || wr.id,wr.workflow_name FROM workflow_runs wr
          JOIN repositories repo ON repo.id=wr.repository_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE ui.user_id=? AND (wr.workflow_name LIKE ? ESCAPE '\' OR repo.full_name LIKE ? ESCAPE '\')
        ) ORDER BY sort_value LIMIT 12"#,
    )
    .bind(&user.id).bind(&pattern).bind(&pattern).bind(&user.id)
    .bind(&pattern).bind(&user.id).bind(&user.id).bind(&pattern).bind(&pattern)
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
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM runner_pools p
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT p.id,p.name,p.scope,p.mode,p.provider,p.labels,p.image,p.desired_count,p.min_count,
          p.max_count,CAST(p.cpu_limit AS REAL) AS cpu_limit,p.memory_limit_mb,p.paused,p.state,
          p.provision_failure_count,p.provision_retry_at,p.provision_circuit_open,i.account_login,
          CASE WHEN NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations manage
                WHERE manage.user_id=? AND manage.installation_id=mapped.installation_id
                  AND manage.permission='admin')
          ) THEN 'admin' ELSE 'read' END AS installation_permission,
          repo.full_name AS repository,
          (SELECT COUNT(*) FROM runner_pool_repositories membership WHERE membership.pool_id=p.id) AS repository_count,
          (SELECT COUNT(*) FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id) AS account_count,
          COUNT(CASE WHEN r.deleted_at IS NULL THEN 1 END) AS total_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('online','idle','busy') THEN 1 END) AS online_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN 1 END) AS busy_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status='failed' THEN 1 END) AS failed_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.configuration_version < p.configuration_version THEN 1 END) AS outdated_runners,
          p.created_at FROM runner_pools p
        JOIN installations i ON i.id=p.installation_id LEFT JOIN repositories repo ON repo.id=p.repository_id
        LEFT JOIN runners r ON r.pool_id=p.id
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
        GROUP BY p.id ORDER BY p.created_at DESC
        LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "name": row.get::<String,_>("name"), "scope": row.get::<String,_>("scope"),
        "mode": row.get::<String,_>("mode"), "provider": row.get::<String,_>("provider"),
        "labels": json_array(row.get::<&str,_>("labels")), "image": row.get::<String,_>("image"),
        "desiredCount": row.get::<i64,_>("desired_count"), "minCount": row.get::<i64,_>("min_count"), "maxCount": row.get::<i64,_>("max_count"),
        "cpuLimit": row.get::<f64,_>("cpu_limit"), "memoryLimitMb": row.get::<i64,_>("memory_limit_mb"),
        "paused": row.get::<bool,_>("paused"), "state": row.get::<String,_>("state"), "accountLogin": row.get::<String,_>("account_login"),
        "provisionFailureCount": row.get::<i64,_>("provision_failure_count"),
        "provisionRetryAt": iso_optional(row.try_get::<Option<i64>,_>("provision_retry_at").ok().flatten()),
        "provisionCircuitOpen": row.get::<bool,_>("provision_circuit_open"),
        "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(), "repositoryCount": row.get::<i64,_>("repository_count"),
        "accountCount": row.get::<i64,_>("account_count"), "totalRunners": row.get::<i64,_>("total_runners"),
        "onlineRunners": row.get::<i64,_>("online_runners"), "busyRunners": row.get::<i64,_>("busy_runners"),
        "failedRunners": row.get::<i64,_>("failed_runners"), "outdatedRunners": row.get::<i64,_>("outdated_runners"),
        "canManage": user.role == "admin" || row.get::<String,_>("installation_permission") == "admin",
        "createdAt": iso(row.get::<i64,_>("created_at")),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
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
    let mut repositories = sqlx::query(
        r#"SELECT repo.id,repo.installation_id,repo.full_name,repo.private,
          installation.account_login,installation.account_type
          FROM runner_pool_repositories membership
           JOIN repositories repo ON repo.id=membership.repository_id
           JOIN installations installation ON installation.id=repo.installation_id
           WHERE membership.pool_id=? ORDER BY membership.created_at,repo.id"#,
    )
    .bind(&pool_id)
    .fetch_all(&state.database)
    .await?
    .into_iter()
    .map(|row| {
        json!({
            "id": row.get::<i64, _>("id"),
            "installationId": row.get::<i64, _>("installation_id"),
            "accountLogin": row.get::<String, _>("account_login"),
            "accountType": row.get::<String, _>("account_type"),
            "fullName": row.get::<String, _>("full_name"),
            "private": row.get::<bool, _>("private"),
        })
    })
    .collect::<Vec<_>>();
    if repositories.is_empty()
        && let Some(repository_id) = pool.repository_id
        && let Some(repository) = &repository
    {
        repositories.push(json!({
            "id": repository_id, "installationId": pool.installation_id,
            "accountLogin": pool.account_login, "accountType": "Unknown",
            "fullName": repository, "private": false
        }));
    }
    let repository_ids = repositories
        .iter()
        .filter_map(|repository| repository.get("id").and_then(Value::as_i64))
        .collect::<Vec<_>>();
    let (max_cpu_limit, max_memory_limit_mb) = manager_resource_capacity(&state).await;
    Ok(Json(json!({
        "id": pool_id,
        "installationId": pool.installation_id,
        "repositoryId": pool.repository_id,
        "repository": repository,
        "repositoryIds": repository_ids,
        "repositories": repositories,
        "accountLogin": pool.account_login,
        "name": pool.name,
        "scope": pool.scope,
        "mode": pool.mode,
        "provider": pool.provider,
        "labels": additional_labels,
        "image": pool.image,
        "dockerImage": state.config.runner_image(),
        "tartImage": default_tart_image(),
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
        "maxCpuLimit": max_cpu_limit,
        "maxMemoryLimitMb": max_memory_limit_mb,
        "configurationVersion": pool.configuration_version,
        "provisionFailureCount": pool.provision_failure_count,
        "provisionRetryAt": iso_optional(pool.provision_retry_at),
        "provisionCircuitOpen": pool.provision_circuit_open,
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
    assert_pool_admin(&state, &user, &pool_id).await?;
    let (max_cpu_limit, max_memory_limit_mb) = manager_resource_capacity(&state).await;
    if input.cpu_limit > max_cpu_limit as f64 {
        return Err(ApiError::BadRequest(format!(
            "CPU limit cannot exceed host CPU capacity ({max_cpu_limit})."
        )));
    }
    if input.memory_limit_mb > max_memory_limit_mb {
        return Err(ApiError::BadRequest(format!(
            "Memory limit cannot exceed host runner budget ({max_memory_limit_mb} MB)."
        )));
    }
    let existing_repository_ids = sqlx::query_scalar::<_, i64>(
        "SELECT repository_id FROM runner_pool_repositories WHERE pool_id=? ORDER BY created_at,repository_id",
    )
    .bind(&pool_id)
    .fetch_all(&state.database)
    .await?;
    let repository_ids = if pool.scope == "repository" {
        input.repository_ids.clone().unwrap_or_else(|| {
            if existing_repository_ids.is_empty() {
                pool.repository_id.into_iter().collect()
            } else {
                existing_repository_ids.clone()
            }
        })
    } else {
        Vec::new()
    };
    if pool.scope == "repository" && repository_ids.is_empty() {
        return Err(ApiError::BadRequest(
            "A repository pool requires at least one repository.".into(),
        ));
    }
    if i64::try_from(repository_ids.len()).unwrap_or(i64::MAX) > input.max_count {
        return Err(ApiError::BadRequest(
            "Repository count cannot exceed maximum runner capacity.".into(),
        ));
    }
    if input.repository_ids.is_some() && pool.scope != "repository" {
        return Err(ApiError::BadRequest(
            "Organization pools use runner-group repository access.".into(),
        ));
    }
    let selected = if input.repository_ids.is_some() {
        selected_available_repositories(&state, &user, &repository_ids).await?
    } else {
        Vec::new()
    };
    for item in &selected {
        upsert_repository(&state, item.installation.id, &item.repository).await?;
    }
    let primary_installation_id = selected
        .first()
        .map_or(pool.installation_id, |item| item.installation.id);
    let labels = normalized_pool_labels(&input.name, &input.labels)?;
    let encoded_labels =
        serde_json::to_string(&labels).map_err(|error| ApiError::Internal(error.into()))?;
    let runner_group_id = if pool.scope == "repository" {
        1
    } else {
        input.runner_group_id
    };
    let existing = existing_repository_ids.into_iter().collect::<HashSet<_>>();
    let requested = repository_ids.iter().copied().collect::<HashSet<_>>();
    let repositories_changed = existing != requested;
    let runtime_changed = pool.name != input.name
        || pool.mode != input.mode
        || pool.provider != input.provider
        || pool.labels != encoded_labels
        || pool.image != input.image
        || (pool.cpu_limit - input.cpu_limit).abs() > f64::EPSILON
        || pool.memory_limit_mb != input.memory_limit_mb
        || pool.runner_group_id != runner_group_id
        || repositories_changed;
    let version_increment = i64::from(runtime_changed);
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    let result = sqlx::query(
        r#"UPDATE runner_pools SET installation_id=?,name=?,mode=?,provider=?,labels=?,image=?,desired_count=?,min_count=?,
          max_count=?,cpu_limit=?,memory_limit_mb=?,ephemeral=?,runner_group_id=?,
          autoscaling_enabled=?,queue_scale_factor=?,idle_timeout_minutes=?,
          repository_id=?,
          configuration_version=configuration_version+?,
          provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0,
          state=CASE WHEN ?=1 AND paused=0 THEN 'updating' ELSE state END,updated_at=? WHERE id=?"#,
    )
    .bind(primary_installation_id)
    .bind(&input.name)
    .bind(&input.mode)
    .bind(&input.provider)
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
    .bind(repository_ids.first().copied())
    .bind(version_increment)
    .bind(runtime_changed)
    .bind(now)
    .bind(&pool_id)
    .execute(&mut *transaction)
    .await;
    if let Err(sqlx::Error::Database(error)) = &result
        && error.is_unique_violation()
    {
        return Err(ApiError::Conflict(
            "A runner pool with this name already exists for the installation.".into(),
        ));
    }
    result?;
    if pool.scope == "repository" {
        sqlx::query("DELETE FROM runner_pool_repositories WHERE pool_id=?")
            .bind(&pool_id)
            .execute(&mut *transaction)
            .await?;
        for (position, repository_id) in repository_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES (?,?,?)",
            )
            .bind(&pool_id)
            .bind(repository_id)
            .bind(now.saturating_add(i64::try_from(position).unwrap_or(i64::MAX)))
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
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
            "repositoryIds": repository_ids,
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
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM runners r JOIN runner_pools p ON p.id=r.pool_id
        WHERE r.deleted_at IS NULL
          AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT r.id,r.name,r.status,r.busy,r.ephemeral,r.os,r.architecture,r.container_id,
          r.github_runner_id,r.failure_reason,r.registered_at,r.last_heartbeat_at,r.created_at,
          p.id AS pool_id,p.name AS pool_name,p.paused AS pool_paused,
          COALESCE(target_installation.account_login,primary_installation.account_login) AS account_login,
          CASE WHEN NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations manage
                WHERE manage.user_id=? AND manage.installation_id=mapped.installation_id
                  AND manage.permission='admin')
          ) THEN 'admin' ELSE 'read' END AS installation_permission,
          repo.full_name AS repository,wj.name AS current_job_name,wj.run_id AS current_run_id
        FROM runners r JOIN runner_pools p ON p.id=r.pool_id
        JOIN installations primary_installation ON primary_installation.id=p.installation_id
        LEFT JOIN repositories repo ON repo.id=r.target_repository_id
        LEFT JOIN installations target_installation ON target_installation.id=repo.installation_id
        LEFT JOIN workflow_jobs wj ON wj.id=r.current_job_id
        WHERE r.deleted_at IS NULL
          AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
        ORDER BY r.created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
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
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn workflow_runs(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM workflow_runs wr
        JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id AND ui.user_id=?"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT wr.id,wr.workflow_name,wr.run_number,wr.run_attempt,wr.event,wr.status,
          wr.conclusion,wr.head_branch,wr.head_sha,wr.actor_login,wr.html_url,wr.started_at,
          wr.completed_at,wr.github_created_at,repo.full_name,ui.permission AS installation_permission,
          COUNT(wj.id) AS job_count,COUNT(CASE WHEN wj.status='in_progress' THEN 1 END) AS active_jobs,
          COUNT(CASE WHEN wj.conclusion='failure' THEN 1 END) AS failed_jobs
        FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id AND ui.user_id=?
        LEFT JOIN workflow_jobs wj ON wj.run_id=wr.id GROUP BY wr.id
        ORDER BY wr.github_created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows
        .iter()
        .map(|row| workflow_run_json(row, user.role == "admin"))
        .collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
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
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM webhook_deliveries wd
        WHERE wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui
          WHERE ui.installation_id=wd.installation_id AND ui.user_id=?)"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT wd.id,wd.event,wd.action,wd.installation_id,wd.repository_id,
          wd.signature_valid,wd.status,wd.error,wd.received_at,wd.processed_at,
          wd.payload IS NOT NULL AS has_payload,
          CASE WHEN wd.installation_id IS NULL THEN ?='admin' ELSE EXISTS (
            SELECT 1 FROM user_installations manage WHERE manage.installation_id=wd.installation_id
              AND manage.user_id=? AND (manage.permission='admin' OR ?='admin')
          ) END AS can_retry,
          i.account_login,repo.full_name FROM webhook_deliveries wd
        LEFT JOIN installations i ON i.id=wd.installation_id LEFT JOIN repositories repo ON repo.id=wd.repository_id
        WHERE wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui
          WHERE ui.installation_id=wd.installation_id AND ui.user_id=?)
        ORDER BY wd.received_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.role)
    .bind(&user.id)
    .bind(&user.role)
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "event": row.get::<String,_>("event"), "action": row.try_get::<Option<String>,_>("action").ok().flatten(),
        "installationId": row.try_get::<Option<i64>,_>("installation_id").ok().flatten(), "repositoryId": row.try_get::<Option<i64>,_>("repository_id").ok().flatten(),
        "signatureValid": row.get::<bool,_>("signature_valid"), "status": row.get::<String,_>("status"), "error": row.try_get::<Option<String>,_>("error").ok().flatten(),
        "receivedAt": iso(row.get::<i64,_>("received_at")), "processedAt": iso_optional(row.try_get::<Option<i64>,_>("processed_at").ok().flatten()),
        "accountLogin": row.try_get::<Option<String>,_>("account_login").ok().flatten(), "repository": row.try_get::<Option<String>,_>("full_name").ok().flatten(),
        "canRetry": row.get::<bool,_>("can_retry"), "hasPayload": row.get::<bool,_>("has_payload"),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn webhook_delivery(
    State(state): State<AppState>,
    Path(delivery_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        r#"SELECT wd.id,wd.event,wd.payload FROM webhook_deliveries wd
        WHERE wd.id=? AND (
          (wd.installation_id IS NULL AND ?='admin') OR EXISTS (
            SELECT 1 FROM user_installations ui
            WHERE ui.installation_id=wd.installation_id AND ui.user_id=?
          )
        )"#,
    )
    .bind(&delivery_id)
    .bind(&user.role)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Webhook delivery does not exist or is not accessible.".into())
    })?;
    let stored = row.try_get::<Option<String>, _>("payload")?;
    let payload = stored
        .as_deref()
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored webhook payload is invalid")))?;
    Ok(Json(json!({
        "id": row.get::<String, _>("id"),
        "event": row.get::<String, _>("event"),
        "payload": payload,
        "payloadBytes": stored.as_ref().map_or(0, String::len),
    })))
}

pub async fn audit_events(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE actor_user_id=? OR actor_label='system'",
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT id,actor_label,action,target_type,target_id,metadata,ip_address,created_at
        FROM audit_events WHERE actor_user_id=? OR actor_label='system'
        ORDER BY created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "actorLabel": row.get::<String,_>("actor_label"), "action": row.get::<String,_>("action"),
        "targetType": row.get::<String,_>("target_type"), "targetId": row.try_get::<Option<String>,_>("target_id").ok().flatten(),
        "metadata": row.get::<String,_>("metadata"), "ipAddress": row.try_get::<Option<String>,_>("ip_address").ok().flatten(),
        "createdAt": iso(row.get::<i64,_>("created_at")),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn log_targets(
    State(state): State<AppState>,
    Query(query): Query<LogTargetsQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT
          (SELECT COUNT(*) FROM runners r
            JOIN runner_pools p ON p.id=r.pool_id
            JOIN workflow_jobs job ON job.id=COALESCE(r.current_job_id,r.last_job_id)
            WHERE r.deleted_at IS NULL AND r.container_id IS NOT NULL
              AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
              AND NOT EXISTS (
                SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                  AND NOT EXISTS (SELECT 1 FROM user_installations access
                    WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
              ))
          +
          (SELECT COUNT(*) FROM log_streams ls JOIN user_installations ui
            ON ui.installation_id=ls.installation_id AND ui.user_id=?
            WHERE ls.complete=1 AND ls.job_id IS NOT NULL)"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT id,runner_id,name,status,busy,container_id,updated_at,pool_name,repository,
          kind,size_bytes,job_id,run_id,job_name,job_status,job_conclusion,run_number,workflow_name
        FROM (
          SELECT r.id,CAST(NULL AS TEXT) AS runner_id,r.name,r.status,r.busy,r.container_id,
            r.updated_at,p.name AS pool_name,repo.full_name AS repository,'live' AS kind,
            CAST(NULL AS INTEGER) AS size_bytes,job.id AS job_id,job.run_id,job.name AS job_name,
            job.status AS job_status,job.conclusion AS job_conclusion,run.run_number,run.workflow_name
          FROM runners r JOIN runner_pools p ON p.id=r.pool_id
          JOIN workflow_jobs job ON job.id=COALESCE(r.current_job_id,r.last_job_id)
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=run.repository_id
          WHERE r.deleted_at IS NULL AND r.container_id IS NOT NULL
            AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                AND NOT EXISTS (SELECT 1 FROM user_installations access
                  WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
            )
          UNION ALL
          SELECT ls.id,ls.runner_id,COALESCE(ls.runner_name,'Archived runner'),'archived',0,NULL,
            ls.created_at,COALESCE(ls.pool_name,'Deleted pool'),COALESCE(repo.full_name,ls.repository),
            'archive',ls.size_bytes,job.id,job.run_id,job.name,job.status,job.conclusion,
            run.run_number,run.workflow_name
          FROM log_streams ls JOIN user_installations ui
            ON ui.installation_id=ls.installation_id AND ui.user_id=?
          JOIN workflow_jobs job ON job.id=ls.job_id
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=run.repository_id
          WHERE ls.complete=1 AND ls.job_id IS NOT NULL
        ) targets ORDER BY
          CASE WHEN id=? OR runner_id=? THEN 0 ELSE 1 END,
          CASE kind WHEN 'live' THEN 0 ELSE 1 END,busy DESC,updated_at DESC
        LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(query.target.as_deref())
    .bind(query.target.as_deref())
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "runnerId": row.try_get::<Option<String>,_>("runner_id").ok().flatten(),
        "name": row.get::<String,_>("name"), "status": row.get::<String,_>("status"),
        "busy": row.get::<bool,_>("busy"), "containerId": row.try_get::<Option<String>,_>("container_id").ok().flatten(),
        "updatedAt": iso(row.get::<i64,_>("updated_at")), "poolName": row.get::<String,_>("pool_name"),
        "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(),
        "sizeBytes": row.try_get::<Option<i64>,_>("size_bytes").ok().flatten(), "kind": row.get::<String,_>("kind"),
        "jobId": row.get::<i64,_>("job_id"), "runId": row.get::<i64,_>("run_id"),
        "jobName": row.get::<String,_>("job_name"), "jobStatus": row.get::<String,_>("job_status"),
        "jobConclusion": row.try_get::<Option<String>,_>("job_conclusion").ok().flatten(),
        "runNumber": row.get::<i64,_>("run_number"), "workflowName": row.get::<String,_>("workflow_name"),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
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
    let installation_access = available_installations(&state, &user, true).await?;
    let installations = installation_access
        .iter()
        .map(|installation| {
            json!({
                "id": installation.id, "accountLogin": installation.account_login,
                "accountType": installation.account_type,
            })
        })
        .collect::<Vec<_>>();
    let app_slug = state.github_app_slug().await.map_err(ApiError::Internal)?;
    let (max_cpu_limit, max_memory_limit_mb) = manager_resource_capacity(&state).await;
    Ok(Json(json!({
        "authenticated": true, "installations": installations, "repositories": [], "runnerGroups": [],
        "installUrl": format!("https://github.com/apps/{app_slug}/installations/new"),
        "defaults": runner_pool_defaults(state.config.runner_image(), max_cpu_limit, max_memory_limit_mb)
    })))
}

pub async fn runner_pool_repository_options(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let (_, mut available) = available_repositories(&state, &user, true).await?;
    available.retain(|item| !item.repository.archived);
    available.sort_by(|left, right| {
        left.repository
            .full_name
            .to_lowercase()
            .cmp(&right.repository.full_name.to_lowercase())
    });
    let items = available
        .into_iter()
        .map(|item| {
            json!({
                "id": item.repository.id,
                "installationId": item.installation.id,
                "accountLogin": item.installation.account_login,
                "accountType": item.installation.account_type,
                "fullName": item.repository.full_name,
                "private": item.repository.private,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

pub async fn installation_repositories(
    State(state): State<AppState>,
    Path(installation_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_installation_admin(&state, &user, installation_id).await?;
    let installation = available_installations(&state, &user, true)
        .await?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| ApiError::NotFound("GitHub installation does not exist.".into()))?;
    let mut repositories =
        available_repositories_for_installation(&state, &user, &installation).await?;
    repositories.retain(|repository| !repository.archived);
    repositories.sort_by(|left, right| {
        left.full_name
            .to_lowercase()
            .cmp(&right.full_name.to_lowercase())
    });
    let items = repositories
        .into_iter()
        .map(|repository| {
            json!({
                "id": repository.id,
                "installationId": installation.id,
                "accountLogin": installation.account_login,
                "accountType": installation.account_type,
                "fullName": repository.full_name,
                "private": repository.private,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

pub async fn installation_runner_groups(
    State(state): State<AppState>,
    Path(installation_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_installation_admin(&state, &user, installation_id).await?;
    let installation = sqlx::query(
        "SELECT account_login,account_type FROM installations WHERE id=? AND suspended_at IS NULL",
    )
    .bind(installation_id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("GitHub installation does not exist.".into()))?;
    let account_login = installation.get::<String, _>("account_login");
    let account_type = installation.get::<String, _>("account_type");
    if account_type != "Organization" {
        return Ok(Json(json!({ "items": [] })));
    }

    let token = control_token(&state, &user.id, installation_id).await?;
    let groups = state
        .github
        .runner_groups(&account_login, &token)
        .await
        .map_err(ApiError::Internal)?;
    let items = groups
        .into_iter()
        .map(|group| {
            json!({
                "id": group.id,
                "name": group.name,
                "visibility": group.visibility,
                "isDefault": group.is_default,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

pub async fn create_runner_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<CreateRunnerPool>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    assert_same_origin(&state, &headers)?;
    input.validate().map_err(ApiError::BadRequest)?;
    let (max_cpu_limit, max_memory_limit_mb) = manager_resource_capacity(&state).await;
    if input.cpu_limit > max_cpu_limit as f64 {
        return Err(ApiError::BadRequest(format!(
            "CPU limit cannot exceed host CPU capacity ({max_cpu_limit})."
        )));
    }
    if input.memory_limit_mb > max_memory_limit_mb {
        return Err(ApiError::BadRequest(format!(
            "Memory limit cannot exceed host runner budget ({max_memory_limit_mb} MB)."
        )));
    }
    let repository_ids = input.selected_repository_ids();
    let selected = if input.scope == "repository" {
        selected_available_repositories(&state, &user, &repository_ids).await?
    } else {
        assert_installation_admin(&state, &user, input.installation_id).await?;
        Vec::new()
    };
    for item in &selected {
        upsert_repository(&state, item.installation.id, &item.repository).await?;
    }
    let primary_installation_id = selected
        .first()
        .map_or(input.installation_id, |item| item.installation.id);
    let pool_id = uuid::Uuid::new_v4().to_string();
    let labels = normalized_pool_labels(&input.name, &input.labels)?;
    let runner_group_id = if input.scope == "repository" {
        1
    } else {
        input.runner_group_id
    };
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    let result = sqlx::query(
        r#"INSERT INTO runner_pools (
          id,installation_id,repository_id,name,scope,mode,provider,labels,image,desired_count,min_count,
          max_count,cpu_limit,memory_limit_mb,ephemeral,paused,state,created_by,created_at,updated_at,
          runner_group_id,autoscaling_enabled,queue_scale_factor,idle_timeout_minutes
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,0,'active',?,?,?,?,?,?,?)"#,
    )
    .bind(&pool_id).bind(primary_installation_id).bind(repository_ids.first().copied()).bind(&input.name)
    .bind(&input.scope).bind(&input.mode).bind(&input.provider).bind(serde_json::to_string(&labels).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(&input.image).bind(input.desired_count).bind(input.min_count).bind(input.max_count).bind(input.cpu_limit)
    .bind(input.memory_limit_mb).bind(input.mode == "ephemeral").bind(&user.id).bind(now).bind(now)
    .bind(runner_group_id).bind(input.autoscaling_enabled).bind(input.queue_scale_factor).bind(input.idle_timeout_minutes)
    .execute(&mut *transaction).await;
    if let Err(sqlx::Error::Database(error)) = &result
        && error.is_unique_violation()
    {
        return Err(ApiError::Conflict(
            "A runner pool with this name already exists for the installation.".into(),
        ));
    }
    result?;
    for (position, repository_id) in repository_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES (?,?,?)",
        )
        .bind(&pool_id)
        .bind(repository_id)
        .bind(now.saturating_add(i64::try_from(position).unwrap_or(i64::MAX)))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    audit(
        &state,
        &user,
        "runner_pool.created",
        "runner_pool",
        Some(&pool_id),
        json!({ "name": input.name, "scope": input.scope, "repositoryIds": repository_ids, "desiredCount": input.desired_count }),
    )
    .await?;
    let mut provisioned = Vec::new();
    for _ in 0..input.desired_count {
        match provision(&state, &user, &pool_id, None).await {
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
        "retry" => {
            pool_access(&state, &user, &pool_id).await?;
            assert_pool_admin(&state, &user, &pool_id).await?;
            sqlx::query("UPDATE runner_pools SET provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0,state='active',updated_at=? WHERE id=?")
                .bind(now_millis())
                .bind(&pool_id)
                .execute(&state.database)
                .await?;
            audit(
                &state,
                &user,
                "runner_pool.provisioning_retried",
                "runner_pool",
                Some(&pool_id),
                json!({}),
            )
            .await?;
        }
        "reconcile" => return Ok(Json(reconcile_pool(&state, &user, &pool_id).await?)),
        "scale" => {
            let pool = pool_access(&state, &user, &pool_id).await?;
            assert_pool_admin(&state, &user, &pool_id).await?;
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
    pool_access(&state, &user, &pool_id).await?;
    assert_pool_admin(&state, &user, &pool_id).await?;
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
            provision(&state, &user, &runner.pool_id, runner.target_repository_id).await?;
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
    let endpoint = workflow_action_endpoint(&input.action)?;
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

pub async fn workflow_job_log_view(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        r#"SELECT job.id,job.run_id,job.name,job.status,job.conclusion,job.started_at,
          job.completed_at,run.run_number,run.workflow_name,repo.owner,repo.name AS repository_name,
          repo.full_name,repo.installation_id
          FROM workflow_jobs job
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=run.repository_id
          JOIN user_installations access ON access.installation_id=repo.installation_id
          WHERE job.id=? AND access.user_id=?"#,
    )
    .bind(job_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Workflow job does not exist or is not accessible.".into())
    })?;
    let owner = row.get::<String, _>("owner");
    let repository = row.get::<String, _>("repository_name");
    let installation_id = row.get::<i64, _>("installation_id");
    let token = control_token(&state, &user.id, installation_id).await?;
    let job_endpoint = format!("/repos/{owner}/{repository}/actions/jobs/{job_id}");
    let (remote_job_result, remote_log_result) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(8),
            state.github.get::<GitHubWorkflowJob>(&job_endpoint, &token),
        ),
        tokio::time::timeout(
            Duration::from_secs(8),
            github_job_log_text(&state, &owner, &repository, job_id, &token),
        ),
    );
    let remote_job = match remote_job_result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("GitHub job metadata request timed out")),
    };
    let metadata_warning = remote_job.as_ref().err().map(
        |_| "GitHub job metadata is temporarily unavailable; GridOps is using retained job state.",
    );
    let remote_log = remote_log_result.ok().and_then(Result::ok).flatten();
    let (raw_logs, source, truncated) = if let Some((logs, truncated)) = remote_log {
        (logs, "github", truncated)
    } else if let Some((logs, truncated)) = local_job_log_text(&state, &user, job_id).await? {
        (logs, "runner", truncated)
    } else {
        (String::new(), "pending", false)
    };
    let fallback_step;
    let (steps, status, conclusion, started_at, completed_at) = if let Ok(job) = &remote_job {
        (
            job.steps.as_slice(),
            job.status.as_str(),
            job.conclusion.as_deref(),
            job.started_at.as_deref(),
            job.completed_at.as_deref(),
        )
    } else {
        fallback_step = vec![GitHubWorkflowStep {
            name: row.get::<String, _>("name"),
            status: row.get::<String, _>("status"),
            conclusion: row.try_get::<Option<String>, _>("conclusion")?,
            number: 1,
            started_at: row.try_get::<Option<i64>, _>("started_at")?.map(iso),
            completed_at: row.try_get::<Option<i64>, _>("completed_at")?.map(iso),
        }];
        (
            fallback_step.as_slice(),
            row.get::<&str, _>("status"),
            row.try_get::<Option<&str>, _>("conclusion")?,
            None,
            None,
        )
    };
    let parsed = structure_job_log(&raw_logs, steps, source == "runner");
    let started_at = started_at.map(ToOwned::to_owned).or_else(|| {
        row.try_get::<Option<i64>, _>("started_at")
            .ok()
            .flatten()
            .map(iso)
    });
    let completed_at = completed_at.map(ToOwned::to_owned).or_else(|| {
        row.try_get::<Option<i64>, _>("completed_at")
            .ok()
            .flatten()
            .map(iso)
    });
    Ok(Json(json!({
        "id": job_id,
        "runId": row.get::<i64, _>("run_id"),
        "runNumber": row.get::<i64, _>("run_number"),
        "workflowName": row.get::<String, _>("workflow_name"),
        "repository": row.get::<String, _>("full_name"),
        "name": row.get::<String, _>("name"),
        "status": status,
        "conclusion": conclusion,
        "startedAt": started_at,
        "completedAt": completed_at,
        "source": source,
        "truncated": truncated,
        "metadataWarning": metadata_warning,
        "hiddenDiagnosticLines": parsed.hidden_diagnostic_lines,
        "lineCount": parsed.line_count,
        "annotations": parsed.annotations.iter().map(log_annotation_json).collect::<Vec<_>>(),
        "steps": parsed.steps.iter().map(structured_step_json).collect::<Vec<_>>(),
    })))
}

async fn github_job_log_text(
    state: &AppState,
    owner: &str,
    repository: &str,
    job_id: i64,
    token: &str,
) -> anyhow::Result<Option<(String, bool)>> {
    let response = state
        .http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repository}/actions/jobs/{job_id}/logs"
        ))
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .await?;
    if matches!(response.status().as_u16(), 404 | 409 | 410) {
        return Ok(None);
    }
    let response = response.error_for_status()?;
    let bytes = response.bytes().await?;
    let truncated = bytes.len() > MAX_STRUCTURED_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_STRUCTURED_LOG_BYTES);
    Ok(Some((
        String::from_utf8_lossy(&bytes[start..]).into_owned(),
        truncated,
    )))
}

async fn local_job_log_text(
    state: &AppState,
    user: &AuthUser,
    job_id: i64,
) -> ApiResult<Option<(String, bool)>> {
    let container_id = sqlx::query_scalar::<_, String>(
        r#"SELECT runner.container_id FROM runners runner
          JOIN runner_pools pool ON pool.id=runner.pool_id
          WHERE runner.deleted_at IS NULL AND runner.container_id IS NOT NULL
            AND (runner.current_job_id=? OR runner.last_job_id=?)
            AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=pool.id)
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=pool.id
                AND NOT EXISTS (SELECT 1 FROM user_installations access
                  WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
            )
          ORDER BY CASE WHEN runner.current_job_id=? THEN 0 ELSE 1 END,runner.updated_at DESC
          LIMIT 1"#,
    )
    .bind(job_id)
    .bind(job_id)
    .bind(&user.id)
    .bind(job_id)
    .fetch_optional(&state.database)
    .await?;
    if let Some(container_id) = container_id {
        let logs = manager_text(
            state,
            &format!("v1/runners/{container_id}/logs?tail=100000"),
        )
        .await?;
        return Ok(Some(truncate_log_text(logs)));
    }
    let row = sqlx::query(
        r#"SELECT stream.path FROM log_streams stream
          JOIN user_installations access ON access.installation_id=stream.installation_id
          WHERE stream.job_id=? AND stream.complete=1 AND access.user_id=?
          ORDER BY stream.created_at DESC LIMIT 1"#,
    )
    .bind(job_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let path = safe_log_path(state, row.get::<&str, _>("path"))?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let truncated = bytes.len() > MAX_STRUCTURED_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_STRUCTURED_LOG_BYTES);
    Ok(Some((
        String::from_utf8_lossy(&bytes[start..]).into_owned(),
        truncated,
    )))
}

fn truncate_log_text(logs: String) -> (String, bool) {
    if logs.len() <= MAX_STRUCTURED_LOG_BYTES {
        return (logs, false);
    }
    let mut start = logs.len() - MAX_STRUCTURED_LOG_BYTES;
    while !logs.is_char_boundary(start) {
        start += 1;
    }
    (logs[start..].to_owned(), true)
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
            json!({
                "ok": true,
                "dockerVersion": value.get("dockerVersion"),
                "apiVersion": value.get("apiVersion"),
                "availableCpus": value.get("availableCpus"),
                "totalMemoryMb": value.get("totalMemoryMb"),
                "provisioningPaused": value.get("provisioningPaused"),
                "capacity": value.get("capacity"),
                "disk": value.get("disk"),
            })
        }
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    };
    let users = if user.role == "admin" {
        let admin_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE role='admin'")
                .fetch_one(&state.database)
                .await?;
        sqlx::query("SELECT id,login,name,avatar_url,role,last_login_at FROM users ORDER BY login")
            .fetch_all(&state.database)
            .await?
            .iter()
            .map(|row| {
                let role = row.get::<String, _>("role");
                json!({
                    "id": row.get::<String, _>("id"),
                    "login": row.get::<String, _>("login"),
                    "name": row.try_get::<Option<String>, _>("name").ok().flatten(),
                    "avatarUrl": row.try_get::<Option<String>, _>("avatar_url").ok().flatten(),
                    "role": role,
                    "lastLoginAt": iso(row.get::<i64, _>("last_login_at")),
                    "canDemote": role != "admin" || admin_count > 1,
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let configuration = configuration(&state).await?;
    let github_app = if configuration.github_app_control {
        let slug = state.github_app_slug().await.map_err(ApiError::Internal)?;
        Some(json!({
            "slug": &slug,
            "appUrl": format!("https://github.com/apps/{slug}"),
            "installUrl": format!("https://github.com/apps/{slug}/installations/new"),
        }))
    } else {
        None
    };
    let installation_rows = sqlx::query(
        r#"SELECT installation.id,installation.account_login,installation.account_type,
          installation.account_avatar_url,installation.repository_selection,
          installation.suspended_at,installation.last_synced_at,access.permission,
          (SELECT COUNT(*) FROM runner_pool_installations mapped
            WHERE mapped.installation_id=installation.id) AS pool_count
          FROM user_installations access
          JOIN installations installation ON installation.id=access.installation_id
          WHERE access.user_id=? ORDER BY installation.account_login"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let installations = installation_rows
        .iter()
        .map(|row| {
            let id = row.get::<i64, _>("id");
            let account_login = row.get::<String, _>("account_login");
            let account_type = row.get::<String, _>("account_type");
            json!({
                "id": id,
                "accountLogin": &account_login,
                "accountType": &account_type,
                "accountAvatarUrl": row.try_get::<Option<String>, _>("account_avatar_url").ok().flatten(),
                "repositorySelection": row.get::<String, _>("repository_selection"),
                "permission": row.get::<String, _>("permission"),
                "suspended": row.try_get::<Option<i64>, _>("suspended_at").ok().flatten().is_some(),
                "lastSyncedAt": iso_optional(row.try_get::<Option<i64>, _>("last_synced_at").ok().flatten()),
                "poolCount": row.get::<i64, _>("pool_count"),
                "manageUrl": github_installation_settings_url(&account_type, &account_login, id),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "authenticated": true, "data": {
        "configuration": configuration, "githubApp": github_app, "installations": installations,
        "manager": manager,
        "settings": {
            "logRetentionDays": stored_i64(&stored, "logRetentionDays", 30),
            "logStorageBudgetMb": stored_i64(&stored, "logStorageBudgetMb", 4096),
            "webhookRetentionDays": stored_i64(&stored, "webhookRetentionDays", 90),
            "auditRetentionDays": stored_i64(&stored, "auditRetentionDays", 365),
            "reconcileIntervalSeconds": stored_i64(&stored, "reconcileIntervalSeconds", 30),
            "githubSyncIntervalSeconds": stored_i64(&stored, "githubSyncIntervalSeconds", 60),
            "autoUpdateImages": stored.get("autoUpdateImages").and_then(Value::as_bool).unwrap_or(false),
            "provisioningPaused": stored.get("provisioningPaused").and_then(Value::as_bool).unwrap_or(false),
        }, "user": { "id": user.id, "login": user.login, "role": user.role }, "users": users
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
        || !(100..=1_048_576).contains(&input.log_storage_budget_mb)
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
        ("logStorageBudgetMb", json!(input.log_storage_budget_mb)),
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
        ("provisioningPaused", json!(input.provisioning_paused)),
    ];
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    for (key, value) in values {
        sqlx::query("INSERT INTO settings (key,value,updated_by,updated_at) VALUES (?,?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_by=excluded.updated_by,updated_at=excluded.updated_at")
            .bind(key).bind(value.to_string()).bind(&user.id).bind(now).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    if let Err(error) = manager_json(
        &state,
        Method::PUT,
        "v1/policy",
        Some(json!({ "provisioningPaused": input.provisioning_paused })),
    )
    .await
    {
        tracing::warn!(error = ?error, "saved provisioning policy but manager synchronization is pending");
    }
    audit(&state, &user, "settings.updated", "system", Some("gridops"), json!({
        "logRetentionDays": input.log_retention_days, "logStorageBudgetMb": input.log_storage_budget_mb,
        "webhookRetentionDays": input.webhook_retention_days,
        "auditRetentionDays": input.audit_retention_days, "reconcileIntervalSeconds": input.reconcile_interval_seconds,
        "githubSyncIntervalSeconds": input.github_sync_interval_seconds,
        "autoUpdateImages": input.auto_update_images,
        "provisioningPaused": input.provisioning_paused,
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

async fn provision(
    state: &AppState,
    user: &AuthUser,
    pool_id: &str,
    preferred_repository_id: Option<i64>,
) -> ApiResult<Value> {
    let pool = pool_access(state, user, pool_id).await?;
    assert_pool_admin(state, user, pool_id).await?;
    if pool.paused {
        return Err(ApiError::Conflict("Runner pool is paused.".into()));
    }
    if setting_bool(state, "provisioningPaused", false).await {
        return Err(ApiError::Conflict(
            "Runner provisioning is globally paused.".into(),
        ));
    }
    let runner_id = uuid::Uuid::new_v4().to_string();
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let runner_name = format!("{}-{suffix}", pool.name);
    let target_repository = if pool.scope == "repository" {
        let preferred = if let Some(repository_id) = preferred_repository_id {
            sqlx::query(
                r#"SELECT repo.id,repo.installation_id,repo.owner,repo.name FROM runner_pool_repositories membership
                   JOIN repositories repo ON repo.id=membership.repository_id
                   WHERE membership.pool_id=? AND membership.repository_id=? AND repo.archived=0"#,
            )
            .bind(pool_id)
            .bind(repository_id)
            .fetch_optional(&state.database)
            .await?
            .map(|row| RepositoryCapacity {
                repository_id: row.get("id"),
                installation_id: row.get("installation_id"),
                owner: row.get("owner"),
                name: row.get("name"),
                queued: 0,
                active: 0,
                busy: 0,
            })
        } else {
            None
        };
        Some(
            match preferred {
                Some(repository) => Some(repository),
                None => {
                    next_runner_repository(&state.database, pool_id, pool.queue_scale_factor)
                        .await?
                }
            }
            .ok_or_else(|| {
                ApiError::Conflict("Runner pool has no available repositories.".into())
            })?,
        )
    } else {
        None
    };
    let capacity_lease = reserve_runner_capacity(
        state,
        &runner_id,
        pool_id,
        pool.cpu_limit,
        pool.memory_limit_mb,
    )
    .await?;
    let now = now_millis();
    if let Err(error) = sqlx::query("INSERT INTO runners (id,pool_id,target_repository_id,name,status,ephemeral,configuration_version,created_at,updated_at) VALUES (?,?,?,?,'starting',?,?,?,?)")
        .bind(&runner_id).bind(pool_id)
        .bind(target_repository.as_ref().map(|repository| repository.repository_id))
        .bind(&runner_name).bind(pool.ephemeral)
        .bind(pool.configuration_version).bind(now).bind(now).execute(&state.database).await
    {
        release_runner_capacity(state, &capacity_lease).await;
        return Err(error.into());
    }
    let result = async {
        let target_installation_id = target_repository
            .as_ref()
            .map_or(pool.installation_id, |repository| repository.installation_id);
        let token = control_token(state, &user.id, target_installation_id).await?;
        let labels = json_array(&pool.labels);
        let target = match &target_repository {
            Some(repository) => RunnerTarget::Repository {
                owner: &repository.owner,
                repository: &repository.name,
            },
            None => RunnerTarget::Organization { organization: &pool.account_login },
        };
        let mut request = json!({
            "runnerId": runner_id, "poolId": pool_id, "name": runner_name, "image": pool.image,
            "mode": pool.mode, "provider": pool.provider, "labels": &labels, "cpuLimit": pool.cpu_limit,
            "memoryLimitMb": pool.memory_limit_mb, "network": state.config.runner_network(),
            "capacityLease": &capacity_lease,
            "pullImage": setting_bool(state, "autoUpdateImages", false).await,
        });
        let github_runner_id = if pool.ephemeral {
            let jit = state.github.generate_jit_config(target, &token, &JitRequest {
                name: runner_name.clone(), runner_group_id: pool.runner_group_id,
                labels: effective_runner_labels(&pool.provider, &labels), work_folder: "_work".into(),
            }).await.map_err(ApiError::Internal)?;
            request["jitConfig"] = Value::String(jit.encoded_jit_config);
            Some(jit.runner.id)
        } else {
            let registration = state.github.generate_registration_token(target, &token)
                .await.map_err(ApiError::Internal)?;
            request["registrationToken"] = Value::String(registration.token);
            request["registrationUrl"] = Value::String(runner_registration_url(
                &pool.account_login,
                target_repository.as_ref(),
            ));
            if pool.scope == "organization" && pool.runner_group_id != 1 {
                let group = state.github.runner_group_name(
                    &pool.account_login, pool.runner_group_id, &token,
                ).await.map_err(ApiError::Internal)?;
                request["runnerGroup"] = Value::String(group);
            }
            None
        };
        if let Some(github_runner_id) = github_runner_id {
            sqlx::query("UPDATE runners SET github_runner_id=?,updated_at=? WHERE id=?")
                .bind(github_runner_id)
                .bind(now_millis())
                .bind(&runner_id)
                .execute(&state.database)
                .await?;
        }
        let manager = manager_json(state, Method::POST, "v1/runners", Some(request)).await?;
        let container_id = manager.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::ServiceUnavailable("Runner manager returned an invalid container identifier.".into()))?;
        let status = if manager.get("state").and_then(Value::as_str) == Some("running") { "online" } else { "starting" };
        let updated = now_millis();
        sqlx::query("UPDATE runners SET github_runner_id=?,container_id=?,container_name=?,status=?,registered_at=?,last_heartbeat_at=?,updated_at=? WHERE id=?")
            .bind(github_runner_id).bind(container_id).bind(manager.get("name").and_then(Value::as_str)).bind(status)
            .bind(updated).bind(updated).bind(updated).bind(&runner_id).execute(&state.database).await?;
        sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,event,message,metadata,created_at) VALUES (?,?,?,'Runner started',?,?,?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&runner_id).bind(pool_id).bind(format!("{runner_name} started in pool {}", pool.name))
            .bind(json!({ "containerId": container_id, "githubRunnerId": github_runner_id, "mode": pool.mode, "repositoryId": target_repository.as_ref().map(|repository| repository.repository_id) }).to_string()).bind(updated).execute(&state.database).await?;
        audit(state, user, "runner.provisioned", "runner", Some(&runner_id), json!({ "poolId": pool_id, "containerId": container_id, "githubRunnerId": github_runner_id, "mode": pool.mode, "repositoryId": target_repository.as_ref().map(|repository| repository.repository_id) })).await?;
        Ok::<Value, ApiError>(json!({ "runnerId": runner_id, "status": status }))
    }.await;
    if let Err(error) = &result {
        release_runner_capacity(state, &capacity_lease).await;
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
    pool_access(state, user, pool_id).await?;
    assert_pool_admin(state, user, pool_id).await?;
    sqlx::query("UPDATE runner_pools SET paused=?,state=?,updated_at=? WHERE id=?")
        .bind(paused)
        .bind(if paused { "draining" } else { "active" })
        .bind(now_millis())
        .bind(pool_id)
        .execute(&state.database)
        .await?;
    if !paused {
        sqlx::query("UPDATE runner_pools SET provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0 WHERE id=?")
            .bind(pool_id)
            .execute(&state.database)
            .await?;
    }
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
    assert_pool_admin(state, user, pool_id).await?;
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
            provision(state, user, pool_id, None).await?;
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
    sqlx::query_as::<_, PoolAccess>(
        r#"SELECT p.installation_id,
      CASE WHEN NOT EXISTS (
        SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
          AND NOT EXISTS (SELECT 1 FROM user_installations manage
            WHERE manage.user_id=? AND manage.installation_id=mapped.installation_id
              AND manage.permission='admin')
      ) THEN 'admin' ELSE 'read' END AS installation_permission,
      i.account_login,
      p.repository_id,repo.owner AS repository_owner,repo.name AS repository_name,p.name,p.scope,
      p.mode,p.provider,p.labels,p.image,p.desired_count,p.min_count,p.max_count,
      CAST(p.cpu_limit AS REAL) AS cpu_limit,p.memory_limit_mb,
      p.runner_group_id,p.ephemeral,p.paused,p.state,p.autoscaling_enabled,p.queue_scale_factor,
      p.idle_timeout_minutes,p.configuration_version,p.provision_failure_count,
      p.provision_retry_at,p.provision_circuit_open
      FROM runner_pools p JOIN installations i ON i.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=p.repository_id
      WHERE p.id=?
        AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
        AND NOT EXISTS (
          SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
            AND NOT EXISTS (SELECT 1 FROM user_installations access
              WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
        )"#,
    )
    .bind(&user.id)
    .bind(pool_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("Runner pool does not exist or is not accessible.".into()))
}

async fn runner_access(
    state: &AppState,
    user: &AuthUser,
    runner_id: &str,
) -> ApiResult<RunnerAccess> {
    sqlx::query_as::<_, RunnerAccess>(r#"SELECT r.id AS runner_id,r.name AS runner_name,r.container_id,
      r.github_runner_id,r.status AS runner_status,r.busy,r.ephemeral,r.configuration_version,
      r.last_job_id,p.id AS pool_id,p.name AS pool_name,
      COALESCE(repo.installation_id,p.installation_id) AS installation_id,
      COALESCE(target_installation.account_login,primary_installation.account_login) AS account_login,
      r.target_repository_id,
      repo.owner AS repository_owner,repo.name AS repository_name FROM runners r
      JOIN runner_pools p ON p.id=r.pool_id
      JOIN installations primary_installation ON primary_installation.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=r.target_repository_id
      LEFT JOIN installations target_installation ON target_installation.id=repo.installation_id
      WHERE r.id=? AND r.deleted_at IS NULL
        AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
        AND NOT EXISTS (
          SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
            AND NOT EXISTS (SELECT 1 FROM user_installations access
              WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
        )"#)
        .bind(runner_id).bind(&user.id).fetch_optional(&state.database).await?
        .ok_or_else(|| ApiError::NotFound("Runner does not exist or is not accessible.".into()))
}

async fn runners_for_pool(state: &AppState, pool_id: &str) -> ApiResult<Vec<RunnerAccess>> {
    Ok(sqlx::query_as::<_, RunnerAccess>(r#"SELECT r.id AS runner_id,r.name AS runner_name,r.container_id,
      r.github_runner_id,r.status AS runner_status,r.busy,r.ephemeral,r.configuration_version,
      r.last_job_id,p.id AS pool_id,p.name AS pool_name,
      COALESCE(repo.installation_id,p.installation_id) AS installation_id,
      COALESCE(target_installation.account_login,primary_installation.account_login) AS account_login,
      r.target_repository_id,
      repo.owner AS repository_owner,repo.name AS repository_name FROM runners r
      JOIN runner_pools p ON p.id=r.pool_id
      JOIN installations primary_installation ON primary_installation.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=r.target_repository_id
      LEFT JOIN installations target_installation ON target_installation.id=repo.installation_id
      WHERE p.id=? AND r.deleted_at IS NULL ORDER BY r.created_at DESC"#)
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
        "provider": "docker", "image": image, "tartImage": tart_image,
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
