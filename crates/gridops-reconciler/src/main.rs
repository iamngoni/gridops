use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result, bail};
use gridops_core::{
    Config, GitHubClient, JitRequest, RunnerTarget, Vault, connect_database, now_millis,
};
use reqwest::{Method, StatusCode};
use secrecy::ExposeSecret as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, Row as _, SqlitePool};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

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
}

#[derive(Debug, FromRow)]
struct Runner {
    id: String,
    container_id: Option<String>,
    github_runner_id: Option<i64>,
    status: String,
    busy: bool,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateContainer<'a> {
    runner_id: &'a str,
    pool_id: &'a str,
    name: &'a str,
    image: &'a str,
    jit_config: &'a str,
    cpu_limit: f64,
    memory_limit_mb: i64,
    network: &'a str,
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
          repo.name AS repository_name,p.name,p.labels,p.image,p.desired_count,p.min_count,
          p.max_count,p.cpu_limit,p.memory_limit_mb,p.runner_group_id,p.ephemeral,p.paused,
          p.autoscaling_enabled,p.idle_timeout_minutes FROM runner_pools p
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
                "exited" | "dead" | "missing" => "stopped",
                value => value,
            };
            sqlx::query("UPDATE runners SET status=?,last_heartbeat_at=?,updated_at=? WHERE id=?")
                .bind(status)
                .bind(now_millis())
                .bind(now_millis())
                .bind(&runner.id)
                .execute(&app.database)
                .await?;
        }
    }

    maybe_scale_down(app, pool, &known_runners).await?;
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
    let active_count = runners(app, &pool.id)
        .await?
        .iter()
        .filter(|runner| active_status(&runner.status))
        .count();
    sqlx::query("UPDATE runner_pools SET state=?,updated_at=? WHERE id=?")
        .bind(if pool.paused {
            "paused"
        } else if active_count > desired as usize {
            "draining"
        } else {
            "active"
        })
        .bind(now_millis())
        .bind(&pool.id)
        .execute(&app.database)
        .await?;
    if provisioned > 0 || removed > 0 {
        system_event(
            app,
            Some(&pool.id),
            "info",
            "Pool reconciled",
            &format!("Provisioned {provisioned}, removed {removed}, active {active_count}"),
            json!({ "desired": desired, "active": active_count, "provisioned": provisioned, "removed": removed }),
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
    let queued = sqlx::query(
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
    .fetch_one(&app.database)
    .await?
    .get::<i64, _>("count");
    if queued > 0 {
        return Ok(());
    }
    let last_activity = runners
        .iter()
        .map(|runner| runner.updated_at)
        .max()
        .unwrap_or(0);
    let idle_millis = pool.idle_timeout_minutes * 60 * 1_000;
    if now_millis() - last_activity < idle_millis {
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

async fn provision(app: &Reconciler, pool: &Pool) -> Result<()> {
    let token = installation_token(app, pool.installation_id)
        .await?
        .context("GitHub App credentials are required for autonomous reconciliation")?;
    let runner_id = uuid::Uuid::new_v4().to_string();
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let runner_name = format!("{}-{suffix}", pool.name);
    let now = now_millis();
    sqlx::query("INSERT INTO runners (id,pool_id,name,status,ephemeral,created_at,updated_at) VALUES (?,?,?,'starting',?,?,?)")
        .bind(&runner_id)
        .bind(&pool.id)
        .bind(&runner_name)
        .bind(pool.ephemeral)
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
        let jit = app
            .github
            .generate_jit_config(
                target,
                &token,
                &JitRequest {
                    name: runner_name.clone(),
                    runner_group_id: pool.runner_group_id,
                    labels,
                    work_folder: "_work".into(),
                },
            )
            .await?;
        let created = manager_request::<CreatedRunner>(
            app,
            Method::POST,
            "v1/runners",
            Some(serde_json::to_value(CreateContainer {
                runner_id: &runner_id,
                pool_id: &pool.id,
                name: &runner_name,
                image: &pool.image,
                jit_config: &jit.encoded_jit_config,
                cpu_limit: pool.cpu_limit,
                memory_limit_mb: pool.memory_limit_mb,
                network: app.config.runner_network(),
            })?),
        )
        .await?;
        let updated = now_millis();
        sqlx::query("UPDATE runners SET github_runner_id=?,container_id=?,container_name=?,status=?,registered_at=?,last_heartbeat_at=?,updated_at=? WHERE id=?")
            .bind(jit.runner.id)
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
    if let Some(container_id) = &runner.container_id {
        let result = manager_request::<Value>(
            app,
            Method::DELETE,
            &format!("v1/runners/{container_id}"),
            None,
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(container_id, error = ?error, "could not remove runner container");
        }
    }
    if let Some(github_runner_id) = runner.github_runner_id {
        let token = installation_token(app, pool.installation_id)
            .await?
            .context("GitHub App credentials are required for autonomous reconciliation")?;
        let path = match (&pool.repository_owner, &pool.repository_name) {
            (Some(owner), Some(repository)) => {
                format!("/repos/{owner}/{repository}/actions/runners/{github_runner_id}")
            }
            _ => format!(
                "/orgs/{}/actions/runners/{github_runner_id}",
                pool.account_login
            ),
        };
        app.github.delete(&path, &token).await?;
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

async fn runners(app: &Reconciler, pool_id: &str) -> Result<Vec<Runner>> {
    Ok(sqlx::query_as::<_, Runner>(
        r#"SELECT id,container_id,github_runner_id,status,busy,updated_at
           FROM runners WHERE pool_id=? AND deleted_at IS NULL ORDER BY created_at DESC"#,
    )
    .bind(pool_id)
    .fetch_all(&app.database)
    .await?)
}

async fn cleanup_retention(app: &Reconciler) -> Result<()> {
    let now = now_millis();
    let log_cutoff = now - setting_i64(&app.database, "logRetentionDays", 30).await * 86_400_000;
    let webhook_cutoff =
        now - setting_i64(&app.database, "webhookRetentionDays", 90).await * 86_400_000;
    let audit_cutoff =
        now - setting_i64(&app.database, "auditRetentionDays", 365).await * 86_400_000;
    sqlx::query("DELETE FROM log_streams WHERE expires_at IS NOT NULL AND expires_at < ?")
        .bind(log_cutoff)
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
    Ok(())
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
    matches!(status, "starting" | "online" | "idle" | "busy" | "paused")
}
