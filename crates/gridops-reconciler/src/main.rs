use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use gridops_core::{
    Config, GitHubClient, JitRequest, RunnerTarget, Vault, WorkflowJobPage, WorkflowRunPage,
    assigned_queued_jobs, associate_runner_with_job, connect_database, effective_runner_labels,
    next_runner_provider, next_runner_repository, now_millis, provider_capacities,
    provider_capacity_deficit, repository_capacities, repository_capacity_deficit, scale_up_target,
};
use reqwest::{Method, StatusCode};
use secrecy::ExposeSecret as _;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{FromRow, Row as _, SqlitePool};
use tokio::io::AsyncWriteExt as _;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

const MAX_ARCHIVED_LOG_BYTES: i64 = 100 * 1_024 * 1_024;

#[derive(Clone)]
struct Reconciler {
    config: Config,
    database: SqlitePool,
    github: GitHubClient,
    vault: Vault,
    http: reqwest::Client,
}

#[derive(Debug, FromRow)]
struct Pool {
    id: String,
    installation_id: i64,
    account_login: String,
    scope: String,
    name: String,
    state: String,
    mode: String,
    provider: String,
    providers: String,
    labels: String,
    docker_image: String,
    tart_image: String,
    desired_count: i64,
    min_count: i64,
    max_count: i64,
    cpu_limit: f64,
    memory_limit_mb: i64,
    runner_group_id: i64,
    ephemeral: bool,
    paused: bool,
    autoscaling_enabled: bool,
    queue_scale_factor: i64,
    idle_timeout_minutes: i64,
    configuration_version: i64,
    provision_failure_count: i64,
    provision_retry_at: Option<i64>,
    provision_circuit_open: bool,
}

#[derive(Debug, FromRow)]
struct Runner {
    id: String,
    installation_id: i64,
    name: String,
    container_id: Option<String>,
    github_runner_id: Option<i64>,
    target_repository_id: Option<i64>,
    repository_owner: Option<String>,
    repository_name: Option<String>,
    provider: String,
    status: String,
    busy: bool,
    last_job_id: Option<i64>,
    configuration_version: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct Repository {
    id: i64,
    installation_id: i64,
    owner: String,
    name: String,
    full_name: String,
}

#[derive(Debug, FromRow)]
struct PendingGitHubCleanup {
    id: String,
    installation_id: i64,
    target_owner: String,
    target_repository: Option<String>,
    github_runner_id: Option<i64>,
    runner_name: String,
    attempts: i64,
}

#[derive(Debug, Deserialize)]
struct ManagerRunners {
    runners: Vec<ManagedRunner>,
}

#[derive(Debug, Deserialize)]
struct ManagedRunner {
    id: String,
    state: String,
}

#[derive(Debug, Deserialize)]
struct CreatedRunner {
    id: String,
    name: String,
    state: String,
}

enum ProvisionAttempt {
    Provisioned,
    Deferred,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "gridops_reconciler=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    let config = Config::from_env()?;
    let database = connect_database(&config).await?;
    let github = GitHubClient::new(config.clone())?;
    let vault = Vault::from_config(&config)?;
    let reconciler = Reconciler {
        config,
        database,
        github,
        vault,
        http: reqwest::Client::builder()
            .user_agent("GridOps reconciler/0.1")
            .build()?,
    };

    tracing::info!("GridOps Rust reconciler started");
    loop {
        let started = std::time::Instant::now();
        if let Err(error) = reconcile(&reconciler).await {
            tracing::error!(error = ?error, "reconciliation pass failed");
        }
        let interval = setting_i64(&reconciler.database, "reconcileIntervalSeconds", 30)
            .await
            .clamp(5, 3_600);
        let elapsed = started.elapsed();
        tokio::time::sleep(Duration::from_secs(interval as u64).saturating_sub(elapsed)).await;
    }
}

async fn reconcile(app: &Reconciler) -> Result<()> {
    if let Err(error) = maybe_sync_github(app).await {
        tracing::warn!(error = ?error, "periodic GitHub workflow sync failed");
    }
    if let Err(error) = retry_github_runner_cleanup(app).await {
        tracing::warn!(error = ?error, "deferred GitHub runner cleanup pass failed");
    }
    let provisioning_paused = setting_bool(&app.database, "provisioningPaused", false).await;
    if let Err(error) = manager_request::<Value>(
        app,
        Method::PUT,
        "v1/policy",
        Some(json!({ "provisioningPaused": provisioning_paused })),
    )
    .await
    {
        tracing::warn!(error = ?error, "could not synchronize runner provisioning policy");
    }
    let managed = match manager_request::<ManagerRunners>(app, Method::GET, "v1/runners", None)
        .await
    {
        Ok(value) => value.runners,
        Err(error) => {
            tracing::warn!(error = ?error, "runner manager unavailable; skipping runner reconciliation");
            cleanup_retention(app).await?;
            return Ok(());
        }
    };
    let container_states = managed
        .into_iter()
        .map(|runner| (runner.id, runner.state))
        .collect::<HashMap<_, _>>();
    let pools = load_pools(&app.database).await?;
    for pool in pools {
        if let Err(error) = reconcile_pool(app, &pool, &container_states, provisioning_paused).await
        {
            tracing::error!(pool_id = %pool.id, pool = %pool.name, error = ?error, "pool reconciliation failed");
            system_event(
                app,
                Some(&pool.id),
                "error",
                "Pool reconciliation failed",
                &error.to_string(),
                json!({}),
            )
            .await?;
        }
    }
    cleanup_retention(app).await?;
    sqlx::query("DELETE FROM oauth_states WHERE expires_at < ?")
        .bind(now_millis())
        .execute(&app.database)
        .await?;
    sqlx::query("DELETE FROM github_app_manifest_states WHERE expires_at < ?")
        .bind(now_millis())
        .execute(&app.database)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now_millis())
        .execute(&app.database)
        .await?;
    Ok(())
}

async fn load_pools(database: &SqlitePool) -> Result<Vec<Pool>> {
    Ok(sqlx::query_as::<_, Pool>(
        r#"SELECT p.id,p.installation_id,i.account_login,p.scope,p.name,p.state,p.mode,p.provider,p.providers,p.labels,p.docker_image,p.tart_image,p.desired_count,p.min_count,
          p.max_count,CAST(p.cpu_limit AS REAL) AS cpu_limit,p.memory_limit_mb,
          p.runner_group_id,p.ephemeral,p.paused,
          p.autoscaling_enabled,p.queue_scale_factor,p.idle_timeout_minutes,p.configuration_version,
          p.provision_failure_count,p.provision_retry_at,p.provision_circuit_open FROM runner_pools p
          JOIN installations i ON i.id=p.installation_id
          WHERE p.state != 'deleting' ORDER BY p.created_at"#,
    )
    .fetch_all(database)
    .await?)
}

async fn reconcile_pool(
    app: &Reconciler,
    pool: &Pool,
    container_states: &HashMap<String, String>,
    provisioning_paused: bool,
) -> Result<()> {
    let known_runners = runners(app, &pool.id).await?;
    for runner in &known_runners {
        if let Some(container_id) = &runner.container_id {
            let docker_state = container_states
                .get(container_id)
                .map(String::as_str)
                .unwrap_or("missing");
            let status = match docker_state {
                "running" if runner.busy => "busy",
                "running" => "online",
                "paused" => "paused",
                "exited" | "dead" if runner.status == "stopped" => "stopped",
                "exited" | "dead" | "missing" => "failed",
                value => value,
            };
            let heartbeat = now_millis();
            sqlx::query(
                "UPDATE runners SET status=?,last_heartbeat_at=?,updated_at=CASE WHEN status<>? THEN ? ELSE updated_at END WHERE id=?",
            )
                .bind(status)
                .bind(heartbeat)
                .bind(status)
                .bind(heartbeat)
                .bind(&runner.id)
                .execute(&app.database)
                .await?;
        }
    }

    let failed = runners(app, &pool.id)
        .await?
        .into_iter()
        .filter(|runner| runner.status == "failed")
        .collect::<Vec<_>>();
    for runner in &failed {
        delete_runner(app, pool, runner).await?;
    }

    let retry_deferred = pool
        .provision_retry_at
        .is_some_and(|retry_at| retry_at > now_millis());
    let provisioning_blocked = provisioning_paused || pool.provision_circuit_open || retry_deferred;
    let current = runners(app, &pool.id).await?;
    let mut rotated = 0;
    if !provisioning_blocked
        && let Some(stale) = current
            .iter()
            .find(|runner| !runner.busy && runner_needs_update(pool, runner))
    {
        delete_runner(app, pool, stale).await?;
        rotated = 1;
    }

    let current = runners(app, &pool.id).await?;
    if !provisioning_blocked {
        maybe_scale_up(app, pool, &current).await?;
    }
    maybe_scale_down(app, pool, &current).await?;
    let desired = if pool.paused {
        0
    } else {
        sqlx::query("SELECT desired_count FROM runner_pools WHERE id=?")
            .bind(&pool.id)
            .fetch_one(&app.database)
            .await?
            .get::<i64, _>("desired_count")
            .clamp(pool.min_count, pool.max_count)
    };
    let rebalanced = if provisioning_blocked {
        0
    } else {
        rebalance_repository_capacity(app, pool, desired).await?
    };
    let refreshed = runners(app, &pool.id).await?;
    let active = refreshed
        .iter()
        .filter(|runner| active_status(&runner.status))
        .collect::<Vec<_>>();
    let mut provisioned = 0;
    let mut removed = 0;
    let mut capacity_deferred = false;
    if !provisioning_blocked && active.len() < desired as usize {
        for _ in active.len()..desired as usize {
            match provision(app, pool).await? {
                ProvisionAttempt::Provisioned => provisioned += 1,
                ProvisionAttempt::Deferred => {
                    capacity_deferred = true;
                    break;
                }
            }
        }
    } else if active.len() > desired as usize {
        let excess = active.len() - desired as usize;
        for runner in active
            .into_iter()
            .filter(|runner| !runner.busy)
            .take(excess)
        {
            delete_runner(app, pool, runner).await?;
            removed += 1;
        }
    }
    let final_runners = runners(app, &pool.id).await?;
    let active_count = final_runners
        .iter()
        .filter(|runner| active_status(&runner.status))
        .count();
    let outdated_count = final_runners
        .iter()
        .filter(|runner| runner_needs_update(pool, runner))
        .count();
    sqlx::query("UPDATE runner_pools SET state=?,updated_at=? WHERE id=?")
        .bind(if pool.paused {
            "paused"
        } else if provisioning_paused {
            "provisioning-paused"
        } else if pool.provision_circuit_open {
            "blocked"
        } else if capacity_deferred || (retry_deferred && pool.provision_failure_count == 0) {
            "waiting"
        } else if retry_deferred {
            "backoff"
        } else if outdated_count > 0 {
            "updating"
        } else if active_count > desired as usize {
            "draining"
        } else {
            "active"
        })
        .bind(now_millis())
        .bind(&pool.id)
        .execute(&app.database)
        .await?;
    record_capacity_sample(app, pool, &final_runners).await?;
    if provisioned > 0 || removed > 0 || rotated > 0 || rebalanced > 0 {
        system_event(
            app,
            Some(&pool.id),
            "info",
            "Pool reconciled",
            &format!("Provisioned {provisioned}, removed {removed}, rotated {rotated}, rebalanced {rebalanced}, active {active_count}"),
            json!({ "desired": desired, "active": active_count, "provisioned": provisioned, "removed": removed, "rotated": rotated, "rebalanced": rebalanced, "outdated": outdated_count }),
        )
        .await?;
    }
    Ok(())
}

async fn maybe_scale_up(app: &Reconciler, pool: &Pool, runners: &[Runner]) -> Result<()> {
    if !pool.autoscaling_enabled || pool.paused {
        return Ok(());
    }
    let queued = assigned_queued_jobs(&app.database, &pool.id).await?;
    if queued == 0 {
        return Ok(());
    }
    let busy = i64::try_from(
        runners
            .iter()
            .filter(|runner| runner.busy && active_status(&runner.status))
            .count(),
    )
    .unwrap_or(i64::MAX);
    let aggregate_target = scale_up_target(
        pool.desired_count,
        busy,
        queued,
        pool.queue_scale_factor,
        pool.max_count,
    );
    let repository_placement_needed = if pool.scope == "repository" {
        repository_capacities(&app.database, &pool.id)
            .await?
            .iter()
            .map(|capacity| repository_capacity_deficit(capacity, pool.queue_scale_factor).max(0))
            .sum::<i64>()
    } else {
        0
    };
    let providers = serde_json::from_str::<Vec<String>>(&pool.providers).unwrap_or_default();
    let labels = serde_json::from_str::<Vec<String>>(&pool.labels).unwrap_or_default();
    let provider_placement_needed =
        provider_capacities(&app.database, &pool.id, &providers, &labels)
            .await?
            .iter()
            .map(|capacity| provider_capacity_deficit(capacity, pool.queue_scale_factor).max(0))
            .sum::<i64>();
    let active = i64::try_from(
        runners
            .iter()
            .filter(|runner| active_status(&runner.status))
            .count(),
    )
    .unwrap_or(i64::MAX);
    let target = aggregate_target
        .max(active.saturating_add(repository_placement_needed))
        .max(active.saturating_add(provider_placement_needed))
        .min(pool.max_count);
    if target <= pool.desired_count {
        return Ok(());
    }
    let now = now_millis();
    sqlx::query("UPDATE runner_pools SET desired_count=?,state='scaling',updated_at=? WHERE id=?")
        .bind(target)
        .bind(now)
        .bind(&pool.id)
        .execute(&app.database)
        .await?;
    system_event(
        app,
        Some(&pool.id),
        "info",
        "Autoscale requested",
        &format!(
            "{queued} queued jobs raised desired capacity from {} to {target}",
            pool.desired_count
        ),
        json!({ "queuedJobs": queued, "desiredCount": target }),
    )
    .await?;
    system_audit(
        app,
        "runner_pool.autoscaled",
        "runner_pool",
        &pool.id,
        json!({ "queuedJobs": queued, "desiredCount": target, "source": "polling" }),
    )
    .await?;
    Ok(())
}

async fn rebalance_repository_capacity(app: &Reconciler, pool: &Pool, desired: i64) -> Result<i64> {
    if pool.paused {
        return Ok(0);
    }
    let current = runners(app, &pool.id).await?;
    let active = current
        .iter()
        .filter(|runner| active_status(&runner.status))
        .count();
    if active < usize::try_from(desired).unwrap_or(usize::MAX) {
        return Ok(0);
    }
    let capacities = if pool.scope == "repository" {
        repository_capacities(&app.database, &pool.id).await?
    } else {
        Vec::new()
    };
    let repository_needs_capacity = capacities
        .iter()
        .any(|capacity| repository_capacity_deficit(capacity, pool.queue_scale_factor) > 0);
    let providers = serde_json::from_str::<Vec<String>>(&pool.providers).unwrap_or_default();
    let labels = serde_json::from_str::<Vec<String>>(&pool.labels).unwrap_or_default();
    let provider_capacities =
        provider_capacities(&app.database, &pool.id, &providers, &labels).await?;
    let provider_needs_capacity = provider_capacities
        .iter()
        .any(|capacity| provider_capacity_deficit(capacity, pool.queue_scale_factor) > 0);
    if !repository_needs_capacity && !provider_needs_capacity {
        return Ok(0);
    }
    let surplus = capacities
        .iter()
        .filter(|capacity| {
            capacity.active
                > capacity
                    .busy
                    .saturating_add(capacity.queued.saturating_mul(pool.queue_scale_factor))
        })
        .map(|capacity| capacity.repository_id)
        .collect::<HashSet<_>>();
    let provider_surplus = provider_capacities
        .iter()
        .filter(|capacity| {
            capacity.active
                > capacity
                    .busy
                    .saturating_add(capacity.queued.saturating_mul(pool.queue_scale_factor))
        })
        .map(|capacity| capacity.provider.as_str())
        .collect::<HashSet<_>>();
    let membership = capacities
        .iter()
        .map(|capacity| capacity.repository_id)
        .collect::<HashSet<_>>();
    let Some(runner) = current.iter().find(|runner| {
        !runner.busy
            && active_status(&runner.status)
            && (provider_surplus.contains(runner.provider.as_str())
                || !providers.contains(&runner.provider)
                || runner.target_repository_id.is_some_and(|repository_id| {
                    surplus.contains(&repository_id) || !membership.contains(&repository_id)
                }))
    }) else {
        return Ok(0);
    };
    delete_runner(app, pool, runner).await?;
    Ok(1)
}

async fn maybe_scale_down(app: &Reconciler, pool: &Pool, runners: &[Runner]) -> Result<()> {
    if !pool.autoscaling_enabled || pool.paused || pool.desired_count <= pool.min_count {
        return Ok(());
    }
    if runners.iter().any(|runner| runner.busy) {
        return Ok(());
    }
    let queued = assigned_queued_jobs(&app.database, &pool.id).await?;
    if queued > 0 {
        return Ok(());
    }
    let last_activity = runners
        .iter()
        .map(|runner| runner.updated_at)
        .max()
        .unwrap_or(0);
    if !idle_period_elapsed(now_millis(), pool.idle_timeout_minutes, last_activity) {
        return Ok(());
    }
    sqlx::query("UPDATE runner_pools SET desired_count=?,state='scaling',updated_at=? WHERE id=?")
        .bind(pool.min_count)
        .bind(now_millis())
        .bind(&pool.id)
        .execute(&app.database)
        .await?;
    system_audit(
        app,
        "runner_pool.autoscaled_down",
        "runner_pool",
        &pool.id,
        json!({ "desiredCount": pool.min_count }),
    )
    .await?;
    Ok(())
}

async fn retry_github_runner_cleanup(app: &Reconciler) -> Result<()> {
    let now = now_millis();
    let pending = sqlx::query_as::<_, PendingGitHubCleanup>(
        r#"SELECT id,installation_id,target_owner,target_repository,github_runner_id,
          runner_name,attempts FROM github_runner_cleanup
          WHERE next_attempt_at<=? ORDER BY next_attempt_at LIMIT 25"#,
    )
    .bind(now)
    .fetch_all(&app.database)
    .await?;
    for cleanup in pending {
        let result = async {
            let token = installation_token(app, cleanup.installation_id)
                .await?
                .context("GitHub App credentials are unavailable")?;
            let target = match cleanup.target_repository.as_deref() {
                Some(repository) => RunnerTarget::Repository {
                    owner: &cleanup.target_owner,
                    repository,
                },
                None => RunnerTarget::Organization {
                    organization: &cleanup.target_owner,
                },
            };
            let github_runner_id = match cleanup.github_runner_id {
                Some(id) => Some(id),
                None => app
                    .github
                    .runner_by_name(target, &token, &cleanup.runner_name)
                    .await?
                    .map(|runner| runner.id),
            };
            if let Some(github_runner_id) = github_runner_id {
                let path = match cleanup.target_repository.as_deref() {
                    Some(repository) => format!(
                        "/repos/{}/{repository}/actions/runners/{github_runner_id}",
                        cleanup.target_owner
                    ),
                    None => format!(
                        "/orgs/{}/actions/runners/{github_runner_id}",
                        cleanup.target_owner
                    ),
                };
                app.github.delete(&path, &token).await?;
            }
            Result::<()>::Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                sqlx::query("DELETE FROM github_runner_cleanup WHERE id=?")
                    .bind(&cleanup.id)
                    .execute(&app.database)
                    .await?;
                system_audit(
                    app,
                    "runner.github_cleanup_completed",
                    "runner",
                    &cleanup.id,
                    json!({ "attempts": cleanup.attempts + 1 }),
                )
                .await?;
            }
            Err(error) => {
                let message = error.to_string().chars().take(2_000).collect::<String>();
                let attempts = cleanup.attempts.saturating_add(1);
                let next_attempt_at = now.saturating_add(github_cleanup_backoff_ms(attempts));
                sqlx::query(
                    "UPDATE github_runner_cleanup SET attempts=?,last_error=?,next_attempt_at=?,updated_at=? WHERE id=?",
                )
                .bind(attempts)
                .bind(&message)
                .bind(next_attempt_at)
                .bind(now)
                .bind(&cleanup.id)
                .execute(&app.database)
                .await?;
                tracing::warn!(runner_id = %cleanup.id, attempts, error = %message, "GitHub runner cleanup remains deferred");
            }
        }
    }
    Ok(())
}

fn github_cleanup_backoff_ms(attempts: i64) -> i64 {
    let exponent = u32::try_from(attempts.saturating_sub(1).clamp(0, 7)).unwrap_or(7);
    30_000_i64.saturating_mul(1_i64 << exponent).min(3_600_000)
}

async fn record_capacity_sample(app: &Reconciler, pool: &Pool, runners: &[Runner]) -> Result<()> {
    let online = runners
        .iter()
        .filter(|runner| matches!(runner.status.as_str(), "online" | "idle" | "busy"))
        .count();
    let busy = runners.iter().filter(|runner| runner.busy).count();
    let available = online.saturating_sub(busy);
    let queued = assigned_queued_jobs(&app.database, &pool.id).await?;
    let recorded_at = now_millis().div_euclid(60_000) * 60_000;
    sqlx::query(
        r#"INSERT INTO capacity_samples (id,installation_id,pool_id,available,busy,queued,recorded_at)
          VALUES (?,?,?,?,?,?,?) ON CONFLICT(pool_id,recorded_at) DO UPDATE SET
          available=excluded.available,busy=excluded.busy,queued=excluded.queued"#,
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(pool.installation_id)
    .bind(&pool.id)
    .bind(i64::try_from(available).unwrap_or(i64::MAX))
    .bind(i64::try_from(busy).unwrap_or(i64::MAX))
    .bind(queued)
    .bind(recorded_at)
    .execute(&app.database)
    .await?;
    Ok(())
}

async fn maybe_sync_github(app: &Reconciler) -> Result<()> {
    let now = now_millis();
    let interval_seconds = setting_i64(&app.database, "githubSyncIntervalSeconds", 60)
        .await
        .clamp(30, 3_600);
    let last_sync = setting_i64(&app.database, "lastGithubSyncAt", 0).await;
    if now.saturating_sub(last_sync) < interval_seconds * 1_000 {
        return Ok(());
    }
    sqlx::query(
        r#"INSERT INTO settings (key,value,updated_at) VALUES ('lastGithubSyncAt',?,?)
           ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at"#,
    )
    .bind(now.to_string())
    .bind(now)
    .execute(&app.database)
    .await?;
    let repositories = sqlx::query_as::<_, Repository>(
        r#"SELECT repo.id,repo.installation_id,repo.owner,repo.name,repo.full_name
           FROM repositories repo JOIN installations i ON i.id=repo.installation_id
           WHERE repo.archived=0 AND i.suspended_at IS NULL
             AND (EXISTS (SELECT 1 FROM runner_pool_repositories membership
                    WHERE membership.repository_id=repo.id)
               OR EXISTS (SELECT 1 FROM runner_pools p WHERE p.repository_id=repo.id)
               OR EXISTS (SELECT 1 FROM workflow_runs wr WHERE wr.repository_id=repo.id))
           ORDER BY repo.id"#,
    )
    .fetch_all(&app.database)
    .await?;
    let mut tokens = HashMap::<i64, Option<String>>::new();
    for repository in repositories {
        let token = if let Some(token) = tokens.get(&repository.installation_id) {
            token.clone()
        } else {
            let token = installation_token(app, repository.installation_id).await?;
            tokens.insert(repository.installation_id, token.clone());
            token
        };
        let Some(token) = token else {
            continue;
        };
        if let Err(error) = sync_repository_workflows(app, &repository, &token).await {
            tracing::warn!(repository = %repository.full_name, error = ?error, "GitHub workflow polling failed for repository");
        }
    }
    Ok(())
}

async fn sync_repository_workflows(
    app: &Reconciler,
    repository: &Repository,
    token: &str,
) -> Result<()> {
    let response: WorkflowRunPage = app
        .github
        .get(
            &format!(
                "/repos/{}/{}/actions/runs?per_page=50",
                repository.owner, repository.name
            ),
            token,
        )
        .await?;
    let now = now_millis();
    let mut job_run_ids = Vec::new();
    let mut seen_run_ids = HashSet::new();
    for run in response
        .workflow_runs
        .iter()
        .filter(|run| matches!(run.status.as_str(), "queued" | "in_progress"))
        .chain(response.workflow_runs.iter().take(5))
    {
        if seen_run_ids.insert(run.id) {
            job_run_ids.push(run.id);
        }
    }
    let mut transaction = app.database.begin().await?;
    for run in response.workflow_runs {
        let updated_at = parse_github_date(Some(&run.updated_at)).unwrap_or(now);
        sqlx::query(
            r#"INSERT INTO workflow_runs (
              id,repository_id,workflow_id,workflow_name,run_number,run_attempt,event,status,
              conclusion,head_branch,head_sha,actor_login,html_url,started_at,completed_at,
              github_created_at,github_updated_at,created_at,updated_at
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT(id) DO UPDATE SET repository_id=excluded.repository_id,
              workflow_id=excluded.workflow_id,workflow_name=excluded.workflow_name,
              run_number=excluded.run_number,run_attempt=excluded.run_attempt,event=excluded.event,
              status=excluded.status,conclusion=excluded.conclusion,head_branch=excluded.head_branch,
              head_sha=excluded.head_sha,actor_login=excluded.actor_login,html_url=excluded.html_url,
              started_at=excluded.started_at,completed_at=excluded.completed_at,
              github_created_at=excluded.github_created_at,
              github_updated_at=excluded.github_updated_at,updated_at=excluded.updated_at"#,
        )
        .bind(run.id)
        .bind(repository.id)
        .bind(run.workflow_id)
        .bind(
            run.name
                .or(run.display_title)
                .unwrap_or_else(|| "Workflow".into()),
        )
        .bind(run.run_number)
        .bind(run.run_attempt)
        .bind(run.event)
        .bind(&run.status)
        .bind(run.conclusion)
        .bind(run.head_branch)
        .bind(run.head_sha)
        .bind(run.actor.map(|actor| actor.login))
        .bind(run.html_url)
        .bind(parse_github_date(run.run_started_at.as_deref()))
        .bind((run.status == "completed").then_some(updated_at))
        .bind(parse_github_date(Some(&run.created_at)).unwrap_or(now))
        .bind(updated_at)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    for run_id in job_run_ids {
        if let Err(error) = sync_run_jobs(app, repository, run_id, token).await {
            tracing::warn!(repository = %repository.full_name, run_id, error = ?error, "GitHub workflow job polling failed");
        }
    }
    Ok(())
}

async fn sync_run_jobs(
    app: &Reconciler,
    repository: &Repository,
    run_id: i64,
    token: &str,
) -> Result<()> {
    for page in 1..=100 {
        let response: WorkflowJobPage = app
            .github
            .get(
                &format!(
                    "/repos/{}/{}/actions/runs/{run_id}/jobs?filter=latest&per_page=100&page={page}",
                    repository.owner, repository.name
                ),
                token,
            )
            .await?;
        let final_page = response.jobs.len() < 100;
        let now = now_millis();
        let mut transaction = app.database.begin().await?;
        for job in &response.jobs {
            sqlx::query(
                r#"INSERT INTO workflow_jobs (
                  id,run_id,name,status,conclusion,runner_id,runner_name,runner_group_id,
                  runner_group_name,labels,html_url,started_at,completed_at,created_at,updated_at
                ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
                ON CONFLICT(id) DO UPDATE SET run_id=excluded.run_id,name=excluded.name,
                  status=excluded.status,conclusion=excluded.conclusion,runner_id=excluded.runner_id,
                  runner_name=excluded.runner_name,runner_group_id=excluded.runner_group_id,
                  runner_group_name=excluded.runner_group_name,labels=excluded.labels,
                  html_url=excluded.html_url,started_at=excluded.started_at,
                  completed_at=excluded.completed_at,updated_at=excluded.updated_at"#,
            )
            .bind(job.id)
            .bind(job.run_id)
            .bind(&job.name)
            .bind(&job.status)
            .bind(&job.conclusion)
            .bind(job.runner_id)
            .bind(&job.runner_name)
            .bind(job.runner_group_id)
            .bind(&job.runner_group_name)
            .bind(serde_json::to_string(&job.labels)?)
            .bind(&job.html_url)
            .bind(parse_github_date(job.started_at.as_deref()))
            .bind(parse_github_date(job.completed_at.as_deref()))
            .bind(parse_github_date(job.started_at.as_deref()).unwrap_or(now))
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        for job in &response.jobs {
            associate_runner_with_job(
                &app.database,
                job.id,
                &job.status,
                job.runner_id,
                job.runner_name.as_deref(),
                now,
            )
            .await?;
        }
        if final_page {
            break;
        }
    }
    Ok(())
}

fn parse_github_date(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
        .map(|date| date.timestamp_millis())
}

async fn provision(app: &Reconciler, pool: &Pool) -> Result<ProvisionAttempt> {
    let runner_id = uuid::Uuid::new_v4().to_string();
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let runner_name = format!("{}-{suffix}", pool.name);
    let target_repository = if pool.scope == "repository" {
        Some(
            next_runner_repository(&app.database, &pool.id, pool.queue_scale_factor)
                .await?
                .context("runner pool has no available repositories")?,
        )
    } else {
        None
    };
    let mut providers = serde_json::from_str::<Vec<String>>(&pool.providers).unwrap_or_default();
    if providers.is_empty() {
        providers.push(pool.provider.clone());
    }
    let labels = serde_json::from_str::<Vec<String>>(&pool.labels).unwrap_or_default();
    let provider = next_runner_provider(
        &app.database,
        &pool.id,
        target_repository
            .as_ref()
            .map(|repository| repository.repository_id),
        &providers,
        &labels,
        pool.queue_scale_factor,
    )
    .await?
    .context("runner pool has no configured providers")?;
    let image = if provider == "tart" {
        &pool.tart_image
    } else {
        &pool.docker_image
    };
    let target_installation_id = target_repository
        .as_ref()
        .map_or(pool.installation_id, |repository| {
            repository.installation_id
        });
    let token = installation_token(app, target_installation_id)
        .await?
        .context("GitHub App credentials are required for autonomous reconciliation")?;
    let admission = reserve_runner_capacity(
        app,
        &runner_id,
        &pool.id,
        &provider,
        pool.cpu_limit,
        pool.memory_limit_mb,
    )
    .await?;
    let Some(capacity_lease) = admission.lease_id else {
        defer_pool_capacity(app, pool, admission.reason.as_deref()).await?;
        return Ok(ProvisionAttempt::Deferred);
    };
    let now = now_millis();
    let (runner_os, runner_architecture) = if provider == "tart" {
        ("macOS", "ARM64")
    } else {
        ("linux", gridops_core::runner_arch_label())
    };
    if let Err(error) = sqlx::query("INSERT INTO runners (id,pool_id,target_repository_id,name,provider,os,architecture,status,ephemeral,configuration_version,created_at,updated_at) VALUES (?,?,?,?,?,?,?,'starting',?,?,?,?)")
        .bind(&runner_id)
        .bind(&pool.id)
        .bind(target_repository.as_ref().map(|repository| repository.repository_id))
        .bind(&runner_name)
        .bind(&provider)
        .bind(runner_os)
        .bind(runner_architecture)
        .bind(pool.ephemeral)
        .bind(pool.configuration_version)
        .bind(now)
        .bind(now)
        .execute(&app.database)
        .await
    {
        release_runner_capacity(app, &capacity_lease).await;
        return Err(error.into());
    }

    let result = async {
        let target = match &target_repository {
            Some(repository) => RunnerTarget::Repository {
                owner: &repository.owner,
                repository: &repository.name,
            },
            None => RunnerTarget::Organization {
                organization: &pool.account_login,
            },
        };
        let mut request = json!({
            "runnerId": runner_id,
            "poolId": pool.id,
            "name": runner_name,
            "image": image,
            "mode": pool.mode,
            "provider": provider,
            "labels": &labels,
            "cpuLimit": pool.cpu_limit,
            "memoryLimitMb": pool.memory_limit_mb,
            "network": app.config.runner_network(),
            "capacityLease": &capacity_lease,
            "pullImage": setting_bool(&app.database, "autoUpdateImages", false).await,
        });
        let github_runner_id = if pool.ephemeral {
            let jit = app
                .github
                .generate_jit_config(
                    target,
                    &token,
                    &JitRequest {
                        name: runner_name.clone(),
                        runner_group_id: pool.runner_group_id,
                        labels: effective_runner_labels(&provider, &labels),
                        work_folder: "_work".into(),
                    },
                )
                .await?;
            request["jitConfig"] = Value::String(jit.encoded_jit_config);
            Some(jit.runner.id)
        } else {
            let registration = app
                .github
                .generate_registration_token(target, &token)
                .await?;
            request["registrationToken"] = Value::String(registration.token);
            request["registrationUrl"] = Value::String(runner_registration_url(
                &pool.account_login,
                target_repository.as_ref(),
            ));
            if pool.scope == "organization" && pool.runner_group_id != 1 {
                let group = app
                    .github
                    .runner_group_name(&pool.account_login, pool.runner_group_id, &token)
                    .await?;
                request["runnerGroup"] = Value::String(group);
            }
            None
        };
        if let Some(github_runner_id) = github_runner_id {
            sqlx::query("UPDATE runners SET github_runner_id=?,updated_at=? WHERE id=?")
                .bind(github_runner_id)
                .bind(now_millis())
                .bind(&runner_id)
                .execute(&app.database)
                .await?;
        }
        let created = manager_request::<CreatedRunner>(
            app,
            Method::POST,
            "v1/runners",
            Some(request),
        )
        .await?;
        let updated = now_millis();
        sqlx::query("UPDATE runners SET github_runner_id=?,container_id=?,container_name=?,status=?,registered_at=?,last_heartbeat_at=?,updated_at=? WHERE id=?")
            .bind(github_runner_id)
            .bind(&created.id)
            .bind(&created.name)
            .bind(if created.state == "running" { "online" } else { "starting" })
            .bind(updated)
            .bind(updated)
            .bind(updated)
            .bind(&runner_id)
            .execute(&app.database)
            .await?;
        Result::<()>::Ok(())
    }
        .await;
    if let Err(error) = result {
        release_runner_capacity(app, &capacity_lease).await;
        let message = error.to_string().chars().take(2_000).collect::<String>();
        sqlx::query("UPDATE runners SET status='failed',failure_reason=?,updated_at=? WHERE id=?")
            .bind(&message)
            .bind(now_millis())
            .bind(&runner_id)
            .execute(&app.database)
            .await?;
        system_event(
            app,
            Some(&pool.id),
            "error",
            "Runner provisioning failed",
            &message,
            json!({ "runnerId": runner_id }),
        )
        .await?;
        if deferred_manager_error(&error) {
            defer_pool_capacity(app, pool, Some(&message)).await?;
            return Ok(ProvisionAttempt::Deferred);
        }
        record_provision_failure(app, pool).await?;
        return Err(error);
    }
    sqlx::query(
        "UPDATE runner_pools SET provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0 WHERE id=?",
    )
    .bind(&pool.id)
    .execute(&app.database)
    .await?;
    system_audit(
        app,
        "runner.provisioned",
        "runner",
        &runner_id,
        json!({ "poolId": pool.id, "provider": provider, "repositoryId": target_repository.as_ref().map(|repository| repository.repository_id) }),
    )
    .await?;
    Ok(ProvisionAttempt::Provisioned)
}

async fn delete_runner(app: &Reconciler, pool: &Pool, runner: &Runner) -> Result<()> {
    if runner.container_id.is_some()
        && let Err(error) = archive_runner_logs(app, pool, runner).await
    {
        tracing::warn!(runner_id = %runner.id, error = ?error, "could not archive runner logs");
    }
    let target = match (&runner.repository_owner, &runner.repository_name) {
        (Some(owner), Some(repository)) => RunnerTarget::Repository { owner, repository },
        _ => RunnerTarget::Organization {
            organization: &pool.account_login,
        },
    };
    match installation_token(app, runner.installation_id).await {
        Ok(Some(token)) => {
            let github_cleanup = async {
                let github_runner_id = match runner.github_runner_id {
                    Some(id) => Some(id),
                    None => app
                        .github
                        .runner_by_name(target, &token, &runner.name)
                        .await?
                        .map(|runner| runner.id),
                };
                if let Some(github_runner_id) = github_runner_id {
                    let path = match (&runner.repository_owner, &runner.repository_name) {
                        (Some(owner), Some(repository)) => format!(
                            "/repos/{owner}/{repository}/actions/runners/{github_runner_id}"
                        ),
                        _ => format!(
                            "/orgs/{}/actions/runners/{github_runner_id}",
                            pool.account_login
                        ),
                    };
                    app.github.delete(&path, &token).await?;
                }
                Result::<()>::Ok(())
            }
            .await;
            if let Err(error) = github_cleanup {
                tracing::warn!(runner_id = %runner.id, error = ?error, "could not remove GitHub runner registration");
            }
        }
        Ok(None) => {
            tracing::warn!(runner_id = %runner.id, "GitHub App credentials unavailable during runner cleanup");
        }
        Err(error) => {
            tracing::warn!(runner_id = %runner.id, error = ?error, "GitHub installation unavailable during runner cleanup");
        }
    }
    if let Some(container_id) = &runner.container_id {
        remove_manager_container(app, container_id).await?;
    }
    let now = now_millis();
    sqlx::query("UPDATE runners SET status='deleted',busy=0,deleted_at=?,updated_at=? WHERE id=?")
        .bind(now)
        .bind(now)
        .bind(&runner.id)
        .execute(&app.database)
        .await?;
    system_audit(
        app,
        "runner.deleted",
        "runner",
        &runner.id,
        json!({ "poolId": pool.id }),
    )
    .await?;
    Ok(())
}

async fn remove_manager_container(app: &Reconciler, container_id: &str) -> Result<()> {
    let token = app
        .config
        .manager_token()
        .context("GRIDOPS_MANAGER_TOKEN is required")?
        .expose_secret();
    let url = app
        .config
        .manager_url()
        .join(&format!("v1/runners/{container_id}"))?;
    let response = app.http.delete(url).bearer_auth(token).send().await?;
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_FOUND {
        return Ok(());
    }
    let detail = response.text().await.unwrap_or_default();
    bail!(
        "runner manager deletion failed ({status}): {}",
        detail.chars().take(500).collect::<String>()
    )
}

async fn runners(app: &Reconciler, pool_id: &str) -> Result<Vec<Runner>> {
    Ok(sqlx::query_as::<_, Runner>(
        r#"SELECT runner.id,COALESCE(repo.installation_id,pool.installation_id) AS installation_id,
           runner.name,runner.container_id,runner.github_runner_id,
           runner.target_repository_id,repo.owner AS repository_owner,repo.name AS repository_name,
           runner.provider,runner.status,runner.busy,runner.last_job_id,runner.configuration_version,runner.updated_at
           FROM runners runner JOIN runner_pools pool ON pool.id=runner.pool_id
           LEFT JOIN repositories repo ON repo.id=runner.target_repository_id
           WHERE runner.pool_id=? AND runner.deleted_at IS NULL ORDER BY runner.created_at DESC"#,
    )
    .bind(pool_id)
    .fetch_all(&app.database)
    .await?)
}

async fn cleanup_retention(app: &Reconciler) -> Result<()> {
    let now = now_millis();
    let webhook_cutoff =
        now - setting_i64(&app.database, "webhookRetentionDays", 90).await * 86_400_000;
    let audit_cutoff =
        now - setting_i64(&app.database, "auditRetentionDays", 365).await * 86_400_000;
    let capacity_cutoff = now - 31 * 86_400_000;
    let expired_logs =
        sqlx::query("SELECT path FROM log_streams WHERE expires_at IS NOT NULL AND expires_at < ?")
            .bind(now)
            .fetch_all(&app.database)
            .await?;
    for row in expired_logs {
        let filename = row.get::<String, _>("path");
        if let Some(path) = safe_log_path(app, &filename) {
            match tokio::fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(filename, error = ?error, "could not remove expired log file");
                }
            }
        }
    }
    sqlx::query("DELETE FROM log_streams WHERE expires_at IS NOT NULL AND expires_at < ?")
        .bind(now)
        .execute(&app.database)
        .await?;
    enforce_log_storage_budget(app).await?;
    sqlx::query("DELETE FROM webhook_deliveries WHERE received_at < ?")
        .bind(webhook_cutoff)
        .execute(&app.database)
        .await?;
    sqlx::query("DELETE FROM audit_events WHERE created_at < ?")
        .bind(audit_cutoff)
        .execute(&app.database)
        .await?;
    sqlx::query("DELETE FROM capacity_samples WHERE recorded_at < ?")
        .bind(capacity_cutoff)
        .execute(&app.database)
        .await?;
    Ok(())
}

async fn enforce_log_storage_budget(app: &Reconciler) -> Result<()> {
    let budget_bytes = setting_i64(&app.database, "logStorageBudgetMb", 4_096)
        .await
        .clamp(100, 1_048_576)
        .saturating_mul(1_024 * 1_024);
    let mut retained_bytes = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size_bytes),0) FROM log_streams WHERE complete=1",
    )
    .fetch_one(&app.database)
    .await?;
    if retained_bytes <= budget_bytes {
        return Ok(());
    }
    let logs = sqlx::query(
        "SELECT id,path,size_bytes FROM log_streams WHERE complete=1 ORDER BY created_at,id",
    )
    .fetch_all(&app.database)
    .await?;
    for row in logs {
        if retained_bytes <= budget_bytes {
            break;
        }
        let id = row.get::<String, _>("id");
        let filename = row.get::<String, _>("path");
        let size_bytes = row.get::<i64, _>("size_bytes").max(0);
        let Some(path) = safe_log_path(app, &filename) else {
            tracing::warn!(
                id,
                filename,
                "refusing to remove an invalid retained-log path"
            );
            continue;
        };
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(id, filename, error = ?error, "could not remove retained log while enforcing storage budget");
                continue;
            }
        }
        sqlx::query("DELETE FROM log_streams WHERE id=?")
            .bind(&id)
            .execute(&app.database)
            .await?;
        retained_bytes = retained_bytes.saturating_sub(size_bytes);
    }
    Ok(())
}

async fn archive_runner_logs(app: &Reconciler, pool: &Pool, runner: &Runner) -> Result<()> {
    let Some(container_id) = runner.container_id.as_deref() else {
        return Ok(());
    };
    if sqlx::query_scalar::<_, String>(
        "SELECT id FROM log_streams WHERE runner_id=? AND source=? AND complete=1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&runner.id)
    .bind(&runner.provider)
    .fetch_optional(&app.database)
    .await?
    .is_some()
    {
        return Ok(());
    }
    let token = app
        .config
        .manager_token()
        .context("GRIDOPS_MANAGER_TOKEN is required")?
        .expose_secret();
    let url = app
        .config
        .manager_url()
        .join(&format!("v1/runners/{container_id}/logs?tail=100000"))?;
    let response = app.http.get(url).bearer_auth(token).send().await?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        bail!(
            "runner log download failed ({status}): {}",
            detail.chars().take(300).collect::<String>()
        );
    }

    tokio::fs::create_dir_all(app.config.log_directory()).await?;
    let stream_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{stream_id}.log");
    let path = safe_log_path(app, &filename).context("generated log path is invalid")?;
    let write_result = async {
        let mut file = tokio::fs::File::create(&path).await?;
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
            file.write_all(retained).await?;
        }
        file.flush().await?;
        Result::<(i64, String)>::Ok((size_bytes, hex::encode(checksum.finalize())))
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
    let retention_days = setting_i64(&app.database, "logRetentionDays", 30).await;
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
    .bind(&runner.id)
    .bind(runner.last_job_id)
    .bind(runner.installation_id)
    .bind(&runner.name)
    .bind(&pool.name)
    .bind(repository)
    .bind(&runner.provider)
    .bind(&filename)
    .bind(size_bytes)
    .bind(checksum)
    .bind(now + retention_days * 86_400_000)
    .bind(now)
    .bind(now)
    .execute(&app.database)
    .await;
    if let Err(error) = inserted {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    Ok(())
}

fn safe_log_path(app: &Reconciler, filename: &str) -> Option<std::path::PathBuf> {
    let candidate = std::path::Path::new(filename);
    if candidate.is_absolute()
        || candidate.components().count() != 1
        || candidate.file_name().and_then(|value| value.to_str()) != Some(filename)
        || filename.len() > 128
        || !filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    Some(app.config.log_directory().join(filename))
}

async fn manager_request<T: serde::de::DeserializeOwned>(
    app: &Reconciler,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T> {
    let token = app
        .config
        .manager_token()
        .context("GRIDOPS_MANAGER_TOKEN is required")?
        .expose_secret();
    let url = app
        .config
        .manager_url()
        .join(path.trim_start_matches('/'))?;
    let mut request = app.http.request(method, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let payload = serde_json::from_str::<Value>(&text).ok();
        let code = payload
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("manager_error");
        let message = payload
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or(&text);
        if status == StatusCode::NOT_FOUND {
            bail!("runner manager object was not found");
        }
        bail!(
            "runner manager request failed ({status}) [{code}]: {}",
            message.chars().take(500).collect::<String>()
        );
    }
    Ok(serde_json::from_str(&text)?)
}

struct CapacityAdmission {
    lease_id: Option<String>,
    reason: Option<String>,
}

async fn reserve_runner_capacity(
    app: &Reconciler,
    runner_id: &str,
    pool_id: &str,
    provider: &str,
    cpu_limit: f64,
    memory_limit_mb: i64,
) -> Result<CapacityAdmission> {
    let response = manager_request::<Value>(
        app,
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
    .await;
    match response {
        Ok(value) => Ok(CapacityAdmission {
            lease_id: value
                .get("leaseId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            reason: None,
        }),
        Err(error) if deferred_manager_error(&error) => Ok(CapacityAdmission {
            lease_id: None,
            reason: Some(error.to_string()),
        }),
        Err(error) => Err(error),
    }
}

async fn release_runner_capacity(app: &Reconciler, lease_id: &str) {
    if let Err(error) = manager_request::<Value>(
        app,
        Method::DELETE,
        &format!("v1/admissions/{lease_id}"),
        None,
    )
    .await
    {
        tracing::warn!(lease_id, error = ?error, "could not release runner capacity reservation");
    }
}

fn deferred_manager_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    [
        "[host_capacity_exhausted]",
        "[host_disk_guardrail]",
        "[provisioning_paused]",
    ]
    .iter()
    .any(|code| message.contains(code))
}

async fn defer_pool_capacity(app: &Reconciler, pool: &Pool, reason: Option<&str>) -> Result<()> {
    let retry_at = now_millis().saturating_add(30_000);
    sqlx::query(
        "UPDATE runner_pools SET state='waiting',provision_retry_at=?,updated_at=? WHERE id=?",
    )
    .bind(retry_at)
    .bind(now_millis())
    .bind(&pool.id)
    .execute(&app.database)
    .await?;
    if pool.state != "waiting" {
        let mut metadata = json!({ "retryAt": retry_at });
        if let Some(reason) = reason {
            metadata["reason"] = Value::String(reason.chars().take(2_000).collect());
        }
        system_event(
            app,
            Some(&pool.id),
            "warning",
            "Runner provisioning waiting",
            "The runner host has no safe capacity available; GridOps will retry without exceeding host limits.",
            metadata,
        )
        .await?;
    }
    Ok(())
}

async fn record_provision_failure(app: &Reconciler, pool: &Pool) -> Result<()> {
    let attempts = pool.provision_failure_count.saturating_add(1);
    let circuit_open = attempts >= 3;
    let retry_at =
        (!circuit_open).then(|| now_millis().saturating_add(provision_backoff_ms(attempts)));
    sqlx::query(
        "UPDATE runner_pools SET provision_failure_count=?,provision_retry_at=?,provision_circuit_open=?,state=?,updated_at=? WHERE id=?",
    )
    .bind(attempts)
    .bind(retry_at)
    .bind(circuit_open)
    .bind(if circuit_open { "blocked" } else { "backoff" })
    .bind(now_millis())
    .bind(&pool.id)
    .execute(&app.database)
    .await?;
    if circuit_open {
        system_event(
            app,
            Some(&pool.id),
            "error",
            "Provisioning circuit opened",
            "GridOps stopped provisioning this pool after three consecutive failures. Retry the pool after correcting the cause.",
            json!({ "attempts": attempts }),
        )
        .await?;
    }
    Ok(())
}

fn provision_backoff_ms(attempts: i64) -> i64 {
    let exponent = u32::try_from(attempts.saturating_sub(1).clamp(0, 10)).unwrap_or(10);
    30_000_i64.saturating_mul(2_i64.saturating_pow(exponent))
}

async fn setting_i64(database: &SqlitePool, key: &str, fallback: i64) -> i64 {
    sqlx::query("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(database)
        .await
        .ok()
        .flatten()
        .and_then(|row| serde_json::from_str::<i64>(row.get::<&str, _>("value")).ok())
        .unwrap_or(fallback)
}

async fn setting_bool(database: &SqlitePool, key: &str, fallback: bool) -> bool {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key=?")
        .bind(key)
        .fetch_optional(database)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<bool>(&value).ok())
        .unwrap_or(fallback)
}

fn runner_registration_url(
    account_login: &str,
    repository: Option<&gridops_core::RepositoryCapacity>,
) -> String {
    match repository {
        Some(repository) => format!(
            "https://github.com/{}/{}",
            repository.owner, repository.name
        ),
        None => format!("https://github.com/{account_login}"),
    }
}

async fn installation_token(app: &Reconciler, installation_id: i64) -> Result<Option<String>> {
    let app_id = match runtime_secret(app, "github.app_id").await? {
        Some(value) => Some(value),
        None => app.config.github_app_id().map(ToOwned::to_owned),
    };
    let private_key = match runtime_secret(app, "github.app_private_key").await? {
        Some(value) => Some(value),
        None => app
            .config
            .github_app_private_key()
            .map(|value| value.expose_secret().to_owned()),
    };
    let Some((app_id, private_key)) = app_id.zip(private_key) else {
        return Ok(None);
    };
    app.github
        .installation_token_with_credentials(installation_id, &app_id, &private_key)
        .await
        .map(Some)
}

async fn runtime_secret(app: &Reconciler, key: &str) -> Result<Option<String>> {
    let sealed = sqlx::query_scalar::<_, String>("SELECT value FROM runtime_secrets WHERE key=?")
        .bind(key)
        .fetch_optional(&app.database)
        .await?;
    sealed
        .map(|value| {
            app.vault
                .open(&value)
                .context("could not decrypt runtime secret")
        })
        .transpose()
}

async fn system_event(
    app: &Reconciler,
    pool_id: Option<&str>,
    level: &str,
    event: &str,
    message: &str,
    metadata: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO runner_events (id,pool_id,level,event,message,metadata,created_at) VALUES (?,?,?,?,?,?,?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(pool_id)
        .bind(level)
        .bind(event)
        .bind(message)
        .bind(metadata.to_string())
        .bind(now_millis())
        .execute(&app.database)
        .await?;
    Ok(())
}

async fn system_audit(
    app: &Reconciler,
    action: &str,
    target_type: &str,
    target_id: &str,
    metadata: Value,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_events (id,actor_label,action,target_type,target_id,metadata,created_at) VALUES (?,'system',?,?,?,?,?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(metadata.to_string())
        .bind(now_millis())
        .execute(&app.database)
        .await?;
    Ok(())
}

fn active_status(status: &str) -> bool {
    matches!(
        status,
        "starting" | "online" | "idle" | "busy" | "paused" | "stopped"
    )
}

fn runner_needs_update(pool: &Pool, runner: &Runner) -> bool {
    runner.configuration_version < pool.configuration_version
}

fn idle_period_elapsed(now: i64, idle_timeout_minutes: i64, last_activity: i64) -> bool {
    now.saturating_sub(last_activity) >= idle_timeout_minutes.saturating_mul(60_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridops_core::connect_database_path;
    use std::fs;

    fn pool(configuration_version: i64) -> Pool {
        Pool {
            id: "pool-1".into(),
            installation_id: 1,
            account_login: "octo-org".into(),
            scope: "organization".into(),
            name: "linux".into(),
            state: "active".into(),
            mode: "ephemeral".into(),
            provider: "docker".into(),
            providers: r#"["docker"]"#.into(),
            labels: "[]".into(),
            docker_image: "runner:latest".into(),
            tart_image: "gridops-macos-tahoe-base".into(),
            desired_count: 1,
            min_count: 0,
            max_count: 10,
            cpu_limit: 2.0,
            memory_limit_mb: 4096,
            runner_group_id: 1,
            ephemeral: true,
            paused: false,
            autoscaling_enabled: true,
            queue_scale_factor: 1,
            idle_timeout_minutes: 5,
            configuration_version,
            provision_failure_count: 0,
            provision_retry_at: None,
            provision_circuit_open: false,
        }
    }

    fn runner(configuration_version: i64) -> Runner {
        Runner {
            id: "runner-1".into(),
            installation_id: 1,
            name: "linux-1234".into(),
            container_id: None,
            github_runner_id: None,
            target_repository_id: None,
            repository_owner: None,
            repository_name: None,
            provider: "docker".into(),
            status: "online".into(),
            busy: false,
            last_job_id: None,
            configuration_version,
            updated_at: 1_000,
        }
    }

    #[tokio::test]
    async fn loads_whole_cpu_limits_stored_as_sqlite_integers() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("gridops-cpu-limit-test-{}", uuid::Uuid::new_v4()));
        let database = connect_database_path(&directory.join("gridops.sqlite")).await?;
        sqlx::raw_sql(
            r#"
            INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
              VALUES (1,1,'octo-org','Organization','Organization','all',1,1);
            INSERT INTO runner_pools (id,installation_id,name,scope,image,created_at,updated_at)
              VALUES ('pool-1',1,'linux','organization','runner:latest',1,1);
            "#,
        )
        .execute(&database)
        .await?;

        let storage_class =
            sqlx::query_scalar::<_, String>("SELECT typeof(cpu_limit) FROM runner_pools")
                .fetch_one(&database)
                .await?;
        assert_eq!(storage_class, "integer");
        let pools = load_pools(&database).await?;
        assert_eq!(pools.len(), 1);
        assert!((pools[0].cpu_limit - 2.0).abs() < f64::EPSILON);

        database.close().await;
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn detects_outdated_runner_generations() {
        assert!(runner_needs_update(&pool(2), &runner(1)));
        assert!(!runner_needs_update(&pool(2), &runner(2)));
        assert!(!runner_needs_update(&pool(2), &runner(3)));
    }

    #[test]
    fn applies_idle_timeout_to_activity_not_heartbeats() {
        let five_minutes = 5 * 60_000;
        assert!(!idle_period_elapsed(1_000 + five_minutes - 1, 5, 1_000));
        assert!(idle_period_elapsed(1_000 + five_minutes, 5, 1_000));
    }

    #[test]
    fn deferred_github_cleanup_uses_bounded_backoff() {
        assert_eq!(github_cleanup_backoff_ms(1), 30_000);
        assert_eq!(github_cleanup_backoff_ms(2), 60_000);
        assert_eq!(github_cleanup_backoff_ms(8), 3_600_000);
        assert_eq!(github_cleanup_backoff_ms(i64::MAX), 3_600_000);
    }

    #[test]
    fn provisioning_failures_back_off_before_opening_the_circuit() {
        assert_eq!(provision_backoff_ms(1), 30_000);
        assert_eq!(provision_backoff_ms(2), 60_000);
        assert_eq!(provision_backoff_ms(3), 120_000);
    }

    #[tokio::test]
    async fn queued_jobs_are_assigned_to_one_label_compatible_pool() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("gridops-reconciler-test-{}", uuid::Uuid::new_v4()));
        let database = connect_database_path(&directory.join("gridops.sqlite")).await?;
        sqlx::raw_sql(
            r#"
            INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
              VALUES
              (1,1,'octo-org','Organization','Organization','all',1,1),
              (2,2,'other-org','Organization','Organization','selected',1,1);
            INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
              VALUES
              (10,1,'octo-org','gridops','octo-org/gridops',0,'master','https://github.com/octo-org/gridops',1,1,1),
              (11,2,'other-org','other','other-org/other',0,'master','https://github.com/other-org/other',1,1,1);
            INSERT INTO runner_pools (id,installation_id,repository_id,name,scope,labels,image,autoscaling_enabled,created_at,updated_at)
              VALUES
              ('pool-a',1,10,'pool-a','repository','["docker","pool-a"]','runner:latest',1,1,1),
              ('pool-b',1,10,'pool-b','repository','["gpu","pool-b"]','runner:latest',1,2,2),
              ('pool-org',1,NULL,'pool-org','organization','["org-only","pool-org"]','runner:latest',1,0,0);
            INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at)
              VALUES ('pool-a',10,1),('pool-a',11,2),('pool-b',10,1);
            INSERT INTO workflow_runs (id,repository_id,workflow_name,run_number,event,status,head_sha,html_url,github_created_at,github_updated_at,created_at,updated_at)
              VALUES
              (20,10,'CI',1,'push','queued','abc123','https://github.com/octo-org/gridops/actions/runs/20',1,1,1,1),
              (21,11,'CI',2,'push','queued','def456','https://github.com/octo-org/other/actions/runs/21',1,1,1,1);
            INSERT INTO workflow_jobs (id,run_id,name,status,labels,html_url,created_at,updated_at)
              VALUES
              (30,20,'generic','queued','["self-hosted","linux"]','https://github.com/octo-org/gridops/actions/runs/20/job/30',1,1),
              (31,20,'docker','queued','["self-hosted","docker"]','https://github.com/octo-org/gridops/actions/runs/20/job/31',1,1),
              (32,20,'gpu','queued','["self-hosted","pool-b"]','https://github.com/octo-org/gridops/actions/runs/20/job/32',1,1),
              (33,20,'org','queued','["self-hosted","org-only"]','https://github.com/octo-org/gridops/actions/runs/20/job/33',1,1),
              (34,20,'unmatched','queued','["self-hosted","unmatched"]','https://github.com/octo-org/gridops/actions/runs/20/job/34',1,1),
              (35,20,'wrong-os','queued','["self-hosted","windows"]','https://github.com/octo-org/gridops/actions/runs/20/job/35',1,1);
            INSERT INTO workflow_jobs (id,run_id,name,status,labels,html_url,created_at,updated_at)
              VALUES (36,21,'other','queued','["self-hosted","pool-a"]','https://github.com/octo-org/other/actions/runs/21/job/36',1,1);
            INSERT INTO runners (id,pool_id,target_repository_id,name,status,created_at,updated_at)
              VALUES
              ('runner-a','pool-a',10,'pool-a-one','online',1,1),
              ('runner-b','pool-a',10,'pool-a-two','online',1,1);
            "#,
        )
        .execute(&database)
        .await?;

        let mut pool_a = pool(1);
        pool_a.id = "pool-a".into();
        let mut pool_b = pool(1);
        pool_b.id = "pool-b".into();
        let mut pool_org = pool(1);
        pool_org.id = "pool-org".into();

        assert_eq!(assigned_queued_jobs(&database, &pool_a.id).await?, 3);
        assert_eq!(assigned_queued_jobs(&database, &pool_b.id).await?, 1);
        assert_eq!(assigned_queued_jobs(&database, &pool_org.id).await?, 1);
        let target = next_runner_repository(&database, &pool_a.id, 1)
            .await?
            .context("multi-repository pool should have a target")?;
        assert_eq!(target.repository_id, 11);
        assert_eq!(target.installation_id, 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM runner_pool_installations WHERE pool_id='pool-a'",
            )
            .fetch_one(&database)
            .await?,
            2
        );

        database.close().await;
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
