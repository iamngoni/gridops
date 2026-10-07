//! Fix-agent endpoints: AI provider connections, agent settings, and the runs
//! started from failed jobs.
//!
//! Provider credentials are sealed in `runtime_secrets` and never returned.
//! Runs are only queued here; the reconciler executes them.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::SecondsFormat;
use gridops_agent::{
    catalog::{ALL_PROVIDERS, AuthMethod, ModelOption, ProviderId, default_models},
    connection::{api_key_secret, fingerprint, list_models, verify_api_key},
    provider_auth,
};
use gridops_core::{
    GitHubWorkflowJob,
    fix_agent::{
        AUTOMATIC_SINCE_SETTING, CONNECTION_SETTING, DAILY_LIMIT_SETTING, MODEL_SETTING,
        REASONING_EFFORT_SETTING, TRIGGER_MODE_SETTING, TriggerMode, active_connection,
        credential_key, load_config, missing_permissions,
    },
    now_millis,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Row as _, sqlite::SqliteRow};

use crate::{
    auth::{AuthUser, assert_installation_admin, audit, require_system_admin},
    error::{ApiError, ApiResult},
    oauth::control_token,
    resources::SameOrigin,
    state::AppState,
};

const OAUTH_ATTEMPT_TTL_MILLIS: i64 = 15 * 60 * 1_000;
const MAX_EVENTS: i64 = 500;
const REASONING_EFFORTS: [&str; 3] = ["low", "medium", "high"];

fn iso(value: Option<i64>) -> Option<String> {
    value
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn provider(value: &str) -> ApiResult<ProviderId> {
    ProviderId::parse(value)
        .ok_or_else(|| ApiError::BadRequest("Choose a supported AI provider.".into()))
}

// ---------------------------------------------------------------------------
// Settings

pub async fn ai_settings(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    Ok(Json(settings_json(&state, &user).await?))
}

async fn settings_json(state: &AppState, user: &AuthUser) -> ApiResult<Value> {
    let rows = sqlx::query(
        r#"SELECT id,provider,auth_method,account_label,fingerprint,status,status_detail,
          created_at,updated_at FROM ai_connections ORDER BY created_at,id"#,
    )
    .fetch_all(&state.database)
    .await?;
    let connections = rows.iter().map(connection_json).collect::<Vec<_>>();
    let config = load_config(&state.database).await?;
    let configured = active_connection(&state.database).await?.is_some();
    Ok(json!({
        "canManage": user.role == "admin",
        "providers": ALL_PROVIDERS.iter().map(|provider| json!({
            "id": provider.as_str(),
            "name": provider.name(),
            "authMethod": provider.auth_method().as_str(),
            "description": provider.description(),
            "keyHint": provider.key_prefix().map(|prefix| format!("{prefix}…")),
        })).collect::<Vec<_>>(),
        "connections": connections,
        "agent": {
            "connectionId": config.connection_id,
            "model": config.model,
            "reasoningEffort": config.reasoning_effort,
            "triggerMode": config.trigger_mode.as_str(),
            "dailyLimit": config.daily_limit,
        },
        "configured": configured,
        "github": github_permissions_json(state, user).await?,
    }))
}

fn connection_json(row: &SqliteRow) -> Value {
    let provider_id = row.get::<String, _>("provider");
    let provider_name = ProviderId::parse(&provider_id).map_or_else(
        || provider_id.clone(),
        |provider| provider.name().to_owned(),
    );
    json!({
        "id": row.get::<String, _>("id"),
        "providerName": provider_name,
        "provider": provider_id,
        "authMethod": row.get::<String, _>("auth_method"),
        "accountLabel": row.get::<Option<String>, _>("account_label"),
        "fingerprint": row.get::<Option<String>, _>("fingerprint"),
        "status": row.get::<String, _>("status"),
        "statusDetail": row.get::<Option<String>, _>("status_detail"),
        "createdAt": iso(Some(row.get::<i64, _>("created_at"))),
        "updatedAt": iso(Some(row.get::<i64, _>("updated_at"))),
    })
}

/// Installations the viewer can see whose App grant is missing what the
/// agent needs to read code and open pull requests.
async fn github_permissions_json(state: &AppState, user: &AuthUser) -> ApiResult<Value> {
    let rows = sqlx::query(
        r#"SELECT installation.id,installation.account_login,installation.permissions
          FROM installations installation
          JOIN user_installations access ON access.installation_id=installation.id
          WHERE access.user_id=? AND installation.suspended_at IS NULL
          ORDER BY installation.account_login"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let missing = rows
        .iter()
        .filter_map(|row| {
            let permissions = serde_json::from_str::<Value>(row.get::<&str, _>("permissions"))
                .unwrap_or_default();
            let missing = missing_permissions(&permissions);
            (!missing.is_empty()).then(|| {
                json!({
                    "installationId": row.get::<i64, _>("id"),
                    "account": row.get::<String, _>("account_login"),
                    "permissions": missing,
                })
            })
        })
        .collect::<Vec<_>>();
    let app_settings_url = state
        .github_app_slug()
        .await
        .ok()
        .map(|slug| format!("https://github.com/settings/apps/{slug}/permissions"));
    Ok(json!({
        "ready": missing.is_empty(),
        "missing": missing,
        "appSettingsUrl": app_settings_url,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyInput {
    provider: String,
    api_key: String,
}

pub async fn save_api_key(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Json(input): Json<ApiKeyInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_system_admin(&user)?;
    let provider = provider(&input.provider)?;
    if provider.auth_method() != AuthMethod::ApiKey {
        return Err(ApiError::BadRequest(format!(
            "{} connects by signing in, not with a key.",
            provider.name()
        )));
    }
    let api_key = input.api_key.trim();
    let models = verify_api_key(provider, api_key)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let secret = api_key_secret(api_key).map_err(ApiError::Internal)?;
    let connection = upsert_connection(
        &state,
        &user,
        provider,
        &secret,
        None,
        Some(fingerprint(api_key)),
        &models,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(connection)))
}

#[derive(Deserialize)]
pub struct OAuthStartInput {
    provider: String,
}

pub async fn start_oauth(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Json(input): Json<OAuthStartInput>,
) -> ApiResult<Json<Value>> {
    require_system_admin(&user)?;
    let provider = provider(&input.provider)?;
    if provider.auth_method() != AuthMethod::SubscriptionOAuth {
        return Err(ApiError::BadRequest(format!(
            "{} connects with an API key.",
            provider.name()
        )));
    }
    let prepared = provider_auth::prepare(provider.as_str()).map_err(ApiError::Internal)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_millis();
    let expires_at = now + OAUTH_ATTEMPT_TTL_MILLIS;
    sqlx::query("DELETE FROM ai_oauth_attempts WHERE expires_at < ? OR user_id=?")
        .bind(now)
        .bind(&user.id)
        .execute(&state.database)
        .await?;
    sqlx::query(
        r#"INSERT INTO ai_oauth_attempts
          (id,provider,user_id,state,verifier_sealed,callback_port,expires_at,created_at)
          VALUES (?,?,?,?,?,?,?,?)"#,
    )
    .bind(&id)
    .bind(provider.as_str())
    .bind(&user.id)
    .bind(&prepared.state)
    .bind(
        state
            .vault
            .seal(&prepared.code_verifier)
            .map_err(ApiError::Internal)?,
    )
    .bind(prepared.callback_port)
    .bind(expires_at)
    .bind(now)
    .execute(&state.database)
    .await?;
    let instructions = match provider {
        ProviderId::Codex => {
            "Sign in to ChatGPT in the tab that opened. When it finishes, the tab lands on a localhost page that won't load. That's expected: copy the full address from the address bar and paste it here."
        }
        _ => {
            "Sign in to Claude in the tab that opened and authorize access. Claude then shows a code. Copy it and paste it here."
        }
    };
    Ok(Json(json!({
        "attemptId": id,
        "authorizeUrl": prepared.authorize_url,
        "expiresAt": iso(Some(expires_at)),
        "instructions": instructions,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCompleteInput {
    attempt_id: String,
    callback_value: String,
}

pub async fn complete_oauth(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Json(input): Json<OAuthCompleteInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_system_admin(&user)?;
    let row = sqlx::query(
        r#"SELECT provider,state,verifier_sealed,callback_port,expires_at
          FROM ai_oauth_attempts WHERE id=? AND user_id=?"#,
    )
    .bind(&input.attempt_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::BadRequest("That sign-in has expired. Start again.".into()))?;
    if row.get::<i64, _>("expires_at") < now_millis() {
        return Err(ApiError::BadRequest(
            "That sign-in has expired. Start again.".into(),
        ));
    }
    let provider = provider(row.get::<&str, _>("provider"))?;
    let expected_state = row.get::<String, _>("state");
    let (code, pasted_state) =
        provider_auth::split_pasted_value(provider.as_str(), &input.callback_value);
    if code.is_empty() || code.len() > 4_096 {
        return Err(ApiError::BadRequest(
            "Paste the code or address the provider showed after you signed in.".into(),
        ));
    }
    if pasted_state
        .as_deref()
        .is_some_and(|value| value != expected_state)
    {
        return Err(ApiError::BadRequest(
            "That code belongs to a different sign-in. Start again.".into(),
        ));
    }
    let verifier = state
        .vault
        .open(row.get::<&str, _>("verifier_sealed"))
        .map_err(ApiError::Internal)?;
    let exchanged = provider_auth::exchange(
        provider.as_str(),
        &verifier,
        &expected_state,
        row.get::<Option<i32>, _>("callback_port"),
        &code,
    )
    .await
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    sqlx::query("DELETE FROM ai_oauth_attempts WHERE id=?")
        .bind(&input.attempt_id)
        .execute(&state.database)
        .await?;
    let connection = upsert_connection(
        &state,
        &user,
        provider,
        &exchanged.secret,
        Some(exchanged.account_label),
        None,
        &default_models(provider),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(connection)))
}

/// Stores a connection's credential, replacing any earlier connection to the
/// same provider. The first connection is also selected for the agent, with a
/// sensible model, so connecting is enough to turn the agent on.
async fn upsert_connection(
    state: &AppState,
    user: &AuthUser,
    provider: ProviderId,
    secret: &str,
    account_label: Option<String>,
    fingerprint: Option<String>,
    models: &[ModelOption],
) -> ApiResult<Value> {
    let sealed = state.vault.seal(secret).map_err(ApiError::Internal)?;
    let now = now_millis();
    let existing =
        sqlx::query_scalar::<_, String>("SELECT id FROM ai_connections WHERE provider=?")
            .bind(provider.as_str())
            .fetch_optional(&state.database)
            .await?;
    let id = existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let key = credential_key(&id);
    let mut transaction = state.database.begin().await?;
    sqlx::query(
        r#"INSERT INTO runtime_secrets (key,value,updated_by,updated_at) VALUES (?,?,?,?)
          ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_by=excluded.updated_by,
            updated_at=excluded.updated_at"#,
    )
    .bind(&key)
    .bind(&sealed)
    .bind(&user.id)
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"INSERT INTO ai_connections
          (id,provider,auth_method,credential_key,account_label,fingerprint,status,status_detail,
           created_by,created_at,updated_at)
          VALUES (?,?,?,?,?,?,'ready',NULL,?,?,?)
          ON CONFLICT(id) DO UPDATE SET account_label=excluded.account_label,
            fingerprint=excluded.fingerprint,status='ready',status_detail=NULL,
            updated_at=excluded.updated_at"#,
    )
    .bind(&id)
    .bind(provider.as_str())
    .bind(provider.auth_method().as_str())
    .bind(&key)
    .bind(&account_label)
    .bind(&fingerprint)
    .bind(&user.id)
    .bind(now)
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    let config = load_config(&state.database).await?;
    if config.connection_id.is_none()
        && let Some(model) = preferred_model(provider, models)
    {
        put_setting(
            &mut transaction,
            CONNECTION_SETTING,
            json!(id),
            &user.id,
            now,
        )
        .await?;
        put_setting(
            &mut transaction,
            MODEL_SETTING,
            json!(model.id),
            &user.id,
            now,
        )
        .await?;
        put_setting(
            &mut transaction,
            REASONING_EFFORT_SETTING,
            json!(model.default_effort),
            &user.id,
            now,
        )
        .await?;
    }
    transaction.commit().await?;
    audit(
        state,
        user,
        "ai_provider.connected",
        "ai_connection",
        Some(&id),
        json!({ "provider": provider.as_str(), "account": account_label }),
    )
    .await?;
    let row = sqlx::query(
        r#"SELECT id,provider,auth_method,account_label,fingerprint,status,status_detail,
          created_at,updated_at FROM ai_connections WHERE id=?"#,
    )
    .bind(&id)
    .fetch_one(&state.database)
    .await?;
    Ok(connection_json(&row))
}

/// The model a new connection starts with: the catalogue's first choice when
/// the provider offers it, otherwise the first model the provider listed.
fn preferred_model(provider: ProviderId, models: &[ModelOption]) -> Option<ModelOption> {
    default_models(provider)
        .into_iter()
        .find(|preferred| models.iter().any(|model| model.id == preferred.id))
        .or_else(|| models.first().cloned())
}

async fn put_setting(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &str,
    value: Value,
    user_id: &str,
    now: i64,
) -> ApiResult<()> {
    sqlx::query(
        r#"INSERT INTO settings (key,value,updated_by,updated_at) VALUES (?,?,?,?)
          ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_by=excluded.updated_by,
            updated_at=excluded.updated_at"#,
    )
    .bind(key)
    .bind(value.to_string())
    .bind(user_id)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn delete_connection(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Path(connection_id): Path<String>,
) -> ApiResult<StatusCode> {
    require_system_admin(&user)?;
    let row = sqlx::query("SELECT provider,credential_key FROM ai_connections WHERE id=?")
        .bind(&connection_id)
        .fetch_optional(&state.database)
        .await?
        .ok_or_else(|| ApiError::NotFound("That AI connection does not exist.".into()))?;
    let config = load_config(&state.database).await?;
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    sqlx::query("DELETE FROM ai_connections WHERE id=?")
        .bind(&connection_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM runtime_secrets WHERE key=?")
        .bind(row.get::<&str, _>("credential_key"))
        .execute(&mut *transaction)
        .await?;
    if config.connection_id.as_deref() == Some(connection_id.as_str()) {
        for key in [CONNECTION_SETTING, MODEL_SETTING, REASONING_EFFORT_SETTING] {
            put_setting(&mut transaction, key, Value::Null, &user.id, now).await?;
        }
    }
    transaction.commit().await?;
    audit(
        &state,
        &user,
        "ai_provider.disconnected",
        "ai_connection",
        Some(&connection_id),
        json!({ "provider": row.get::<String, _>("provider") }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn connection_models(
    State(state): State<AppState>,
    user: AuthUser,
    Path(connection_id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_system_admin(&user)?;
    let row = sqlx::query("SELECT provider,credential_key FROM ai_connections WHERE id=?")
        .bind(&connection_id)
        .fetch_optional(&state.database)
        .await?
        .ok_or_else(|| ApiError::NotFound("That AI connection does not exist.".into()))?;
    let provider = provider(row.get::<&str, _>("provider"))?;
    let secret = state
        .runtime_secret(row.get::<&str, _>("credential_key"))
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound("That connection's credential is missing.".into()))?;
    match list_models(provider, &secret).await {
        Ok(models) => Ok(Json(json!({ "models": models }))),
        Err(error) => Ok(Json(json!({
            "models": default_models(provider),
            "error": error.to_string(),
        }))),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettingsInput {
    connection_id: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    trigger_mode: String,
    daily_limit: i64,
}

pub async fn save_agent_settings(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Json(input): Json<AgentSettingsInput>,
) -> ApiResult<Json<Value>> {
    require_system_admin(&user)?;
    let trigger_mode = TriggerMode::parse(&input.trigger_mode)
        .ok_or_else(|| ApiError::BadRequest("Trigger must be manual or automatic.".into()))?;
    if !(1..=100).contains(&input.daily_limit) {
        return Err(ApiError::BadRequest(
            "The daily limit must be between 1 and 100 runs.".into(),
        ));
    }
    let connection_id = input.connection_id.filter(|id| !id.is_empty());
    if let Some(connection_id) = &connection_id {
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM ai_connections WHERE id=?")
            .bind(connection_id)
            .fetch_one(&state.database)
            .await?;
        if exists == 0 {
            return Err(ApiError::BadRequest(
                "Choose one of the connected providers.".into(),
            ));
        }
    }
    let model = input
        .model
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty());
    if model.as_ref().is_some_and(|model| {
        model.len() > 200 || model.chars().any(|character| character.is_whitespace())
    }) {
        return Err(ApiError::BadRequest("That model id is not valid.".into()));
    }
    if connection_id.is_some() != model.is_some() {
        return Err(ApiError::BadRequest(
            "Choose both a provider connection and a model.".into(),
        ));
    }
    let effort = input.reasoning_effort.filter(|effort| !effort.is_empty());
    if effort
        .as_deref()
        .is_some_and(|effort| !REASONING_EFFORTS.contains(&effort))
    {
        return Err(ApiError::BadRequest(
            "Reasoning effort must be low, medium or high.".into(),
        ));
    }
    let previous = load_config(&state.database).await?;
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    put_setting(
        &mut transaction,
        CONNECTION_SETTING,
        json!(connection_id),
        &user.id,
        now,
    )
    .await?;
    put_setting(&mut transaction, MODEL_SETTING, json!(model), &user.id, now).await?;
    put_setting(
        &mut transaction,
        REASONING_EFFORT_SETTING,
        json!(effort),
        &user.id,
        now,
    )
    .await?;
    put_setting(
        &mut transaction,
        TRIGGER_MODE_SETTING,
        json!(trigger_mode.as_str()),
        &user.id,
        now,
    )
    .await?;
    put_setting(
        &mut transaction,
        DAILY_LIMIT_SETTING,
        json!(input.daily_limit),
        &user.id,
        now,
    )
    .await?;
    if trigger_mode == TriggerMode::Automatic && previous.trigger_mode != TriggerMode::Automatic {
        put_setting(
            &mut transaction,
            AUTOMATIC_SINCE_SETTING,
            json!(now),
            &user.id,
            now,
        )
        .await?;
    }
    transaction.commit().await?;
    audit(
        &state,
        &user,
        "fix_agent.settings_updated",
        "system",
        Some("fix-agent"),
        json!({
            "connectionId": connection_id, "model": model, "reasoningEffort": effort,
            "triggerMode": trigger_mode.as_str(), "dailyLimit": input.daily_limit,
        }),
    )
    .await?;
    Ok(Json(settings_json(&state, &user).await?))
}

// ---------------------------------------------------------------------------
// Runs

struct JobAccess {
    run_id: i64,
    repository_id: i64,
    installation_id: i64,
    owner: String,
    repository: String,
    conclusion: Option<String>,
    head_branch: Option<String>,
    archived: bool,
}

async fn job_access(state: &AppState, user: &AuthUser, job_id: i64) -> ApiResult<JobAccess> {
    let row = sqlx::query(
        r#"SELECT job.run_id,job.conclusion,run.head_branch,repo.id AS repository_id,
          repo.installation_id,repo.owner,repo.name,repo.archived
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
    Ok(JobAccess {
        run_id: row.get("run_id"),
        repository_id: row.get("repository_id"),
        installation_id: row.get("installation_id"),
        owner: row.get("owner"),
        repository: row.get("name"),
        conclusion: row.get("conclusion"),
        head_branch: row.get("head_branch"),
        archived: row.get::<bool, _>("archived"),
    })
}

/// Why this viewer cannot start a run on this job right now, if they can't.
async fn run_blocker(
    state: &AppState,
    user: &AuthUser,
    job: &JobAccess,
    conclusion: Option<&str>,
    active_run: bool,
) -> ApiResult<Option<String>> {
    if conclusion != Some("failure") {
        return Ok(Some("Only failed jobs can be fixed.".into()));
    }
    if job.archived {
        return Ok(Some("The repository is archived.".into()));
    }
    if active_run {
        return Ok(Some("A fix is already running for this job.".into()));
    }
    if assert_installation_admin(state, user, job.installation_id)
        .await
        .is_err()
    {
        return Ok(Some(
            "Only administrators of this installation can start a fix.".into(),
        ));
    }
    let permissions =
        sqlx::query_scalar::<_, String>("SELECT permissions FROM installations WHERE id=?")
            .bind(job.installation_id)
            .fetch_optional(&state.database)
            .await?
            .and_then(|value| serde_json::from_str::<Value>(&value).ok())
            .unwrap_or_default();
    if !missing_permissions(&permissions).is_empty() {
        return Ok(Some(
            "The GridOps GitHub App needs Contents and Pull requests write access on this installation. See Settings → AI agent."
                .into(),
        ));
    }
    Ok(None)
}

/// The agent block of the job log view.
pub async fn job_agent_state(
    state: &AppState,
    user: &AuthUser,
    job_id: i64,
    conclusion: Option<&str>,
) -> ApiResult<Value> {
    let available = active_connection(&state.database).await?.is_some();
    let latest = sqlx::query(
        r#"SELECT id,status,outcome,pull_request_url,pull_request_number,created_at,completed_at
          FROM agent_runs WHERE job_id=? ORDER BY created_at DESC LIMIT 1"#,
    )
    .bind(job_id)
    .fetch_optional(&state.database)
    .await?;
    let active = latest
        .as_ref()
        .is_some_and(|row| matches!(row.get::<&str, _>("status"), "queued" | "running"));
    let reason = if available {
        let job = job_access(state, user, job_id).await?;
        run_blocker(state, user, &job, conclusion, active).await?
    } else {
        Some("Connect an AI provider in Settings → AI agent.".into())
    };
    Ok(json!({
        "available": available,
        "canRun": reason.is_none(),
        "reason": reason,
        "latestRun": latest.map(|row| json!({
            "id": row.get::<String, _>("id"),
            "status": row.get::<String, _>("status"),
            "outcome": row.get::<Option<String>, _>("outcome"),
            "pullRequestUrl": row.get::<Option<String>, _>("pull_request_url"),
            "pullRequestNumber": row.get::<Option<i64>, _>("pull_request_number"),
            "createdAt": iso(Some(row.get::<i64, _>("created_at"))),
            "completedAt": iso(row.get::<Option<i64>, _>("completed_at")),
        })),
    }))
}

pub async fn start_agent_run(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Path(job_id): Path<i64>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let Some(connection) = active_connection(&state.database).await? else {
        return Err(ApiError::BadRequest(
            "Connect an AI provider and choose a model in Settings → AI agent first.".into(),
        ));
    };
    let job = job_access(&state, &user, job_id).await?;
    let conclusion = if job.conclusion.as_deref() == Some("failure") {
        job.conclusion.clone()
    } else {
        // GridOps may not have synced the conclusion yet; ask GitHub.
        let token = control_token(&state, &user.id, job.installation_id).await?;
        state
            .github
            .get::<GitHubWorkflowJob>(
                &format!(
                    "/repos/{}/{}/actions/jobs/{job_id}",
                    job.owner, job.repository
                ),
                &token,
            )
            .await
            .ok()
            .and_then(|remote| remote.conclusion)
    };
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM agent_runs WHERE job_id=? AND status IN ('queued','running')",
    )
    .bind(job_id)
    .fetch_one(&state.database)
    .await?
        > 0;
    if let Some(reason) = run_blocker(&state, &user, &job, conclusion.as_deref(), active).await? {
        return Err(if active {
            ApiError::Conflict(reason)
        } else {
            ApiError::BadRequest(reason)
        });
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_millis();
    let inserted = sqlx::query(
        r#"INSERT INTO agent_runs
          (id,job_id,run_id,repository_id,trigger,status,stage,connection_id,provider,model,
           reasoning_effort,requested_by,created_at,updated_at)
          VALUES (?,?,?,?,'manual','queued','Waiting for the agent worker',?,?,?,?,?,?,?)"#,
    )
    .bind(&id)
    .bind(job_id)
    .bind(job.run_id)
    .bind(job.repository_id)
    .bind(&connection.id)
    .bind(&connection.provider)
    .bind(&connection.model)
    .bind(&connection.reasoning_effort)
    .bind(&user.id)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await;
    if let Err(error) = inserted {
        if error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            return Err(ApiError::Conflict(
                "A fix is already running for this job.".into(),
            ));
        }
        return Err(error.into());
    }
    audit(
        &state,
        &user,
        "fix_agent.run_started",
        "workflow_job",
        Some(&job_id.to_string()),
        json!({
            "agentRunId": id, "repository": format!("{}/{}", job.owner, job.repository),
            "branch": job.head_branch, "model": connection.model,
        }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(run_json(&state, &user, &id).await?),
    ))
}

pub async fn agent_run(
    State(state): State<AppState>,
    user: AuthUser,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(run_json(&state, &user, &run_id).await?))
}

pub async fn cancel_agent_run(
    State(state): State<AppState>,
    _same_origin: SameOrigin,
    user: AuthUser,
    Path(run_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let row = run_row(&state, &user, &run_id).await?;
    assert_installation_admin(&state, &user, row.get::<i64, _>("installation_id")).await?;
    let now = now_millis();
    let cancelled = sqlx::query(
        r#"UPDATE agent_runs SET status='cancelled',stage='Cancelled',completed_at=?,updated_at=?
          WHERE id=? AND status='queued'"#,
    )
    .bind(now)
    .bind(now)
    .bind(&run_id)
    .execute(&state.database)
    .await?
    .rows_affected();
    if cancelled == 0 {
        let requested = sqlx::query(
            r#"UPDATE agent_runs SET cancel_requested=1,stage='Cancelling',updated_at=?
              WHERE id=? AND status='running'"#,
        )
        .bind(now)
        .bind(&run_id)
        .execute(&state.database)
        .await?
        .rows_affected();
        if requested == 0 {
            return Err(ApiError::Conflict("This run has already finished.".into()));
        }
    }
    sqlx::query(
        "INSERT INTO agent_run_events (agent_run_id,kind,title,detail,created_at) VALUES (?,'status',?,NULL,?)",
    )
    .bind(&run_id)
    .bind(format!("Cancelled by {}", user.login))
    .bind(now)
    .execute(&state.database)
    .await?;
    audit(
        &state,
        &user,
        "fix_agent.run_cancelled",
        "agent_run",
        Some(&run_id),
        json!({}),
    )
    .await?;
    Ok(Json(run_json(&state, &user, &run_id).await?))
}

async fn run_row(state: &AppState, user: &AuthUser, run_id: &str) -> ApiResult<SqliteRow> {
    sqlx::query(
        r#"SELECT agent.*,repo.full_name,repo.installation_id,job.name AS job_name,
          requester.login AS requested_by_login
          FROM agent_runs agent
          JOIN repositories repo ON repo.id=agent.repository_id
          JOIN workflow_jobs job ON job.id=agent.job_id
          JOIN user_installations access ON access.installation_id=repo.installation_id
          LEFT JOIN users requester ON requester.id=agent.requested_by
          WHERE agent.id=? AND access.user_id=?"#,
    )
    .bind(run_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("That agent run does not exist or is not accessible.".into()))
}

async fn run_json(state: &AppState, user: &AuthUser, run_id: &str) -> ApiResult<Value> {
    let row = run_row(state, user, run_id).await?;
    let mut events = sqlx::query(
        r#"SELECT id,kind,title,detail,created_at FROM agent_run_events
          WHERE agent_run_id=? ORDER BY id DESC LIMIT ?"#,
    )
    .bind(run_id)
    .bind(MAX_EVENTS)
    .fetch_all(&state.database)
    .await?
    .iter()
    .map(|event| {
        json!({
            "id": event.get::<i64, _>("id"),
            "kind": event.get::<String, _>("kind"),
            "title": event.get::<String, _>("title"),
            "detail": event.get::<Option<String>, _>("detail"),
            "createdAt": iso(Some(event.get::<i64, _>("created_at"))),
        })
    })
    .collect::<Vec<_>>();
    events.reverse();
    let status = row.get::<String, _>("status");
    let active = matches!(status.as_str(), "queued" | "running");
    let can_cancel = active
        && row.get::<i64, _>("cancel_requested") == 0
        && assert_installation_admin(state, user, row.get::<i64, _>("installation_id"))
            .await
            .is_ok();
    let outcome = row.get::<Option<String>, _>("outcome");
    Ok(json!({
        "id": row.get::<String, _>("id"),
        "jobId": row.get::<i64, _>("job_id"),
        "runId": row.get::<i64, _>("run_id"),
        "repository": row.get::<String, _>("full_name"),
        "jobName": row.get::<String, _>("job_name"),
        "status": status,
        "stage": row.get::<Option<String>, _>("stage"),
        "title": row.get::<Option<String>, _>("title"),
        "summary": row.get::<Option<String>, _>("summary"),
        "diagnosis": if outcome.as_deref() == Some("diagnosis") {
            row.get::<Option<String>, _>("details")
        } else {
            None
        },
        "details": row.get::<Option<String>, _>("details"),
        "verification": row.get::<Option<String>, _>("verification"),
        "outcome": outcome,
        "pullRequestUrl": row.get::<Option<String>, _>("pull_request_url"),
        "pullRequestNumber": row.get::<Option<i64>, _>("pull_request_number"),
        "branch": row.get::<Option<String>, _>("branch"),
        "error": row.get::<Option<String>, _>("error"),
        "trigger": row.get::<String, _>("trigger"),
        "provider": row.get::<Option<String>, _>("provider"),
        "model": row.get::<Option<String>, _>("model"),
        "requestedBy": row.get::<Option<String>, _>("requested_by_login"),
        "turns": row.get::<i64, _>("turns"),
        "toolCalls": row.get::<i64, _>("tool_calls"),
        "createdAt": iso(Some(row.get::<i64, _>("created_at"))),
        "startedAt": iso(row.get::<Option<i64>, _>("started_at")),
        "completedAt": iso(row.get::<Option<i64>, _>("completed_at")),
        "canCancel": can_cancel,
        "events": events,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_connections_start_with_a_model_the_provider_offers() {
        let listed = vec![
            ModelOption::listed("claude-haiku-4-5-20251001", None),
            ModelOption::listed("claude-sonnet-5-5", None),
        ];
        let chosen = preferred_model(ProviderId::Anthropic, &listed);
        assert_eq!(
            chosen.map(|model| model.id).as_deref(),
            Some("claude-sonnet-5-5")
        );
        let unknown = vec![ModelOption::listed("custom-model", None)];
        let fallback = preferred_model(ProviderId::OpenAi, &unknown);
        assert_eq!(
            fallback.map(|model| model.id).as_deref(),
            Some("custom-model")
        );
        assert!(preferred_model(ProviderId::OpenAi, &[]).is_none());
        let subscription = preferred_model(
            ProviderId::ClaudeCode,
            &default_models(ProviderId::ClaudeCode),
        );
        assert_eq!(
            subscription.map(|model| model.id).as_deref(),
            Some("claude-opus-5-5")
        );
    }
}
