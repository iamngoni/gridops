use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use hmac::{Hmac, Mac as _};
use serde_json::{Map, Value, json};
use sha2::Sha256;
use sqlx::{Row as _, SqlitePool};

use crate::{
    auth::{AuthUser, assert_installation_admin, assert_same_origin, audit, require_system_admin},
    error::{ApiError, ApiResult},
    state::AppState,
};
use gridops_core::{
    assigned_queued_jobs, associate_runner_with_job, now_millis, runner_supports_system_label,
    scale_up_target,
};

const MAX_WEBHOOK_BYTES: usize = 25 * 1024 * 1024;

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    if body.len() > MAX_WEBHOOK_BYTES {
        return Err(ApiError::PayloadTooLarge(
            "Webhook payload exceeds 25 MB.".into(),
        ));
    }
    let delivery_id = required_header(&headers, "x-github-delivery")?;
    let event = required_header(&headers, "x-github-event")?;
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok());
    let signature_valid = verify_signature(&state, &body, signature).await?;
    let payload: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("Webhook payload is not valid JSON.".into()))?;
    let object = payload
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("Webhook payload must be a JSON object.".into()))?;
    let action = string(object, "action");
    let installation_id = nested_i64(object, "installation", "id");
    let repository_id = nested_i64(object, "repository", "id");
    let hook_id = nested_i64(object, "hook", "id");
    let now = now_millis();

    let result = sqlx::query(
        r#"INSERT INTO webhook_deliveries (
          id,event,action,hook_id,installation_id,repository_id,signature_valid,status,payload,received_at
        ) VALUES (?,?,?,?,?,?,?,'received',?,?) ON CONFLICT(id) DO NOTHING"#,
    )
    .bind(&delivery_id)
    .bind(&event)
    .bind(action)
    .bind(hook_id)
    .bind(installation_id)
    .bind(repository_id)
    .bind(signature_valid)
    .bind(payload.to_string())
    .bind(now)
    .execute(&state.database)
    .await?;
    if result.rows_affected() == 0 {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "accepted": true, "duplicate": true })),
        ));
    }
    if !signature_valid {
        sqlx::query("UPDATE webhook_deliveries SET status='rejected',error=? WHERE id=?")
            .bind("Invalid or unavailable webhook signature.")
            .bind(&delivery_id)
            .execute(&state.database)
            .await?;
        return Err(ApiError::Unauthorized);
    }

    match process(&state, &event, action, object).await {
        Ok(()) => {
            sqlx::query(
                "UPDATE webhook_deliveries SET status='processed',processed_at=? WHERE id=?",
            )
            .bind(now_millis())
            .bind(&delivery_id)
            .execute(&state.database)
            .await?;
            Ok((StatusCode::ACCEPTED, Json(json!({ "accepted": true }))))
        }
        Err(error) => {
            let message = error.to_string().chars().take(2_000).collect::<String>();
            sqlx::query(
                "UPDATE webhook_deliveries SET status='failed',error=?,processed_at=? WHERE id=?",
            )
            .bind(&message)
            .bind(now_millis())
            .bind(&delivery_id)
            .execute(&state.database)
            .await?;
            tracing::error!(delivery_id, event, error = ?error, "webhook processing failed");
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "accepted": true, "processing": "failed" })),
            ))
        }
    }
}

pub async fn retry(
    State(state): State<AppState>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    let delivery = sqlx::query(
        r#"SELECT event,action,installation_id,signature_valid,payload FROM webhook_deliveries wd
           WHERE wd.id=? AND (wd.installation_id IS NULL OR EXISTS (
             SELECT 1 FROM user_installations ui
             WHERE ui.installation_id=wd.installation_id AND ui.user_id=?
           ))"#,
    )
    .bind(&delivery_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Webhook delivery does not exist or is not accessible.".into())
    })?;
    if !delivery.get::<bool, _>("signature_valid") {
        return Err(ApiError::BadRequest(
            "Only verified webhook payloads can be retried.".into(),
        ));
    }
    match delivery.try_get::<Option<i64>, _>("installation_id")? {
        Some(installation_id) => assert_installation_admin(&state, &user, installation_id).await?,
        None => require_system_admin(&user)?,
    }
    let payload = delivery
        .try_get::<Option<String>, _>("payload")?
        .ok_or_else(|| ApiError::BadRequest("Webhook payload is unavailable.".into()))?;
    let value: Value = serde_json::from_str(&payload)
        .map_err(|_| ApiError::BadRequest("Stored webhook payload is invalid.".into()))?;
    let object = value
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("Stored webhook payload is invalid.".into()))?;
    let event = delivery.get::<String, _>("event");
    let action = delivery.try_get::<Option<String>, _>("action")?;

    match process(&state, &event, action.as_deref(), object).await {
        Ok(()) => {
            sqlx::query("UPDATE webhook_deliveries SET status='processed',error=NULL,processed_at=? WHERE id=?")
                .bind(now_millis())
                .bind(&delivery_id)
                .execute(&state.database)
                .await?;
        }
        Err(error) => {
            let message = error.to_string().chars().take(2_000).collect::<String>();
            sqlx::query(
                "UPDATE webhook_deliveries SET status='failed',error=?,processed_at=? WHERE id=?",
            )
            .bind(&message)
            .bind(now_millis())
            .bind(&delivery_id)
            .execute(&state.database)
            .await?;
            return Err(error);
        }
    }
    audit(
        &state,
        &user,
        "webhook.retried",
        "webhook_delivery",
        Some(&delivery_id),
        json!({}),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

async fn process(
    state: &AppState,
    event: &str,
    action: Option<&str>,
    payload: &Map<String, Value>,
) -> ApiResult<()> {
    match event {
        "ping" => Ok(()),
        "installation" => process_installation(state, action, payload).await,
        "installation_repositories" => process_installation_repositories(state, payload).await,
        "workflow_run" => process_workflow_run(state, payload).await,
        "workflow_job" => process_workflow_job(&state.database, action, payload).await,
        "github_app_authorization" if action == Some("revoked") => {
            if let Some(sender_id) = nested_i64(payload, "sender", "id") {
                sqlx::query("DELETE FROM users WHERE github_id=?")
                    .bind(sender_id)
                    .execute(&state.database)
                    .await?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn process_installation(
    state: &AppState,
    action: Option<&str>,
    payload: &Map<String, Value>,
) -> ApiResult<()> {
    let Some(installation) = nested_object(payload, "installation") else {
        return Ok(());
    };
    let Some(id) = i64_value(installation, "id") else {
        return Ok(());
    };
    if action == Some("deleted") {
        let now = now_millis();
        sqlx::query("UPDATE installations SET suspended_at=COALESCE(suspended_at,?),updated_at=? WHERE id=?")
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(&state.database)
            .await?;
        sqlx::query("UPDATE runner_pools SET paused=1,state='draining',updated_at=? WHERE installation_id=?")
            .bind(now)
            .bind(id)
            .execute(&state.database)
            .await?;
        return Ok(());
    }
    let Some(account) = nested_object(installation, "account") else {
        return Ok(());
    };
    let now = now_millis();
    sqlx::query(
        r#"INSERT INTO installations (
          id,account_id,account_login,account_type,account_avatar_url,target_type,
          repository_selection,permissions,events,suspended_at,last_synced_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT(id) DO UPDATE SET account_login=excluded.account_login,
          account_type=excluded.account_type,account_avatar_url=excluded.account_avatar_url,
          repository_selection=excluded.repository_selection,permissions=excluded.permissions,
          events=excluded.events,suspended_at=excluded.suspended_at,
          last_synced_at=excluded.last_synced_at,updated_at=excluded.updated_at"#,
    )
    .bind(id)
    .bind(i64_value(account, "id").unwrap_or_default())
    .bind(string(account, "login").unwrap_or("unknown"))
    .bind(string(account, "type").unwrap_or("User"))
    .bind(string(account, "avatar_url"))
    .bind(string(installation, "target_type").unwrap_or("User"))
    .bind(string(installation, "repository_selection").unwrap_or("selected"))
    .bind(
        installation
            .get("permissions")
            .unwrap_or(&Value::Null)
            .to_string(),
    )
    .bind(
        installation
            .get("events")
            .unwrap_or(&Value::Null)
            .to_string(),
    )
    .bind(date_value(installation, "suspended_at"))
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await?;
    if date_value(installation, "suspended_at").is_some() {
        sqlx::query("UPDATE runner_pools SET paused=1,state='draining',updated_at=? WHERE installation_id=?")
            .bind(now)
            .bind(id)
            .execute(&state.database)
            .await?;
    }
    Ok(())
}

async fn process_installation_repositories(
    state: &AppState,
    payload: &Map<String, Value>,
) -> ApiResult<()> {
    let Some(installation_id) = nested_i64(payload, "installation", "id") else {
        return Ok(());
    };
    let now = now_millis();
    for value in array(payload, "repositories_added") {
        let Some(repository) = value.as_object() else {
            continue;
        };
        let (Some(id), Some(full_name)) =
            (i64_value(repository, "id"), string(repository, "full_name"))
        else {
            continue;
        };
        let (owner, name) = full_name.split_once('/').unwrap_or(("unknown", full_name));
        sqlx::query(
            r#"INSERT INTO repositories (
              id,installation_id,owner,name,full_name,private,archived,default_branch,
              html_url,last_synced_at,created_at,updated_at
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT(id) DO UPDATE SET installation_id=excluded.installation_id,
              owner=excluded.owner,name=excluded.name,full_name=excluded.full_name,
              private=excluded.private,archived=0,default_branch=excluded.default_branch,
              html_url=excluded.html_url,
              last_synced_at=excluded.last_synced_at,updated_at=excluded.updated_at"#,
        )
        .bind(id)
        .bind(installation_id)
        .bind(owner)
        .bind(name)
        .bind(full_name)
        .bind(bool_value(repository, "private"))
        .bind(false)
        .bind(string(repository, "default_branch").unwrap_or("master"))
        .bind(
            string(repository, "html_url")
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("https://github.com/{full_name}")),
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&state.database)
        .await?;
    }
    for value in array(payload, "repositories_removed") {
        if let Some(id) = value.as_object().and_then(|object| i64_value(object, "id")) {
            sqlx::query("UPDATE repositories SET archived=1,updated_at=? WHERE id=?")
                .bind(now)
                .bind(id)
                .execute(&state.database)
                .await?;
            sqlx::query("UPDATE runner_pools SET paused=1,state='draining',updated_at=? WHERE repository_id=?")
                .bind(now)
                .bind(id)
                .execute(&state.database)
                .await?;
        }
    }
    Ok(())
}

async fn process_workflow_run(state: &AppState, payload: &Map<String, Value>) -> ApiResult<()> {
    let Some(run) = nested_object(payload, "workflow_run") else {
        return Ok(());
    };
    let (Some(id), Some(repository_id)) = (
        i64_value(run, "id"),
        nested_i64(payload, "repository", "id"),
    ) else {
        return Ok(());
    };
    let exists = sqlx::query("SELECT 1 FROM repositories WHERE id=?")
        .bind(repository_id)
        .fetch_optional(&state.database)
        .await?;
    if exists.is_none() {
        return Err(ApiError::Conflict(format!(
            "Repository {repository_id} is not synced yet."
        )));
    }
    let now = now_millis();
    let status = string(run, "status").unwrap_or("queued");
    let updated_at = date_value(run, "updated_at").unwrap_or(now);
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
    .bind(id)
    .bind(repository_id)
    .bind(i64_value(run, "workflow_id"))
    .bind(
        string(run, "name")
            .or_else(|| string(run, "display_title"))
            .unwrap_or("Workflow"),
    )
    .bind(i64_value(run, "run_number").unwrap_or_default())
    .bind(i64_value(run, "run_attempt").unwrap_or(1))
    .bind(string(run, "event").unwrap_or("unknown"))
    .bind(status)
    .bind(string(run, "conclusion"))
    .bind(string(run, "head_branch"))
    .bind(string(run, "head_sha").unwrap_or("unknown"))
    .bind(nested_string(run, "actor", "login"))
    .bind(string(run, "html_url").unwrap_or(""))
    .bind(date_value(run, "run_started_at"))
    .bind((status == "completed").then_some(updated_at))
    .bind(date_value(run, "created_at").unwrap_or(now))
    .bind(updated_at)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await?;
    Ok(())
}

async fn process_workflow_job(
    database: &SqlitePool,
    action: Option<&str>,
    payload: &Map<String, Value>,
) -> ApiResult<()> {
    let Some(job) = nested_object(payload, "workflow_job") else {
        return Ok(());
    };
    let (Some(id), Some(run_id)) = (i64_value(job, "id"), i64_value(job, "run_id")) else {
        return Ok(());
    };
    let now = now_millis();
    if sqlx::query("SELECT 1 FROM workflow_runs WHERE id=?")
        .bind(run_id)
        .fetch_optional(database)
        .await?
        .is_none()
    {
        insert_placeholder_run(database, payload, job, run_id, now).await?;
    }
    let status = string(job, "status").or(action).unwrap_or("queued");
    sqlx::query(
        r#"INSERT INTO workflow_jobs (
          id,run_id,name,status,conclusion,runner_id,runner_name,runner_group_id,
          runner_group_name,labels,html_url,started_at,completed_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT(id) DO UPDATE SET status=excluded.status,conclusion=excluded.conclusion,
          runner_id=excluded.runner_id,runner_name=excluded.runner_name,
          runner_group_id=excluded.runner_group_id,runner_group_name=excluded.runner_group_name,
          labels=excluded.labels,started_at=excluded.started_at,
          completed_at=excluded.completed_at,updated_at=excluded.updated_at"#,
    )
    .bind(id)
    .bind(run_id)
    .bind(string(job, "name").unwrap_or("Job"))
    .bind(status)
    .bind(string(job, "conclusion"))
    .bind(i64_value(job, "runner_id"))
    .bind(string(job, "runner_name"))
    .bind(i64_value(job, "runner_group_id"))
    .bind(string(job, "runner_group_name"))
    .bind(job.get("labels").unwrap_or(&Value::Null).to_string())
    .bind(string(job, "html_url").unwrap_or(""))
    .bind(date_value(job, "started_at"))
    .bind(date_value(job, "completed_at"))
    .bind(now)
    .bind(now)
    .execute(database)
    .await?;
    let runner = associate_runner_with_job(
        database,
        id,
        status,
        i64_value(job, "runner_id"),
        string(job, "runner_name"),
        now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO runner_events (id,runner_id,pool_id,level,event,message,metadata,created_at) VALUES (?,?,?,'info',?,?,?,?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(runner.as_ref().map(|runner| &runner.0))
    .bind(runner.as_ref().map(|runner| &runner.1))
    .bind(format!("Workflow job {}", action.unwrap_or(status)))
    .bind(format!(
        "{} · run {run_id}",
        string(job, "name").unwrap_or("Job")
    ))
    .bind(json!({ "jobId": id, "runId": run_id, "labels": job.get("labels") }).to_string())
    .bind(now)
    .execute(database)
    .await?;
    if status == "queued" {
        scale_for_queued_job(database, payload, job, now).await?;
    }
    Ok(())
}

async fn insert_placeholder_run(
    database: &SqlitePool,
    payload: &Map<String, Value>,
    job: &Map<String, Value>,
    run_id: i64,
    now: i64,
) -> ApiResult<()> {
    let repository_id = nested_i64(payload, "repository", "id").ok_or_else(|| {
        ApiError::Conflict(format!(
            "Workflow run {run_id} arrived before its repository was synced."
        ))
    })?;
    if sqlx::query("SELECT 1 FROM repositories WHERE id=?")
        .bind(repository_id)
        .fetch_optional(database)
        .await?
        .is_none()
    {
        return Err(ApiError::Conflict(format!(
            "Repository {repository_id} is not synced yet."
        )));
    }
    let status = string(job, "status").unwrap_or("queued");
    let job_html_url = string(job, "html_url").unwrap_or("");
    let run_html_url = job_html_url
        .split_once("/job/")
        .map_or(job_html_url, |(url, _)| url);
    let created_at = date_value(job, "created_at").unwrap_or(now);
    sqlx::query(
        r#"INSERT INTO workflow_runs (
          id,repository_id,workflow_name,run_number,run_attempt,event,status,conclusion,
          head_branch,head_sha,actor_login,html_url,started_at,completed_at,
          github_created_at,github_updated_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING"#,
    )
    .bind(run_id)
    .bind(repository_id)
    .bind(string(job, "workflow_name").unwrap_or("Workflow"))
    .bind(i64_value(job, "run_number").unwrap_or_default())
    .bind(i64_value(job, "run_attempt").unwrap_or(1))
    .bind(string(job, "event").unwrap_or("unknown"))
    .bind(status)
    .bind(string(job, "conclusion"))
    .bind(string(job, "head_branch"))
    .bind(string(job, "head_sha").unwrap_or("unknown"))
    .bind(nested_string(payload, "sender", "login"))
    .bind(run_html_url)
    .bind(date_value(job, "started_at"))
    .bind(date_value(job, "completed_at"))
    .bind(created_at)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(database)
    .await?;
    Ok(())
}

async fn scale_for_queued_job(
    database: &SqlitePool,
    payload: &Map<String, Value>,
    job: &Map<String, Value>,
    now: i64,
) -> ApiResult<()> {
    let Some(repository_id) = nested_i64(payload, "repository", "id") else {
        return Ok(());
    };
    let requested_labels = array(job, "labels")
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let custom_labels = requested_labels
        .iter()
        .filter(|label| !runner_supports_system_label(label))
        .collect::<Vec<_>>();
    let candidates = sqlx::query(
        r#"SELECT p.id,p.labels,p.desired_count,p.max_count,p.queue_scale_factor,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1
            AND r.status IN ('online','idle','busy') THEN 1 END) AS busy_count
        FROM runner_pools p JOIN repositories event_repo ON event_repo.id=?
        LEFT JOIN runners r ON r.pool_id=p.id
        WHERE p.autoscaling_enabled=1 AND p.paused=0 AND (
          p.repository_id=event_repo.id OR
          (p.scope='organization' AND p.installation_id=event_repo.installation_id)
        ) GROUP BY p.id
        ORDER BY CASE WHEN p.repository_id=event_repo.id THEN 0 ELSE 1 END,p.created_at,p.id"#,
    )
    .bind(repository_id)
    .fetch_all(database)
    .await?;
    for candidate in candidates {
        let labels: Vec<String> =
            serde_json::from_str(candidate.get::<&str, _>("labels")).unwrap_or_default();
        if !custom_labels.iter().all(|requested| {
            labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(requested))
        }) {
            continue;
        }
        let pool_id = candidate.get::<String, _>("id");
        let desired = candidate.get::<i64, _>("desired_count");
        let maximum = candidate.get::<i64, _>("max_count");
        let busy = candidate.get::<i64, _>("busy_count");
        let queued = assigned_queued_jobs(database, &pool_id).await?;
        let factor = candidate.get::<i64, _>("queue_scale_factor");
        let target = scale_up_target(desired, busy, queued, factor, maximum);
        if target <= desired {
            return Ok(());
        }
        sqlx::query(
            "UPDATE runner_pools SET desired_count=?,state='scaling',updated_at=? WHERE id=?",
        )
        .bind(target)
        .bind(now)
        .bind(&pool_id)
        .execute(database)
        .await?;
        sqlx::query(
            r#"INSERT INTO runner_events (id,pool_id,level,event,message,metadata,created_at)
               VALUES (?,?,'info','Autoscale requested',?,?,?)"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&pool_id)
        .bind(format!("Queued job raised desired capacity from {desired} to {target}"))
        .bind(json!({ "jobId": i64_value(job, "id"), "labels": requested_labels, "desiredCount": target }).to_string())
        .bind(now)
        .execute(database)
        .await?;
        sqlx::query(
            r#"INSERT INTO audit_events (id,actor_label,action,target_type,target_id,metadata,created_at)
               VALUES (?,'system','runner_pool.autoscaled','runner_pool',?,?,?)"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&pool_id)
        .bind(json!({ "jobId": i64_value(job, "id"), "desiredCount": target }).to_string())
        .bind(now)
        .execute(database)
        .await?;
        return Ok(());
    }
    Ok(())
}

async fn verify_signature(
    state: &AppState,
    body: &[u8],
    signature: Option<&str>,
) -> ApiResult<bool> {
    let (Some(secret), Some(signature)) = (
        state
            .github_webhook_secret()
            .await
            .map_err(ApiError::Internal)?,
        signature,
    ) else {
        return Ok(false);
    };
    Ok(verify_signature_with_secret(body, signature, &secret))
}

fn verify_signature_with_secret(body: &[u8], signature: &str, secret: &str) -> bool {
    let Some(hex_signature) = signature.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(supplied) = hex::decode(hex_signature) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&supplied).is_ok()
}

fn required_header(headers: &HeaderMap, name: &'static str) -> ApiResult<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ApiError::BadRequest(format!("Missing {name} header.")))
}

fn nested_object<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    object.get(key)?.as_object()
}

fn nested_i64(object: &Map<String, Value>, key: &str, nested_key: &str) -> Option<i64> {
    nested_object(object, key).and_then(|value| i64_value(value, nested_key))
}

fn nested_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    nested_key: &str,
) -> Option<&'a str> {
    nested_object(object, key).and_then(|value| string(value, nested_key))
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

fn i64_value(object: &Map<String, Value>, key: &str) -> Option<i64> {
    object.get(key)?.as_i64()
}

fn bool_value(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn date_value(object: &Map<String, Value>, key: &str) -> Option<i64> {
    string(object, key)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
}

fn array<'a>(object: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridops_core::connect_database_path;
    use std::fs;

    #[test]
    fn payload_helpers_reject_wrong_types() -> Result<(), &'static str> {
        let value = json!({ "number": 42, "text": "yes", "array": ["one"] });
        let object = value.as_object().ok_or("test payload was not an object")?;
        assert_eq!(i64_value(object, "number"), Some(42));
        assert_eq!(string(object, "text"), Some("yes"));
        assert_eq!(array(object, "array").len(), 1);
        assert_eq!(string(object, "number"), None);
        Ok(())
    }

    #[test]
    fn webhook_signatures_are_authenticated() -> Result<(), &'static str> {
        let body = br#"{"zen":"Keep it logically awesome."}"#;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(b"test-secret").map_err(|_| "invalid test key")?;
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature_with_secret(
            body,
            &signature,
            "test-secret"
        ));
        assert!(!verify_signature_with_secret(
            b"tampered",
            &signature,
            "test-secret"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn workflow_jobs_track_runners_and_drive_autoscaling() -> anyhow::Result<()> {
        let directory =
            std::env::temp_dir().join(format!("gridops-webhook-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("gridops.sqlite");
        let database = connect_database_path(&path).await?;
        sqlx::raw_sql(
            r#"
            INSERT INTO installations (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
              VALUES (1,1,'iamngoni','User','User','all',1,1);
            INSERT INTO repositories (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
              VALUES (10,1,'iamngoni','gridops','iamngoni/gridops',0,'master','https://github.com/iamngoni/gridops',1,1,1);
            INSERT INTO runner_pools (id,installation_id,repository_id,name,scope,labels,image,desired_count,max_count,queue_scale_factor,created_at,updated_at)
              VALUES ('pool-1',1,10,'pool-a','repository','["pool-a"]','runner:latest',1,10,2,1,1);
            INSERT INTO workflow_runs (id,repository_id,workflow_name,run_number,event,status,head_sha,html_url,github_created_at,github_updated_at,created_at,updated_at)
              VALUES (20,10,'CI',1,'push','in_progress','abc123','https://github.com/iamngoni/gridops/actions/runs/20',1,1,1,1);
            INSERT INTO runners (id,pool_id,name,status,created_at,updated_at)
              VALUES ('runner-1','pool-1','pool-a-12345678','online',1,1);
            "#,
        )
        .execute(&database)
        .await?;

        let in_progress = json!({
            "repository": { "id": 10 },
            "workflow_job": {
                "id": 30, "run_id": 20, "name": "build", "status": "in_progress",
                "runner_id": 40, "runner_name": "pool-a-12345678", "runner_group_id": 1,
                "runner_group_name": "Default", "labels": ["self-hosted", "pool-a"],
                "html_url": "https://github.com/iamngoni/gridops/actions/runs/20/job/30",
                "started_at": "2026-07-21T00:00:00Z"
            }
        });
        let in_progress = in_progress
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("test payload is not an object"))?;
        process_workflow_job(&database, Some("in_progress"), in_progress).await?;

        let runner = sqlx::query(
            "SELECT busy,current_job_id,last_job_id,github_runner_id FROM runners WHERE id='runner-1'",
        )
        .fetch_one(&database)
        .await?;
        assert!(runner.get::<bool, _>("busy"));
        assert_eq!(runner.get::<i64, _>("current_job_id"), 30);
        assert_eq!(runner.get::<i64, _>("last_job_id"), 30);
        assert_eq!(runner.get::<i64, _>("github_runner_id"), 40);

        let completed = json!({
            "repository": { "id": 10 },
            "workflow_job": {
                "id": 30, "run_id": 20, "name": "build", "status": "completed",
                "conclusion": "success", "runner_id": 40, "runner_name": "pool-a-12345678",
                "runner_group_id": 1, "runner_group_name": "Default",
                "labels": ["self-hosted", "pool-a"],
                "html_url": "https://github.com/iamngoni/gridops/actions/runs/20/job/30",
                "started_at": "2026-07-21T00:00:00Z", "completed_at": "2026-07-21T00:01:00Z"
            }
        });
        let completed = completed
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("test payload is not an object"))?;
        process_workflow_job(&database, Some("completed"), completed).await?;
        let runner =
            sqlx::query("SELECT busy,current_job_id,last_job_id FROM runners WHERE id='runner-1'")
                .fetch_one(&database)
                .await?;
        assert!(!runner.get::<bool, _>("busy"));
        assert_eq!(runner.try_get::<Option<i64>, _>("current_job_id")?, None);
        assert_eq!(runner.get::<i64, _>("last_job_id"), 30);

        let queued = json!({
            "repository": { "id": 10 },
            "workflow_job": {
                "id": 31, "run_id": 20, "name": "test", "status": "queued",
                "labels": ["self-hosted", "pool-a"],
                "html_url": "https://github.com/iamngoni/gridops/actions/runs/20/job/31"
            }
        });
        let queued = queued
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("test payload is not an object"))?;
        process_workflow_job(&database, Some("queued"), queued).await?;
        process_workflow_job(&database, Some("queued"), queued).await?;
        let desired = sqlx::query_scalar::<_, i64>(
            "SELECT desired_count FROM runner_pools WHERE id='pool-1'",
        )
        .fetch_one(&database)
        .await?;
        let associated_events = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM runner_events WHERE runner_id='runner-1' AND pool_id='pool-1'",
        )
        .fetch_one(&database)
        .await?;
        assert_eq!(desired, 2);
        assert_eq!(associated_events, 2);

        let out_of_order = json!({
            "repository": { "id": 10 }, "sender": { "login": "iamngoni" },
            "workflow_job": {
                "id": 32, "run_id": 21, "run_attempt": 1, "workflow_name": "Release",
                "name": "publish", "status": "queued", "head_branch": "master", "head_sha": "def456",
                "labels": ["self-hosted", "unmatched"],
                "html_url": "https://github.com/iamngoni/gridops/actions/runs/21/job/32",
                "created_at": "2026-07-21T00:02:00Z"
            }
        });
        let out_of_order = out_of_order
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("test payload is not an object"))?;
        process_workflow_job(&database, Some("queued"), out_of_order).await?;
        let placeholder = sqlx::query(
            "SELECT workflow_name,head_branch,head_sha,html_url FROM workflow_runs WHERE id=21",
        )
        .fetch_one(&database)
        .await?;
        assert_eq!(placeholder.get::<String, _>("workflow_name"), "Release");
        assert_eq!(placeholder.get::<String, _>("head_branch"), "master");
        assert_eq!(placeholder.get::<String, _>("head_sha"), "def456");
        assert_eq!(
            placeholder.get::<String, _>("html_url"),
            "https://github.com/iamngoni/gridops/actions/runs/21"
        );

        database.close().await;
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
