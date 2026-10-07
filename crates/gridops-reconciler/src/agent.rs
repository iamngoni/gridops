//! Runs fix agents for failed jobs.
//!
//! The API queues runs; this worker claims them one at a time, reserves host
//! capacity, starts a sandbox container through the manager, copies the
//! failing commit into it, and drives the agent loop. The sandbox never holds
//! a model credential or a GitHub token: the model is called from here, the
//! repository arrives as a tarball streamed through the manager, and the fix
//! is committed through the GitHub API from the sandbox's diff.

use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt as _;
use gridops_agent::{
    catalog::ProviderId,
    changes::{ChangeKind, FileChange, collect_changes, diff_stat, prepare_checkout_script},
    connection::{AgentModel, ModelSettings},
    prompt::{FailedStep, FailureContext, LOG_TAIL_LINES, RunnerContext},
    sandbox::{ExecOutput, ExecRequest, Sandbox},
    session::{AgentEvent, EventKind, RunObserver, SessionError, SessionLimits, run_session},
    tools::{FinishOutcome, FinishRequest, clip},
};
use gridops_core::{
    GitHubWorkflowJob,
    fix_agent::{BRANCH_PREFIX, TriggerMode, fix_branch, load_config, missing_permissions},
    job_log::structure_job_log,
    now_millis,
};
use reqwest::Method;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row as _;

use crate::{
    Reconciler, deferred_manager_error, manager_request, release_runner_capacity,
    reserve_runner_capacity, runtime_secret,
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const AUTOMATIC_SCAN_INTERVAL_MS: i64 = 30_000;
const SANDBOX_CLEANUP_INTERVAL_MS: i64 = 5 * 60_000;
const CAPACITY_RETRY_MS: i64 = 60_000;
/// Failures older than this are never picked up automatically.
const AUTOMATIC_WINDOW_MS: i64 = 6 * 60 * 60 * 1_000;
const MAX_TARBALL_BYTES: u64 = 1_024 * 1_024 * 1_024;
const SANDBOX_POOL_ID: &str = "gridops-agent";

/// Sandbox size and image, from the environment.
struct SandboxShape {
    image: String,
    cpu_limit: f64,
    memory_limit_mb: i64,
}

impl SandboxShape {
    fn from_environment(app: &Reconciler) -> Self {
        let read = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        Self {
            image: read("GRIDOPS_AGENT_SANDBOX_IMAGE")
                .unwrap_or_else(|| app.config.runner_image().to_owned()),
            cpu_limit: read("GRIDOPS_AGENT_SANDBOX_CPUS")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| *value > 0.0)
                .unwrap_or(2.0),
            memory_limit_mb: read("GRIDOPS_AGENT_SANDBOX_MEMORY_MB")
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|value| *value >= 512)
                .unwrap_or(2_048),
        }
    }
}

/// The worker loop. Never returns; errors are logged and retried.
pub async fn worker(app: Reconciler) {
    if let Err(error) = recover_interrupted_runs(&app).await {
        tracing::error!(error = ?error, "could not recover interrupted agent runs");
    }
    let mut last_scan = 0;
    let mut last_cleanup = 0;
    loop {
        let now = now_millis();
        if now - last_cleanup >= SANDBOX_CLEANUP_INTERVAL_MS {
            last_cleanup = now;
            if let Err(error) = cleanup_orphaned_sandboxes(&app).await {
                tracing::warn!(error = ?error, "agent sandbox cleanup failed");
            }
        }
        if now - last_scan >= AUTOMATIC_SCAN_INTERVAL_MS {
            last_scan = now;
            if let Err(error) = queue_automatic_runs(&app).await {
                tracing::warn!(error = ?error, "could not queue automatic agent runs");
            }
        }
        match claim_next_run(&app).await {
            Ok(Some(run_id)) => {
                if let Err(error) = execute_run(&app, &run_id).await {
                    tracing::error!(agent_run_id = %run_id, error = ?error, "agent run failed");
                    let _ = finish_failed(&app, &run_id, &format!("{error:#}")).await;
                }
                continue;
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(error = ?error, "could not claim an agent run"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Runs left `running` by a previous process can never finish.
async fn recover_interrupted_runs(app: &Reconciler) -> Result<()> {
    let rows = sqlx::query("SELECT id FROM agent_runs WHERE status='running'")
        .fetch_all(&app.database)
        .await?;
    for row in rows {
        let id = row.get::<String, _>("id");
        finish_failed(
            app,
            &id,
            "GridOps restarted while this run was in progress. Start it again.",
        )
        .await?;
    }
    Ok(())
}

async fn claim_next_run(app: &Reconciler) -> Result<Option<String>> {
    let now = now_millis();
    let candidate = sqlx::query_scalar::<_, String>(
        r#"SELECT id FROM agent_runs WHERE status='queued' AND (retry_at IS NULL OR retry_at<=?)
          ORDER BY created_at LIMIT 1"#,
    )
    .bind(now)
    .fetch_optional(&app.database)
    .await?;
    let Some(id) = candidate else {
        return Ok(None);
    };
    let claimed = sqlx::query(
        r#"UPDATE agent_runs SET status='running',stage='Preparing',started_at=COALESCE(started_at,?),
          retry_at=NULL,updated_at=? WHERE id=? AND status='queued'"#,
    )
    .bind(now)
    .bind(now)
    .bind(&id)
    .execute(&app.database)
    .await?
    .rows_affected();
    Ok((claimed == 1).then_some(id))
}

#[derive(Debug)]
struct RunPlan {
    id: String,
    job_id: i64,
    run_id: i64,
    job_name: String,
    job_url: String,
    job_labels: Vec<String>,
    runner_name: Option<String>,
    owner: String,
    repository: String,
    default_branch: String,
    head_branch: Option<String>,
    head_sha: String,
    installation_id: i64,
    installation_permissions: Value,
    connection_id: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

async fn load_plan(app: &Reconciler, run_id: &str) -> Result<RunPlan> {
    let row = sqlx::query(
        r#"SELECT agent.id,agent.job_id,agent.run_id,agent.connection_id,agent.provider,agent.model,
          agent.reasoning_effort,job.name AS job_name,job.html_url AS job_url,
          job.labels,job.runner_name,repo.owner,repo.name AS repository,repo.default_branch,
          run.head_branch,run.head_sha,repo.installation_id,installation.permissions
          FROM agent_runs agent
          JOIN workflow_jobs job ON job.id=agent.job_id
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=agent.repository_id
          JOIN installations installation ON installation.id=repo.installation_id
          WHERE agent.id=?"#,
    )
    .bind(run_id)
    .fetch_one(&app.database)
    .await?;
    Ok(RunPlan {
        id: row.get("id"),
        job_id: row.get("job_id"),
        run_id: row.get("run_id"),
        job_name: row.get("job_name"),
        job_url: row.get("job_url"),
        job_labels: serde_json::from_str(row.get::<&str, _>("labels")).unwrap_or_default(),
        runner_name: row.get("runner_name"),
        owner: row.get("owner"),
        repository: row.get("repository"),
        default_branch: row.get("default_branch"),
        head_branch: row.get("head_branch"),
        head_sha: row.get("head_sha"),
        installation_id: row.get("installation_id"),
        installation_permissions: serde_json::from_str(row.get::<&str, _>("permissions"))
            .unwrap_or_default(),
        connection_id: row.get("connection_id"),
        provider: row.get("provider"),
        model: row.get("model"),
        reasoning_effort: row.get("reasoning_effort"),
    })
}

async fn execute_run(app: &Reconciler, run_id: &str) -> Result<()> {
    let plan = load_plan(app, run_id).await?;
    let missing = missing_permissions(&plan.installation_permissions);
    if !missing.is_empty() {
        bail!(
            "The GridOps GitHub App needs {} write access on {}. Update the App's permissions on GitHub and approve them for this installation.",
            missing.join(" and "),
            plan.owner
        );
    }
    let (Some(connection_id), Some(provider), Some(model)) = (
        plan.connection_id.clone(),
        plan.provider.as_deref().and_then(ProviderId::parse),
        plan.model.clone(),
    ) else {
        bail!("This run has no AI connection. Choose one in Settings → AI agent and start again.");
    };
    let credential_key =
        sqlx::query_scalar::<_, String>("SELECT credential_key FROM ai_connections WHERE id=?")
            .bind(&connection_id)
            .fetch_optional(&app.database)
            .await?
            .context("The AI connection this run used has been removed.")?;
    let secret = runtime_secret(app, &credential_key)
        .await?
        .context("The AI connection's credential is missing. Reconnect it in Settings.")?;
    let agent_model = AgentModel::connect(
        provider,
        &secret,
        ModelSettings {
            model: model.clone(),
            reasoning_effort: plan.reasoning_effort.clone(),
            max_output_tokens: 32_000,
        },
    )?;

    stage(app, run_id, "Waiting for runner host capacity").await?;
    let shape = SandboxShape::from_environment(app);
    let sandbox_id = uuid::Uuid::new_v4().to_string();
    let admission = reserve_runner_capacity(
        app,
        &sandbox_id,
        SANDBOX_POOL_ID,
        "docker",
        shape.cpu_limit,
        shape.memory_limit_mb,
    )
    .await?;
    let Some(lease_id) = admission.lease_id else {
        tracing::info!(agent_run_id = %run_id, reason = ?admission.reason, "agent run waiting for capacity");
        return requeue_for_capacity(app, run_id).await;
    };

    stage(app, run_id, "Starting the sandbox").await?;
    let created = manager_request::<Value>(
        app,
        Method::POST,
        "v1/sandboxes",
        Some(json!({
            "sandboxId": sandbox_id,
            "image": shape.image,
            "pullImage": true,
            "cpuLimit": shape.cpu_limit,
            "memoryLimitMb": shape.memory_limit_mb,
            "network": app.config.runner_network(),
            "capacityLease": lease_id,
        })),
    )
    .await;
    if let Err(error) = created {
        release_runner_capacity(app, &lease_id).await;
        if deferred_manager_error(&error) {
            return requeue_for_capacity(app, run_id).await;
        }
        return Err(error.context("The sandbox could not be started"));
    }
    sqlx::query("UPDATE agent_runs SET sandbox_id=?,updated_at=? WHERE id=?")
        .bind(&sandbox_id)
        .bind(now_millis())
        .bind(run_id)
        .execute(&app.database)
        .await?;
    let sandbox = ManagerSandbox {
        app: app.clone(),
        sandbox_id: sandbox_id.clone(),
    };
    let result = drive(
        app,
        &plan,
        &sandbox,
        &agent_model,
        &connection_id,
        &credential_key,
    )
    .await;
    if let Err(error) = manager_request::<Value>(
        app,
        Method::DELETE,
        &format!("v1/sandboxes/{sandbox_id}"),
        None,
    )
    .await
    {
        tracing::warn!(sandbox_id, error = ?error, "could not remove agent sandbox; cleanup will retry");
    }
    result
}

/// Puts the run back in the queue so a busy host delays it instead of failing it.
async fn requeue_for_capacity(app: &Reconciler, run_id: &str) -> Result<()> {
    sqlx::query(
        r#"UPDATE agent_runs SET status='queued',stage='Waiting for runner host capacity',
          retry_at=?,updated_at=? WHERE id=? AND status='running'"#,
    )
    .bind(now_millis() + CAPACITY_RETRY_MS)
    .bind(now_millis())
    .bind(run_id)
    .execute(&app.database)
    .await?;
    Ok(())
}

/// Everything after the sandbox exists, so the caller can always remove it.
async fn drive(
    app: &Reconciler,
    plan: &RunPlan,
    sandbox: &ManagerSandbox,
    model: &AgentModel,
    connection_id: &str,
    credential_key: &str,
) -> Result<()> {
    let run_id = plan.id.as_str();
    let token = repository_token(app, plan).await?;
    stage(app, run_id, "Reading the failure").await?;
    let context = failure_context(app, plan, &token).await?;
    record(
        app,
        run_id,
        EventKind::Status,
        "Read the failure",
        Some(
            context
                .failed_steps
                .iter()
                .map(|step| format!("Failed step: {}", step.name))
                .chain(context.annotations.iter().cloned())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    )
    .await?;
    stage(app, run_id, "Copying the repository").await?;
    upload_repository(app, plan, sandbox, &token).await?;
    let baseline = sandbox
        .exec(ExecRequest {
            command: vec![
                "bash".into(),
                "-lc".into(),
                prepare_checkout_script().into(),
            ],
            workdir: "/workspace".into(),
            timeout_seconds: 600,
            max_output_bytes: 64 * 1_024,
            env: BTreeMap::new(),
        })
        .await?
        .require_success("Preparing the checkout")?;
    record(
        app,
        run_id,
        EventKind::Status,
        "Checked out the failing commit",
        Some(format!(
            "{}/{} at {}",
            plan.owner,
            plan.repository,
            baseline.lines().last().unwrap_or(&plan.head_sha)
        )),
    )
    .await?;

    stage(app, run_id, "Agent working").await?;
    let observer = DatabaseObserver {
        app: app.clone(),
        run_id: run_id.to_owned(),
        connection_id: connection_id.to_owned(),
        credential_key: credential_key.to_owned(),
    };
    let outcome = match run_session(
        model,
        sandbox,
        &context,
        SessionLimits::default(),
        &observer,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(SessionError::Cancelled) => {
            finish_cancelled(app, run_id).await?;
            return Ok(());
        }
        Err(SessionError::Failed(message)) => {
            if credential_failure(&message) {
                mark_connection_failed(app, connection_id, &message).await?;
            }
            bail!("{message}");
        }
    };
    sqlx::query("UPDATE agent_runs SET turns=?,tool_calls=?,updated_at=? WHERE id=?")
        .bind(i64::try_from(outcome.turns).unwrap_or(i64::MAX))
        .bind(i64::try_from(outcome.tool_calls).unwrap_or(i64::MAX))
        .bind(now_millis())
        .bind(run_id)
        .execute(&app.database)
        .await?;
    match outcome.finish.outcome {
        FinishOutcome::Diagnosis => {
            finish_succeeded(app, run_id, &outcome.finish, None).await?;
        }
        FinishOutcome::PullRequest => {
            stage(app, run_id, "Opening the pull request").await?;
            let stat = diff_stat(sandbox).await.unwrap_or_default();
            record(app, run_id, EventKind::Status, "Changes", Some(stat)).await?;
            let changes = collect_changes(sandbox).await?;
            let pull_request =
                open_pull_request(app, plan, &context, &outcome.finish, &changes, &token)
                    .await
                    .context("The fix was ready but the pull request could not be opened")?;
            record(
                app,
                run_id,
                EventKind::Result,
                format!("Opened pull request #{}", pull_request.number),
                Some(pull_request.url.clone()),
            )
            .await?;
            finish_succeeded(app, run_id, &outcome.finish, Some(&pull_request)).await?;
        }
    }
    Ok(())
}

/// A token for this repository only, able to read logs and code and open a
/// pull request. It stays in this process.
async fn repository_token(app: &Reconciler, plan: &RunPlan) -> Result<String> {
    let app_id = match runtime_secret(app, "github.app_id").await? {
        Some(value) => value,
        None => app
            .config
            .github_app_id()
            .context("The GitHub App is not configured.")?
            .to_owned(),
    };
    let private_key = match runtime_secret(app, "github.app_private_key").await? {
        Some(value) => value,
        None => app
            .config
            .github_app_private_key()
            .context("The GitHub App is not configured.")?
            .expose_secret()
            .to_owned(),
    };
    app.github
        .scoped_installation_token(
            plan.installation_id,
            &app_id,
            &private_key,
            &plan.repository,
            json!({ "actions": "read", "contents": "write", "pull_requests": "write" }),
        )
        .await
        .context("GitHub refused a token for this repository. Check that the GridOps App has Contents and Pull requests write access")
}

async fn failure_context(app: &Reconciler, plan: &RunPlan, token: &str) -> Result<FailureContext> {
    let repo_path = format!("/repos/{}/{}", plan.owner, plan.repository);
    let job: GitHubWorkflowJob = app
        .github
        .get(&format!("{repo_path}/actions/jobs/{}", plan.job_id), token)
        .await
        .context("Could not read the job from GitHub")?;
    let run: Value = app
        .github
        .get(&format!("{repo_path}/actions/runs/{}", plan.run_id), token)
        .await
        .context("Could not read the workflow run from GitHub")?;
    let raw_log = job_log(app, &repo_path, plan.job_id, token).await?;
    let parsed = structure_job_log(&raw_log, &job.steps, false);
    let failed_steps = parsed
        .steps
        .iter()
        .filter(|step| step.conclusion.as_deref() == Some("failure"))
        .map(|step| {
            let lines = step
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>();
            let tail = lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n");
            FailedStep {
                name: step.name.clone(),
                log_tail: tail,
            }
        })
        .collect::<Vec<_>>();
    let annotations = parsed
        .annotations
        .iter()
        .filter(|annotation| annotation.level == "error")
        .map(|annotation| format!("{}: {}", annotation.step_name, annotation.message))
        .collect();
    Ok(FailureContext {
        repository: format!("{}/{}", plan.owner, plan.repository),
        default_branch: plan.default_branch.clone(),
        head_branch: plan.head_branch.clone(),
        head_sha: plan.head_sha.clone(),
        event: run
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        workflow_name: run
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("workflow")
            .to_owned(),
        workflow_path: run
            .get("path")
            .and_then(Value::as_str)
            .map(|path| path.split('@').next().unwrap_or(path).to_owned()),
        run_number: run
            .get("run_number")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        run_attempt: run.get("run_attempt").and_then(Value::as_i64).unwrap_or(1),
        job_name: plan.job_name.clone(),
        job_url: plan.job_url.clone(),
        job_labels: plan.job_labels.clone(),
        failed_steps,
        annotations,
        runner: runner_context(app, plan).await?,
    })
}

async fn job_log(app: &Reconciler, repo_path: &str, job_id: i64, token: &str) -> Result<String> {
    let response = app
        .http
        .get(format!(
            "https://api.github.com{repo_path}/actions/jobs/{job_id}/logs"
        ))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .timeout(Duration::from_mins(1))
        .send()
        .await?;
    if matches!(response.status().as_u16(), 404 | 410) {
        return Ok(String::new());
    }
    let bytes = response.error_for_status()?.bytes().await?;
    // The end of the log carries the failure; keep the last 8 MiB.
    let start = bytes.len().saturating_sub(8 * 1_024 * 1_024);
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

async fn runner_context(app: &Reconciler, plan: &RunPlan) -> Result<Option<RunnerContext>> {
    let row = sqlx::query(
        r#"SELECT runner.name,runner.os,runner.architecture,runner.provider,pool.name AS pool,
          pool.cpu_limit,pool.memory_limit_mb,pool.labels
          FROM runners runner JOIN runner_pools pool ON pool.id=runner.pool_id
          WHERE runner.name=? OR runner.current_job_id=? OR runner.last_job_id=?
          ORDER BY CASE WHEN runner.name=? THEN 0 ELSE 1 END,runner.updated_at DESC LIMIT 1"#,
    )
    .bind(plan.runner_name.as_deref().unwrap_or_default())
    .bind(plan.job_id)
    .bind(plan.job_id)
    .bind(plan.runner_name.as_deref().unwrap_or_default())
    .fetch_optional(&app.database)
    .await?;
    Ok(row.map(|row| RunnerContext {
        name: row.get("name"),
        pool: row.get("pool"),
        provider: row.get("provider"),
        os: row.get("os"),
        architecture: row.get("architecture"),
        labels: serde_json::from_str(row.get::<&str, _>("labels")).unwrap_or_default(),
        cpu_limit: row.get("cpu_limit"),
        memory_limit_mb: row.get("memory_limit_mb"),
    }))
}

/// Streams the failing commit's tarball from GitHub into the sandbox without
/// holding it in memory.
async fn upload_repository(
    app: &Reconciler,
    plan: &RunPlan,
    sandbox: &ManagerSandbox,
    token: &str,
) -> Result<()> {
    let response = app
        .http
        .get(format!(
            "https://api.github.com/repos/{}/{}/tarball/{}",
            plan.owner, plan.repository, plan.head_sha
        ))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .timeout(Duration::from_mins(10))
        .send()
        .await
        .context("Could not download the repository")?
        .error_for_status()
        .context("GitHub refused the repository download")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TARBALL_BYTES)
    {
        bail!("The repository is larger than 1 GiB, too large for the fix agent.");
    }
    let mut seen = 0_u64;
    let stream = response.bytes_stream().map(move |chunk| {
        let chunk = chunk?;
        seen += chunk.len() as u64;
        if seen > MAX_TARBALL_BYTES {
            return Err(std::io::Error::other("repository archive exceeds 1 GiB").into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(chunk)
    });
    sandbox
        .upload_body("/workspace", reqwest::Body::wrap_stream(stream))
        .await
        .context("Could not copy the repository into the sandbox")
}

#[derive(Debug)]
struct PullRequest {
    number: i64,
    url: String,
    branch: String,
}

#[derive(Deserialize)]
struct GitObject {
    sha: String,
}

#[derive(Deserialize)]
struct GitCommit {
    tree: GitObject,
}

#[derive(Deserialize)]
struct CreatedPullRequest {
    number: i64,
    html_url: String,
}

/// Commits the agent's changes on a new branch through the Git data API and
/// opens a draft pull request against the failing branch.
async fn open_pull_request(
    app: &Reconciler,
    plan: &RunPlan,
    context: &FailureContext,
    finish: &FinishRequest,
    changes: &[FileChange],
    token: &str,
) -> Result<PullRequest> {
    if changes.is_empty() {
        bail!("The agent finished with a fix but no files changed.");
    }
    let repo_path = format!("/repos/{}/{}", plan.owner, plan.repository);
    let base_commit: GitCommit = app
        .github
        .get(&format!("{repo_path}/git/commits/{}", plan.head_sha), token)
        .await?;
    let mut tree = Vec::with_capacity(changes.len());
    for change in changes {
        match (&change.kind, &change.contents) {
            (ChangeKind::Deleted, _) | (_, None) => tree.push(json!({
                "path": change.path, "mode": "100644", "type": "blob", "sha": Value::Null,
            })),
            (_, Some(contents)) => {
                let blob: GitObject = app
                    .github
                    .post(
                        &format!("{repo_path}/git/blobs"),
                        token,
                        json!({ "content": STANDARD.encode(contents), "encoding": "base64" }),
                    )
                    .await?;
                tree.push(json!({
                    "path": change.path, "mode": change.mode, "type": "blob", "sha": blob.sha,
                }));
            }
        }
    }
    let new_tree: GitObject = app
        .github
        .post(
            &format!("{repo_path}/git/trees"),
            token,
            json!({ "base_tree": base_commit.tree.sha, "tree": tree }),
        )
        .await?;
    let message = format!(
        "{}\n\n{}\n\nFixes the failed \"{}\" job: {}",
        finish.title.trim(),
        finish.summary.trim(),
        plan.job_name,
        plan.job_url
    );
    let commit: GitObject = app
        .github
        .post(
            &format!("{repo_path}/git/commits"),
            token,
            json!({ "message": message, "tree": new_tree.sha, "parents": [plan.head_sha] }),
        )
        .await?;
    let branch = fix_branch(&plan.job_name, &plan.id);
    app.github
        .post_empty(
            &format!("{repo_path}/git/refs"),
            token,
            json!({ "ref": format!("refs/heads/{branch}"), "sha": commit.sha }),
        )
        .await?;
    let body = pull_request_body(plan, context, finish, changes);
    let base = plan
        .head_branch
        .clone()
        .filter(|branch| !branch.starts_with(BRANCH_PREFIX))
        .unwrap_or_else(|| plan.default_branch.clone());
    let mut attempts = vec![(base.clone(), true), (base.clone(), false)];
    if base != plan.default_branch {
        attempts.push((plan.default_branch.clone(), true));
        attempts.push((plan.default_branch.clone(), false));
    }
    let mut last_error = None;
    for (base, draft) in attempts {
        match app
            .github
            .post::<CreatedPullRequest>(
                &format!("{repo_path}/pulls"),
                token,
                json!({
                    "title": finish.title.trim(), "head": branch, "base": base, "body": body,
                    "draft": draft, "maintainer_can_modify": true,
                }),
            )
            .await
        {
            Ok(created) => {
                return Ok(PullRequest {
                    number: created.number,
                    url: created.html_url,
                    branch,
                });
            }
            // 422 covers both "drafts are not available on this plan" and a
            // base branch that no longer exists; try the next combination.
            Err(error) if error.to_string().contains("(422") => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("GitHub did not accept the pull request")))
}

fn pull_request_body(
    plan: &RunPlan,
    context: &FailureContext,
    finish: &FinishRequest,
    changes: &[FileChange],
) -> String {
    let verification = finish
        .verification
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Not verified in the sandbox. CI on this pull request is the check.");
    let files = changes
        .iter()
        .take(30)
        .map(|change| format!("- `{}`", change.path))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{summary}\n\n{body}\n\n### Verification\n\n{verification}\n\n### Files\n\n{files}\n\n---\n\nOpened by the GridOps fix agent for the failed [{job}]({url}) job in {workflow} run #{run} at `{sha}`. Review it like any other change before merging.",
        summary = finish.summary.trim(),
        body = finish.body.trim(),
        job = plan.job_name,
        url = plan.job_url,
        workflow = context.workflow_name,
        run = context.run_number,
        sha = plan.head_sha.chars().take(7).collect::<String>(),
    )
}

/// The manager's sandbox endpoints, as a [`Sandbox`].
struct ManagerSandbox {
    app: Reconciler,
    sandbox_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagerExecOutput {
    exit_code: i64,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    stdout_truncated: bool,
    #[serde(default)]
    stderr_truncated: bool,
    #[serde(default)]
    timed_out: bool,
    #[serde(default)]
    duration_ms: u64,
}

impl ManagerSandbox {
    fn url(&self, path: &str) -> Result<reqwest::Url> {
        Ok(self
            .app
            .config
            .manager_url()
            .join(&format!("v1/sandboxes/{}/{path}", self.sandbox_id))?)
    }

    fn token(&self) -> Result<&str> {
        Ok(self
            .app
            .config
            .manager_token()
            .context("GRIDOPS_MANAGER_TOKEN is required")?
            .expose_secret())
    }

    async fn upload_body(&self, destination: &str, body: reqwest::Body) -> Result<()> {
        let mut url = self.url("archive")?;
        url.query_pairs_mut().append_pair("path", destination);
        let response = self
            .app
            .http
            .put(url)
            .bearer_auth(self.token()?)
            .header("Content-Type", "application/x-tar")
            .timeout(Duration::from_mins(10))
            .body(body)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            bail!("sandbox upload failed ({status}): {}", clip(&text, 400));
        }
        Ok(())
    }
}

#[async_trait]
impl Sandbox for ManagerSandbox {
    async fn exec(&self, request: ExecRequest) -> Result<ExecOutput> {
        let response = self
            .app
            .http
            .post(self.url("exec")?)
            .bearer_auth(self.token()?)
            .timeout(Duration::from_secs(request.timeout_seconds + 90))
            .json(&json!({
                "command": request.command,
                "workdir": request.workdir,
                "timeoutSeconds": request.timeout_seconds,
                "maxOutputBytes": request.max_output_bytes,
                "env": request.env,
            }))
            .send()
            .await
            .context("The sandbox did not respond")?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            bail!(
                "sandbox command failed to run ({status}): {}",
                clip(&text, 400)
            );
        }
        let output: ManagerExecOutput = response.json().await?;
        Ok(ExecOutput {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            stdout_truncated: output.stdout_truncated,
            stderr_truncated: output.stderr_truncated,
            timed_out: output.timed_out,
            duration_ms: output.duration_ms,
        })
    }

    async fn upload_tar(&self, destination: &str, archive: Vec<u8>) -> Result<()> {
        self.upload_body(destination, reqwest::Body::from(archive))
            .await
    }
}

/// Records progress in the run's timeline and watches for cancellation.
struct DatabaseObserver {
    app: Reconciler,
    run_id: String,
    connection_id: String,
    credential_key: String,
}

#[async_trait]
impl RunObserver for DatabaseObserver {
    async fn record(&self, event: AgentEvent) {
        if let Err(error) = record(
            &self.app,
            &self.run_id,
            event.kind,
            event.title,
            event.detail,
        )
        .await
        {
            tracing::warn!(agent_run_id = %self.run_id, error = ?error, "could not record agent event");
        }
    }

    async fn cancelled(&self) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT cancel_requested FROM agent_runs WHERE id=?")
            .bind(&self.run_id)
            .fetch_optional(&self.app.database)
            .await
            .ok()
            .flatten()
            .is_some_and(|value| value != 0)
    }

    async fn credential_refreshed(&self, secret: SecretString) {
        let sealed = match self.app.vault.seal(secret.expose_secret()) {
            Ok(sealed) => sealed,
            Err(error) => {
                tracing::error!(error = ?error, "could not seal refreshed AI credential");
                return;
            }
        };
        let saved = sqlx::query("UPDATE runtime_secrets SET value=?,updated_at=? WHERE key=?")
            .bind(sealed)
            .bind(now_millis())
            .bind(&self.credential_key)
            .execute(&self.app.database)
            .await;
        if let Err(error) = saved {
            tracing::error!(connection_id = %self.connection_id, error = ?error, "could not store refreshed AI credential");
        }
    }
}

async fn record(
    app: &Reconciler,
    run_id: &str,
    kind: EventKind,
    title: impl Into<String>,
    detail: Option<String>,
) -> Result<()> {
    let title = clip(&title.into(), 300);
    let now = now_millis();
    sqlx::query(
        "INSERT INTO agent_run_events (agent_run_id,kind,title,detail,created_at) VALUES (?,?,?,?,?)",
    )
    .bind(run_id)
    .bind(kind.as_str())
    .bind(&title)
    .bind(detail.map(|detail| clip(&detail, 12_000)))
    .bind(now)
    .execute(&app.database)
    .await?;
    if kind == EventKind::Tool {
        sqlx::query("UPDATE agent_runs SET stage=?,updated_at=? WHERE id=? AND status='running'")
            .bind(&title)
            .bind(now)
            .bind(run_id)
            .execute(&app.database)
            .await?;
    }
    Ok(())
}

async fn stage(app: &Reconciler, run_id: &str, stage: &str) -> Result<()> {
    sqlx::query("UPDATE agent_runs SET stage=?,updated_at=? WHERE id=?")
        .bind(stage)
        .bind(now_millis())
        .bind(run_id)
        .execute(&app.database)
        .await?;
    Ok(())
}

async fn finish_succeeded(
    app: &Reconciler,
    run_id: &str,
    finish: &FinishRequest,
    pull_request: Option<&PullRequest>,
) -> Result<()> {
    let now = now_millis();
    sqlx::query(
        r#"UPDATE agent_runs SET status='succeeded',stage=?,outcome=?,title=?,summary=?,details=?,
          verification=?,pull_request_url=?,pull_request_number=?,branch=?,completed_at=?,updated_at=?
          WHERE id=?"#,
    )
    .bind(if pull_request.is_some() {
        "Pull request opened"
    } else {
        "Diagnosis ready"
    })
    .bind(if pull_request.is_some() {
        "pull_request"
    } else {
        "diagnosis"
    })
    .bind(finish.title.trim())
    .bind(finish.summary.trim())
    .bind(finish.body.trim())
    .bind(finish.verification.as_deref().map(str::trim))
    .bind(pull_request.map(|pull_request| pull_request.url.as_str()))
    .bind(pull_request.map(|pull_request| pull_request.number))
    .bind(pull_request.map(|pull_request| pull_request.branch.as_str()))
    .bind(now)
    .bind(now)
    .bind(run_id)
    .execute(&app.database)
    .await?;
    if pull_request.is_none() {
        record(
            app,
            run_id,
            EventKind::Result,
            finish.title.trim(),
            Some(finish.summary.trim().to_owned()),
        )
        .await?;
    }
    Ok(())
}

async fn finish_failed(app: &Reconciler, run_id: &str, error: &str) -> Result<()> {
    let now = now_millis();
    let error = clip(error, 2_000);
    let updated = sqlx::query(
        r#"UPDATE agent_runs SET status='failed',stage='Failed',error=?,completed_at=?,updated_at=?
          WHERE id=? AND status IN ('queued','running')"#,
    )
    .bind(&error)
    .bind(now)
    .bind(now)
    .bind(run_id)
    .execute(&app.database)
    .await?
    .rows_affected();
    if updated > 0 {
        record(app, run_id, EventKind::Error, "Run failed", Some(error)).await?;
    }
    Ok(())
}

async fn finish_cancelled(app: &Reconciler, run_id: &str) -> Result<()> {
    let now = now_millis();
    sqlx::query(
        r#"UPDATE agent_runs SET status='cancelled',stage='Cancelled',completed_at=?,updated_at=?
          WHERE id=?"#,
    )
    .bind(now)
    .bind(now)
    .bind(run_id)
    .execute(&app.database)
    .await?;
    Ok(())
}

fn credential_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "401",
        "403",
        "reconnect",
        "invalid x-api-key",
        "invalid api key",
        "unauthorized",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

async fn mark_connection_failed(
    app: &Reconciler,
    connection_id: &str,
    message: &str,
) -> Result<()> {
    let auth_method =
        sqlx::query_scalar::<_, String>("SELECT auth_method FROM ai_connections WHERE id=?")
            .bind(connection_id)
            .fetch_optional(&app.database)
            .await?;
    let status = if auth_method.as_deref() == Some("subscription_oauth") {
        "needs_reconnect"
    } else {
        "error"
    };
    sqlx::query("UPDATE ai_connections SET status=?,status_detail=?,updated_at=? WHERE id=?")
        .bind(status)
        .bind(clip(message, 500))
        .bind(now_millis())
        .bind(connection_id)
        .execute(&app.database)
        .await?;
    Ok(())
}

/// Queues runs for new failures when automatic mode is on, within the daily
/// limit. One run per workflow run, so a failing matrix doesn't start a
/// dozen agents, and never for the agent's own fix branches.
async fn queue_automatic_runs(app: &Reconciler) -> Result<()> {
    let config = load_config(&app.database).await?;
    if config.trigger_mode != TriggerMode::Automatic {
        return Ok(());
    }
    let Some(connection) = gridops_core::fix_agent::active_connection(&app.database).await? else {
        return Ok(());
    };
    let now = now_millis();
    let used = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM agent_runs WHERE trigger='automatic' AND created_at>=?",
    )
    .bind(now - 24 * 60 * 60 * 1_000)
    .fetch_one(&app.database)
    .await?;
    let remaining = config.daily_limit - used;
    if remaining <= 0 {
        return Ok(());
    }
    let since = config
        .automatic_since
        .unwrap_or(now)
        .max(now - AUTOMATIC_WINDOW_MS);
    let candidates = sqlx::query(
        r#"SELECT job.id,job.run_id,run.repository_id,installation.permissions
          FROM workflow_jobs job
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=run.repository_id
          JOIN installations installation ON installation.id=repo.installation_id
          WHERE job.conclusion='failure' AND COALESCE(job.completed_at,job.updated_at)>=?
            AND repo.archived=0 AND installation.suspended_at IS NULL
            AND (run.head_branch IS NULL OR run.head_branch NOT LIKE ?)
            AND NOT EXISTS (SELECT 1 FROM agent_runs agent WHERE agent.run_id=job.run_id)
          ORDER BY COALESCE(job.completed_at,job.updated_at),job.id LIMIT 50"#,
    )
    .bind(since)
    .bind(format!("{BRANCH_PREFIX}%"))
    .fetch_all(&app.database)
    .await?;
    let mut queued_runs = HashSet::new();
    let mut queued = 0;
    for row in candidates {
        if queued >= remaining {
            break;
        }
        let run_id = row.get::<i64, _>("run_id");
        let permissions =
            serde_json::from_str::<Value>(row.get::<&str, _>("permissions")).unwrap_or_default();
        if !queued_runs.insert(run_id) || !missing_permissions(&permissions).is_empty() {
            continue;
        }
        let inserted = sqlx::query(
            r#"INSERT INTO agent_runs
              (id,job_id,run_id,repository_id,trigger,status,stage,connection_id,provider,model,
               reasoning_effort,created_at,updated_at)
              VALUES (?,?,?,?,'automatic','queued','Waiting for the agent worker',?,?,?,?,?,?)"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(row.get::<i64, _>("id"))
        .bind(run_id)
        .bind(row.get::<i64, _>("repository_id"))
        .bind(&connection.id)
        .bind(&connection.provider)
        .bind(&connection.model)
        .bind(&connection.reasoning_effort)
        .bind(now)
        .bind(now)
        .execute(&app.database)
        .await;
        match inserted {
            Ok(_) => queued += 1,
            Err(error)
                if error
                    .as_database_error()
                    .is_some_and(|error| error.is_unique_violation()) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if queued > 0 {
        tracing::info!(queued, "queued automatic agent runs");
    }
    Ok(())
}

#[derive(Deserialize)]
struct SandboxList {
    sandboxes: Vec<ListedSandbox>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedSandbox {
    sandbox_id: String,
}

/// Removes sandboxes no running run owns, e.g. after a crash.
async fn cleanup_orphaned_sandboxes(app: &Reconciler) -> Result<()> {
    let listed = manager_request::<SandboxList>(app, Method::GET, "v1/sandboxes", None).await?;
    if listed.sandboxes.is_empty() {
        return Ok(());
    }
    let active = sqlx::query_scalar::<_, String>(
        "SELECT sandbox_id FROM agent_runs WHERE status='running' AND sandbox_id IS NOT NULL",
    )
    .fetch_all(&app.database)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    for sandbox in listed.sandboxes {
        if active.contains(&sandbox.sandbox_id) {
            continue;
        }
        if let Err(error) = manager_request::<Value>(
            app,
            Method::DELETE,
            &format!("v1/sandboxes/{}", sandbox.sandbox_id),
            None,
        )
        .await
        {
            tracing::warn!(sandbox_id = %sandbox.sandbox_id, error = ?error, "could not remove orphaned agent sandbox");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> RunPlan {
        RunPlan {
            id: "4f9c2b1e-0000".into(),
            job_id: 2,
            run_id: 1,
            job_name: "build".into(),
            job_url: "https://github.com/iamngoni/shipit/actions/runs/1/job/2".into(),
            job_labels: Vec::new(),
            runner_name: None,
            owner: "iamngoni".into(),
            repository: "shipit".into(),
            default_branch: "main".into(),
            head_branch: Some("feature".into()),
            head_sha: "abcdef1234567".into(),
            installation_id: 7,
            installation_permissions: json!({}),
            connection_id: None,
            provider: None,
            model: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn pull_request_bodies_link_back_to_the_failure() {
        let finish = FinishRequest {
            outcome: FinishOutcome::PullRequest,
            title: "Pin pnpm".into(),
            summary: "The setup step had no pnpm version.".into(),
            body: "Added packageManager to package.json.".into(),
            verification: Some("pnpm install succeeded.".into()),
        };
        let context = FailureContext {
            workflow_name: "CI".into(),
            run_number: 99,
            ..FailureContext::default()
        };
        let changes = vec![FileChange {
            path: "package.json".into(),
            kind: ChangeKind::Modified,
            mode: "100644".into(),
            contents: Some(b"{}".to_vec()),
        }];
        let body = pull_request_body(&plan(), &context, &finish, &changes);
        assert!(body.starts_with("The setup step had no pnpm version."));
        assert!(body.contains("### Verification\n\npnpm install succeeded."));
        assert!(body.contains("- `package.json`"));
        assert!(body.contains("[build](https://github.com/iamngoni/shipit/actions/runs/1/job/2) job in CI run #99 at `abcdef1`"));
    }

    #[test]
    fn credential_errors_mark_the_connection() {
        assert!(credential_failure(
            "The model request failed: Claude Code request failed (401 Unauthorized)"
        ));
        assert!(credential_failure(
            "Claude.ai connection needs to be reconnected"
        ));
        assert!(!credential_failure(
            "The model request failed: 529 overloaded"
        ));
    }
}
