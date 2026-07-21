use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use gridops_core::{crypto::hash_token, now_millis};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row as _;

use crate::{
    auth::{AuthUser, assert_same_origin, random_token, require_system_admin},
    error::{ApiError, ApiResult},
    state::{
        AppState, GITHUB_APP_ID, GITHUB_APP_PRIVATE_KEY, GITHUB_APP_SLUG, GITHUB_CLIENT_ID,
        GITHUB_CLIENT_SECRET, GITHUB_WEBHOOK_SECRET,
    },
};

const MANIFEST_STATE_TTL_MILLIS: i64 = 60 * 60 * 1_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRequest {
    owner_type: Option<String>,
    organization: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
pub struct ManifestCallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct ManifestConversion {
    id: i64,
    slug: String,
    client_id: String,
    client_secret: String,
    pem: String,
    webhook_secret: String,
}

pub async fn create_manifest(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<ManifestRequest>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    require_system_admin(&user)?;
    let owner_type = input.owner_type.as_deref().unwrap_or("user");
    let action = match owner_type {
        "user" => "https://github.com/settings/apps/new".to_owned(),
        "organization" => {
            let organization = input
                .organization
                .as_deref()
                .filter(|value| valid_slug(value))
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "A valid organization login is required for organization-owned apps."
                            .into(),
                    )
                })?;
            format!("https://github.com/organizations/{organization}/settings/apps/new")
        }
        _ => {
            return Err(ApiError::BadRequest(
                "GitHub App owner type must be user or organization.".into(),
            ));
        }
    };
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 100)
        .unwrap_or("GridOps Self-Hosted");
    let manifest_state = random_token(32);
    let now = now_millis();
    sqlx::query(
        "INSERT INTO github_app_manifest_states (id,state_hash,user_id,expires_at,created_at) VALUES (?,?,?,?,?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(hash_token(&manifest_state))
    .bind(&user.id)
    .bind(now + MANIFEST_STATE_TTL_MILLIS)
    .bind(now)
    .execute(&state.database)
    .await?;

    let (manifest, webhook_active) =
        build_manifest(name, state.config.base_url()).map_err(ApiError::Internal)?;

    Ok(Json(json!({
        "action": action,
        "state": manifest_state,
        "manifest": manifest.to_string(),
        "webhookActive": webhook_active,
    })))
}

pub async fn manifest_callback(
    State(state): State<AppState>,
    Query(query): Query<ManifestCallbackQuery>,
) -> ApiResult<Response> {
    let (Some(code), Some(manifest_state)) = (query.code, query.state) else {
        return Ok(settings_error_redirect(
            &state,
            "GitHub did not return a valid App manifest code.",
        ));
    };
    let record = sqlx::query(
        r#"SELECT ms.id,ms.user_id,ms.expires_at,u.login,u.role FROM github_app_manifest_states ms
           JOIN users u ON u.id=ms.user_id WHERE ms.state_hash=?"#,
    )
    .bind(hash_token(&manifest_state))
    .fetch_optional(&state.database)
    .await?;
    let Some(record) = record else {
        return Ok(settings_error_redirect(
            &state,
            "The GitHub App setup request is invalid.",
        ));
    };
    sqlx::query("DELETE FROM github_app_manifest_states WHERE id=?")
        .bind(record.get::<String, _>("id"))
        .execute(&state.database)
        .await?;
    if record.get::<i64, _>("expires_at") <= now_millis() {
        return Ok(settings_error_redirect(
            &state,
            "The GitHub App setup request expired.",
        ));
    }
    if record.get::<String, _>("role") != "admin" {
        return Ok(settings_error_redirect(
            &state,
            "Only a GridOps administrator can configure the GitHub App.",
        ));
    }

    let response = state
        .http
        .post(format!(
            "https://api.github.com/app-manifests/{code}/conversions"
        ))
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let details = response.text().await.unwrap_or_default();
        tracing::warn!(status = %status, details = %details.chars().take(300).collect::<String>(), "GitHub App manifest conversion failed");
        return Ok(settings_error_redirect(
            &state,
            "GitHub could not complete the App setup.",
        ));
    }
    let app = response
        .json::<ManifestConversion>()
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    if app.id <= 0
        || app.slug.is_empty()
        || app.client_id.is_empty()
        || app.client_secret.is_empty()
        || app.pem.is_empty()
        || app.webhook_secret.is_empty()
    {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "GitHub returned an incomplete App manifest conversion"
        )));
    }

    let user_id = record.get::<String, _>("user_id");
    let actor = record.get::<String, _>("login");
    let app_id = app.id.to_string();
    let values = [
        (GITHUB_CLIENT_ID, app.client_id.as_str()),
        (GITHUB_CLIENT_SECRET, app.client_secret.as_str()),
        (GITHUB_APP_ID, app_id.as_str()),
        (GITHUB_APP_PRIVATE_KEY, app.pem.as_str()),
        (GITHUB_APP_SLUG, app.slug.as_str()),
        (GITHUB_WEBHOOK_SECRET, app.webhook_secret.as_str()),
    ];
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    for (key, value) in values {
        let sealed = state.vault.seal(value).map_err(ApiError::Internal)?;
        sqlx::query(
            r#"INSERT INTO runtime_secrets (key,value,updated_by,updated_at) VALUES (?,?,?,?)
               ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_by=excluded.updated_by,updated_at=excluded.updated_at"#,
        )
        .bind(key)
        .bind(sealed)
        .bind(&user_id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"INSERT INTO audit_events (id,actor_user_id,actor_label,action,target_type,target_id,metadata,created_at)
           VALUES (?,?,?,?,?,?,?,?)"#,
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&user_id)
    .bind(actor)
    .bind("github_app.configured")
    .bind("github_app")
    .bind(app.id.to_string())
    .bind(json!({ "slug": app.slug }).to_string())
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    let location = state
        .config
        .base_url()
        .join("/settings?appCreated=1")
        .map_err(|error| ApiError::Internal(error.into()))?;
    Ok(Redirect::to(location.as_str()).into_response())
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 39
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn build_manifest(name: &str, base_url: &url::Url) -> anyhow::Result<(Value, bool)> {
    let callback_url = base_url.join("/auth/github/callback")?;
    let manifest_callback_url = base_url.join("/auth/github-app/manifest/callback")?;
    let setup_url = base_url.join("/settings?appCreated=1")?;
    let webhook_url = base_url.join("/api/webhooks/github")?;
    let webhook_active = webhook_url.scheme() == "https"
        && !matches!(
            webhook_url.host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        );
    Ok((
        json!({
            "name": name,
            "url": base_url.as_str(),
            "description": "Self-hosted GitHub Actions runner control plane",
            "hook_attributes": { "url": webhook_url.as_str(), "active": webhook_active },
            "redirect_url": manifest_callback_url.as_str(),
            "callback_urls": [callback_url.as_str()],
            "setup_url": setup_url.as_str(),
            "public": false,
            "request_oauth_on_install": true,
            "setup_on_update": true,
            "default_permissions": {
                "actions": "write",
                "administration": "write",
                "metadata": "read",
                "members": "read",
                "organization_self_hosted_runners": "write"
            },
            "default_events": [
                "github_app_authorization",
                "installation",
                "installation_repositories",
                "workflow_job",
                "workflow_run"
            ]
        }),
        webhook_active,
    ))
}

fn settings_error_redirect(state: &AppState, message: &str) -> Response {
    let mut url = state.config.base_url().clone();
    url.set_path("/settings");
    url.query_pairs_mut().append_pair("appError", message);
    (StatusCode::FOUND, [(header::LOCATION, url.to_string())]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_github_organization_logins() {
        assert!(valid_slug("iamngoni-labs"));
        assert!(!valid_slug("-iamngoni"));
        assert!(!valid_slug("iam_ngoni"));
        assert!(!valid_slug(""));
    }

    #[test]
    fn manifest_contains_runner_permissions_and_safe_local_webhook_defaults() -> anyhow::Result<()>
    {
        let base_url = url::Url::parse("http://localhost:3100")?;
        let (manifest, webhook_active) = build_manifest("GridOps Test", &base_url)?;
        assert!(!webhook_active);
        assert_eq!(
            manifest["default_permissions"]["organization_self_hosted_runners"],
            "write"
        );
        assert_eq!(manifest["default_permissions"]["actions"], "write");
        assert_eq!(manifest["default_permissions"]["members"], "read");
        assert_eq!(
            manifest["redirect_url"],
            "http://localhost:3100/auth/github-app/manifest/callback"
        );
        assert_eq!(manifest["hook_attributes"]["active"], false);
        Ok(())
    }

    #[test]
    fn manifest_enables_webhooks_for_public_https_origins() -> anyhow::Result<()> {
        let base_url = url::Url::parse("https://gridops.example.com")?;
        let (manifest, webhook_active) = build_manifest("GridOps Test", &base_url)?;
        assert!(webhook_active);
        assert_eq!(manifest["hook_attributes"]["active"], true);
        assert_eq!(
            manifest["hook_attributes"]["url"],
            "https://gridops.example.com/api/webhooks/github"
        );
        Ok(())
    }
}
