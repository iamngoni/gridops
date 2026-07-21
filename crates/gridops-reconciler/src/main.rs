use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result, bail};
use futures_util::StreamExt as _;
use gridops_core::{
    Config, GitHubClient, JitRequest, RunnerTarget, Vault, connect_database, now_millis,
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
    repository_owner: Option<String>,
    repository_name: Option<String>,
    name: String,
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
    autoscaling_enabled: bool,
    idle_timeout_minutes: i64,
    configuration_version: i64,
}

#[derive(Debug, FromRow)]
struct Runner {
    id: String,
    name: String,
    container_id: Option<String>,
    github_runner_id: Option<i64>,
    status: String,
    busy: bool,
    last_job_id: Option<i64>,
    configuration_version: i64,
    updated_at: i64,
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
    let pools = sqlx::query_as::<_, Pool>(
        r#"SELECT p.id,p.installation_id,i.account_login,repo.owner AS repository_owner,
          repo.name AS repository_name,p.name,p.mode,p.labels,p.image,p.desired_count,p.min_count,
          p.max_count,p.cpu_limit,p.memory_limit_mb,p.runner_group_id,p.ephemeral,p.paused,
          p.autoscaling_enabled,p.idle_timeout_minutes,p.configuration_version FROM runner_pools p
          JOIN installations i ON i.id=p.installation_id
          LEFT JOIN repositories repo ON repo.id=p.repository_id
          WHERE p.state != 'deleting' ORDER BY p.created_at"#,
    )
    .fetch_all(&app.database)
    .await?;
    for pool in pools {
        if let Err(error) = reconcile_pool(app, &pool, &container_states).await {
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

async fn reconcile_pool(
    app: &Reconciler,
    pool: &Pool,
    container_states: &HashMap<String, String>,
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

    let current = runners(app, &pool.id).await?;
    let mut rotated = 0;
    if let Some(stale) = current
        .iter()
        .find(|runner| !runner.busy && runner_needs_update(pool, runner))
    {
        delete_runner(app, pool, stale).await?;
        rotated = 1;
    }

    let current = runners(app, &pool.id).await?;
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
    let refreshed = runners(app, &pool.id).await?;
    let active = refreshed
        .iter()
        .filter(|runner| active_status(&runner.status))
        .collect::<Vec<_>>();
    let mut provisioned = 0;
    let mut removed = 0;
    if active.len() < desired as usize {
        for _ in active.len()..desired as usize {
            provision(app, pool).await?;
            provisioned += 1;
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
    if provisioned > 0 || removed > 0 || rotated > 0 {
        system_event(
            app,
            Some(&pool.id),
            "info",
            "Pool reconciled",
            &format!("Provisioned {provisioned}, removed {removed}, rotated {rotated}, active {active_count}"),
            json!({ "desired": desired, "active": active_count, "provisioned": provisioned, "removed": removed, "rotated": rotated, "outdated": outdated_count }),
        )
        .await?;
    }
    Ok(())
}

async fn maybe_scale_down(app: &Reconciler, pool: &Pool, runners: &[Runner]) -> Result<()> {
    if !pool.autoscaling_enabled || pool.paused || pool.desired_count <= pool.min_count {
        return Ok(());
    }
    if runners.iter().any(|runner| runner.busy) {
        return Ok(());
    }
    let queued = queued_jobs(&app.database, pool).await?;
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

async fn queued_jobs(database: &SqlitePool, pool: &Pool) -> Result<i64> {
    Ok(sqlx::query(
        r#"SELECT COUNT(*) AS count FROM workflow_jobs wj
          JOIN workflow_runs wr ON wr.id=wj.run_id JOIN repositories repo ON repo.id=wr.repository_id
          WHERE wj.status='queued' AND (
            repo.id=(SELECT repository_id FROM runner_pools WHERE id=?) OR
            ((SELECT scope FROM runner_pools WHERE id=?)='organization' AND
             repo.installation_id=(SELECT installation_id FROM runner_pools WHERE id=?))
          )"#,
    )
    .bind(&pool.id)
    .bind(&pool.id)
    .bind(&pool.id)
    .fetch_one(database)
    .await?
    .get::<i64, _>("count"))
}

async fn record_capacity_sample(app: &Reconciler, pool: &Pool, runners: &[Runner]) -> Result<()> {
    let online = runners
        .iter()
        .filter(|runner| matches!(runner.status.as_str(), "online" | "idle" | "busy"))
        .count();
    let busy = runners.iter().filter(|runner| runner.busy).count();
    let available = online.saturating_sub(busy);
    let queued = queued_jobs(&app.database, pool).await?;
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

async fn provision(app: &Reconciler, pool: &Pool) -> Result<()> {
    let token = installation_token(app, pool.installation_id)
        .await?
        .context("GitHub App credentials are required for autonomous reconciliation")?;
    let runner_id = uuid::Uuid::new_v4().to_string();
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let runner_name = format!("{}-{suffix}", pool.name);
    let now = now_millis();
    sqlx::query("INSERT INTO runners (id,pool_id,name,status,ephemeral,configuration_version,created_at,updated_at) VALUES (?,?,?,'starting',?,?,?,?)")
        .bind(&runner_id)
        .bind(&pool.id)
        .bind(&runner_name)
        .bind(pool.ephemeral)
        .bind(pool.configuration_version)
        .bind(now)
        .bind(now)
        .execute(&app.database)
        .await?;

    let result = async {
        let labels = serde_json::from_str::<Vec<String>>(&pool.labels).unwrap_or_default();
        let target = match (&pool.repository_owner, &pool.repository_name) {
            (Some(owner), Some(repository)) => RunnerTarget::Repository { owner, repository },
            _ => RunnerTarget::Organization {
                organization: &pool.account_login,
            },
        };
        let mut request = json!({
            "runnerId": runner_id,
            "poolId": pool.id,
            "name": runner_name,
            "image": pool.image,
            "mode": pool.mode,
            "labels": labels,
            "cpuLimit": pool.cpu_limit,
            "memoryLimitMb": pool.memory_limit_mb,
            "network": app.config.runner_network(),
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
                        labels: serde_json::from_str::<Vec<String>>(&pool.labels)
                            .unwrap_or_default(),
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
            request["registrationUrl"] = Value::String(runner_registration_url(pool));
            if pool.repository_owner.is_none() && pool.runner_group_id != 1 {
                let group = app
                    .github
                    .runner_group_name(&pool.account_login, pool.runner_group_id, &token)
                    .await?;
                request["runnerGroup"] = Value::String(group);
            }
            None
        };
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
        return Err(error);
    }
    system_audit(
        app,
        "runner.provisioned",
        "runner",
        &runner_id,
        json!({ "poolId": pool.id }),
    )
    .await?;
    Ok(())
}

async fn delete_runner(app: &Reconciler, pool: &Pool, runner: &Runner) -> Result<()> {
    if runner.container_id.is_some()
        && let Err(error) = archive_runner_logs(app, pool, runner).await
    {
        tracing::warn!(runner_id = %runner.id, error = ?error, "could not archive runner logs");
    }
    let target = match (&pool.repository_owner, &pool.repository_name) {
        (Some(owner), Some(repository)) => RunnerTarget::Repository { owner, repository },
        _ => RunnerTarget::Organization {
            organization: &pool.account_login,
        },
    };
    match installation_token(app, pool.installation_id).await {
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
                    let path = match (&pool.repository_owner, &pool.repository_name) {
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
        r#"SELECT id,name,container_id,github_runner_id,status,busy,last_job_id,configuration_version,updated_at
           FROM runners WHERE pool_id=? AND deleted_at IS NULL ORDER BY created_at DESC"#,
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

async fn archive_runner_logs(app: &Reconciler, pool: &Pool, runner: &Runner) -> Result<()> {
    let Some(container_id) = runner.container_id.as_deref() else {
        return Ok(());
    };
    if sqlx::query_scalar::<_, String>(
        "SELECT id FROM log_streams WHERE runner_id=? AND source='docker' AND complete=1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&runner.id)
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
    let repository = pool
        .repository_owner
        .as_ref()
        .zip(pool.repository_name.as_ref())
        .map(|(owner, repository)| format!("{owner}/{repository}"));
    let inserted = sqlx::query(
        r#"INSERT INTO log_streams (
          id,runner_id,job_id,installation_id,runner_name,pool_name,repository,source,path,
          size_bytes,complete,checksum,expires_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,'docker',?, ?,1,?,?,?,?)"#,
    )
    .bind(&stream_id)
    .bind(&runner.id)
    .bind(runner.last_job_id)
    .bind(pool.installation_id)
    .bind(&runner.name)
    .bind(&pool.name)
    .bind(repository)
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
        if status == StatusCode::NOT_FOUND {
            bail!("runner manager object was not found");
        }
        bail!(
            "runner manager request failed ({status}): {}",
            text.chars().take(500).collect::<String>()
        );
    }
    Ok(serde_json::from_str(&text)?)
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

fn runner_registration_url(pool: &Pool) -> String {
    match (&pool.repository_owner, &pool.repository_name) {
        (Some(owner), Some(repository)) => format!("https://github.com/{owner}/{repository}"),
        _ => format!("https://github.com/{}", pool.account_login),
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

    fn pool(configuration_version: i64) -> Pool {
        Pool {
            id: "pool-1".into(),
            installation_id: 1,
            account_login: "octo-org".into(),
            repository_owner: None,
            repository_name: None,
            name: "linux".into(),
            mode: "ephemeral".into(),
            labels: "[]".into(),
            image: "runner:latest".into(),
            desired_count: 1,
            min_count: 0,
            max_count: 10,
            cpu_limit: 2.0,
            memory_limit_mb: 4096,
            runner_group_id: 1,
            ephemeral: true,
            paused: false,
            autoscaling_enabled: true,
            idle_timeout_minutes: 5,
            configuration_version,
        }
    }

    fn runner(configuration_version: i64) -> Runner {
        Runner {
            id: "runner-1".into(),
            name: "linux-1234".into(),
            container_id: None,
            github_runner_id: None,
            status: "online".into(),
            busy: false,
            last_job_id: None,
            configuration_version,
            updated_at: 1_000,
        }
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
}
