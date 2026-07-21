use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use gridops_core::{
    GitHubInstallation, GitHubOrganizationMembership, GitHubRepository, GitHubUser,
    InstallationPage, crypto::hash_token, now_millis,
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{QueryBuilder, Row as _, Sqlite};

use crate::{
    auth::{AuthUser, audit, create_session, random_token},
    error::{ApiError, ApiResult},
    state::AppState,
};

const OAUTH_STATE_TTL_MILLIS: i64 = 10 * 60 * 1_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginQuery {
    return_to: Option<String>,
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

pub async fn begin(
    State(state): State<AppState>,
    Query(query): Query<BeginQuery>,
) -> ApiResult<Redirect> {
    let (client_id, _) = oauth_credentials(&state).await?;
    let oauth_state = random_token(32);
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let now = now_millis();

    sqlx::query(
        "INSERT INTO oauth_states (id,state_hash,code_verifier,return_to,expires_at,created_at) VALUES (?,?,?,?,?,?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(hash_token(&oauth_state))
    .bind(state.vault.seal(&verifier).map_err(ApiError::Internal)?)
    .bind(safe_return_to(query.return_to.as_deref()))
    .bind(now + OAUTH_STATE_TTL_MILLIS)
    .bind(now)
    .execute(&state.database)
    .await?;

    let mut authorize = url::Url::parse("https://github.com/login/oauth/authorize")
        .map_err(|error| ApiError::Internal(error.into()))?;
    authorize
        .query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &callback_url(&state))
        .append_pair("state", &oauth_state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(Redirect::temporary(authorize.as_str()))
}

pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    if let Some(error) = query.error {
        return Ok(error_redirect(
            &state,
            query.error_description.as_deref().unwrap_or(&error),
        ));
    }
    let (Some(code), Some(oauth_state)) = (query.code, query.state) else {
        return Ok(error_redirect(
            &state,
            "GitHub did not return a valid authorization code.",
        ));
    };

    let record = sqlx::query(
        "SELECT id,code_verifier,return_to,expires_at FROM oauth_states WHERE state_hash = ?",
    )
    .bind(hash_token(&oauth_state))
    .fetch_optional(&state.database)
    .await?;
    let Some(record) = record else {
        return Ok(error_redirect(
            &state,
            "The GitHub authorization request is invalid.",
        ));
    };
    sqlx::query("DELETE FROM oauth_states WHERE id = ?")
        .bind(record.get::<String, _>("id"))
        .execute(&state.database)
        .await?;
    if record.get::<i64, _>("expires_at") <= now_millis() {
        return Ok(error_redirect(
            &state,
            "The GitHub authorization request expired.",
        ));
    }

    let verifier = state
        .vault
        .open(&record.get::<String, _>("code_verifier"))
        .map_err(ApiError::Internal)?;
    let token = exchange_code(&state, &code, &verifier).await?;
    let access_token = token.access_token.ok_or_else(|| {
        ApiError::BadRequest(
            token
                .error_description
                .or(token.error)
                .unwrap_or_else(|| "GitHub token exchange failed.".into()),
        )
    })?;
    let profile: GitHubUser = state
        .github
        .get("/user", &access_token)
        .await
        .map_err(ApiError::Internal)?;
    let now = now_millis();
    let existing = sqlx::query("SELECT id FROM users WHERE github_id = ?")
        .bind(profile.id)
        .fetch_optional(&state.database)
        .await?;
    let user_id = existing
        .map(|row| row.get::<String, _>("id"))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    sqlx::query(
        r#"
        INSERT INTO users (
          id,github_id,login,name,email,avatar_url,access_token,access_token_expires_at,
          refresh_token,refresh_token_expires_at,role,last_login_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,?,?,?,
          CASE WHEN EXISTS (SELECT 1 FROM users WHERE role='admin') THEN 'member' ELSE 'admin' END,
          ?,?,?)
        ON CONFLICT(github_id) DO UPDATE SET
          login=excluded.login,name=excluded.name,email=excluded.email,avatar_url=excluded.avatar_url,
          access_token=excluded.access_token,access_token_expires_at=excluded.access_token_expires_at,
          refresh_token=COALESCE(excluded.refresh_token,users.refresh_token),
          refresh_token_expires_at=COALESCE(excluded.refresh_token_expires_at,users.refresh_token_expires_at),
          last_login_at=excluded.last_login_at,updated_at=excluded.updated_at
        "#,
    )
    .bind(&user_id)
    .bind(profile.id)
    .bind(&profile.login)
    .bind(&profile.name)
    .bind(&profile.email)
    .bind(&profile.avatar_url)
    .bind(state.vault.seal(&access_token).map_err(ApiError::Internal)?)
    .bind(token.expires_in.map(|seconds| now + seconds * 1_000))
    .bind(
        token
            .refresh_token
            .as_deref()
            .map(|value| state.vault.seal(value))
            .transpose()
            .map_err(ApiError::Internal)?,
    )
    .bind(
        token
            .refresh_token_expires_in
            .map(|seconds| now + seconds * 1_000),
    )
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await?;

    if state
        .github_app_credentials()
        .await
        .map_err(ApiError::Internal)?
        .is_some()
    {
        sync_user_installations(&state, &user_id, &access_token).await?;
    } else {
        tracing::info!(
            user = %profile.login,
            "bootstrap OAuth login completed; installation sync awaits GitHub App setup"
        );
    }
    let cookie = create_session(
        &state,
        &user_id,
        headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok()),
    )
    .await?;
    let location = state
        .config
        .base_url()
        .join(&record.get::<String, _>("return_to"))
        .map_err(|error| ApiError::Internal(error.into()))?;
    let mut response = Redirect::to(location.as_str()).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?,
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store"
            .parse()
            .map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?,
    );
    Ok(response)
}

pub async fn sync(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    if state
        .github_app_credentials()
        .await
        .map_err(ApiError::Internal)?
        .is_none()
    {
        return Err(ApiError::Conflict(
            "Create the controller GitHub App before synchronizing installations.".into(),
        ));
    }
    let token = user_access_token(&state, &user.id).await?;
    sync_user_installations(&state, &user.id, &token).await?;
    let count = sqlx::query(
        r#"SELECT COUNT(*) AS count FROM repositories r
           JOIN user_installations ui ON ui.installation_id=r.installation_id
           WHERE ui.user_id=?"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?
    .get::<i64, _>("count");
    audit(
        &state,
        &user,
        "github.synced",
        "github_account",
        Some(&user.login),
        json!({ "repositories": count }),
    )
    .await?;
    Ok(Json(json!({ "repositories": count })))
}

pub async fn user_access_token(state: &AppState, user_id: &str) -> ApiResult<String> {
    let row = sqlx::query(
        "SELECT access_token,access_token_expires_at,refresh_token,refresh_token_expires_at FROM users WHERE id=?",
    )
    .bind(user_id)
    .fetch_optional(&state.database)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let expires_at = row.try_get::<Option<i64>, _>("access_token_expires_at")?;
    if expires_at.is_none_or(|expiry| expiry > now_millis() + 5 * 60_000) {
        return state
            .vault
            .open(&row.get::<String, _>("access_token"))
            .map_err(ApiError::Internal);
    }

    let refresh_expires_at = row.try_get::<Option<i64>, _>("refresh_token_expires_at")?;
    if refresh_expires_at.is_some_and(|expiry| expiry <= now_millis()) {
        return Err(ApiError::Unauthorized);
    }
    let sealed_refresh = row
        .try_get::<Option<String>, _>("refresh_token")?
        .ok_or(ApiError::Unauthorized)?;
    let refresh_token = state
        .vault
        .open(&sealed_refresh)
        .map_err(ApiError::Internal)?;
    let refreshed = refresh_access_token(state, &refresh_token).await?;
    let access_token = refreshed
        .access_token
        .ok_or_else(|| ApiError::BadRequest("GitHub rejected the refresh token.".into()))?;
    let now = now_millis();
    let next_refresh = refreshed.refresh_token.as_deref().unwrap_or(&refresh_token);
    sqlx::query(
        r#"UPDATE users SET access_token=?,access_token_expires_at=?,refresh_token=?,
           refresh_token_expires_at=COALESCE(?,refresh_token_expires_at),updated_at=? WHERE id=?"#,
    )
    .bind(
        state
            .vault
            .seal(&access_token)
            .map_err(ApiError::Internal)?,
    )
    .bind(refreshed.expires_in.map(|seconds| now + seconds * 1_000))
    .bind(state.vault.seal(next_refresh).map_err(ApiError::Internal)?)
    .bind(
        refreshed
            .refresh_token_expires_in
            .map(|seconds| now + seconds * 1_000),
    )
    .bind(now)
    .bind(user_id)
    .execute(&state.database)
    .await?;
    Ok(access_token)
}

pub async fn control_token(
    state: &AppState,
    user_id: &str,
    installation_id: i64,
) -> ApiResult<String> {
    if let Some(token) = state
        .installation_token(installation_id)
        .await
        .map_err(ApiError::Internal)?
    {
        return Ok(token);
    }
    user_access_token(state, user_id).await
}

async fn exchange_code(state: &AppState, code: &str, verifier: &str) -> ApiResult<TokenResponse> {
    let (client_id, client_secret) = oauth_credentials(state).await?;
    token_request(
        state,
        &[
            ("client_id", &client_id),
            ("client_secret", &client_secret),
            ("code", code),
            ("redirect_uri", &callback_url(state)),
            ("code_verifier", verifier),
        ],
    )
    .await
}

async fn refresh_access_token(state: &AppState, refresh_token: &str) -> ApiResult<TokenResponse> {
    let (client_id, client_secret) = oauth_credentials(state).await?;
    token_request(
        state,
        &[
            ("client_id", &client_id),
            ("client_secret", &client_secret),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

async fn token_request(state: &AppState, values: &[(&str, &str)]) -> ApiResult<TokenResponse> {
    let response = state
        .http
        .request(Method::POST, "https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .form(values)
        .send()
        .await?;
    let status = response.status();
    let token = response.json::<TokenResponse>().await?;
    if !status.is_success() {
        return Err(ApiError::BadRequest(
            token
                .error_description
                .or(token.error)
                .unwrap_or_else(|| format!("GitHub token exchange failed ({status}).")),
        ));
    }
    Ok(token)
}

async fn sync_user_installations(
    state: &AppState,
    user_id: &str,
    access_token: &str,
) -> ApiResult<()> {
    let user_login = sqlx::query_scalar::<_, String>("SELECT login FROM users WHERE id=?")
        .bind(user_id)
        .fetch_one(&state.database)
        .await?;
    let mut installations = Vec::new();
    for page in 1..=100 {
        let response: InstallationPage = state
            .github
            .get(
                &format!("/user/installations?per_page=100&page={page}"),
                access_token,
            )
            .await
            .map_err(ApiError::Internal)?;
        let final_page = response.installations.len() < 100;
        installations.extend(response.installations);
        if final_page {
            break;
        }
    }

    for installation in &installations {
        let Some(account) = &installation.account else {
            continue;
        };
        upsert_installation(state, installation).await?;
        let permission = installation_permission(state, account, &user_login, access_token).await;
        sqlx::query(
            r#"INSERT INTO user_installations (user_id,installation_id,permission,created_at)
               VALUES (?,?,?,?) ON CONFLICT(user_id,installation_id)
               DO UPDATE SET permission=excluded.permission"#,
        )
        .bind(user_id)
        .bind(installation.id)
        .bind(permission)
        .bind(now_millis())
        .execute(&state.database)
        .await?;

        tracing::info!(installation = %account.login, "GitHub installation synced");
    }

    let mut transaction = state.database.begin().await?;
    if installations.is_empty() {
        sqlx::query(
            r#"DELETE FROM user_installations WHERE user_id=? AND installation_id NOT IN
               (SELECT id FROM installations WHERE suspended_at IS NOT NULL)"#,
        )
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    } else {
        let mut query =
            QueryBuilder::<Sqlite>::new("DELETE FROM user_installations WHERE user_id=");
        query
            .push_bind(user_id)
            .push(" AND installation_id NOT IN (");
        let mut separated = query.separated(",");
        for installation in &installations {
            separated.push_bind(installation.id);
        }
        separated.push_unseparated(") AND installation_id NOT IN (SELECT id FROM installations WHERE suspended_at IS NOT NULL)");
        query.build().execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn installation_permission(
    state: &AppState,
    account: &gridops_core::github::GitHubAccount,
    user_login: &str,
    access_token: &str,
) -> &'static str {
    if account.kind == "Organization" {
        match state
            .github
            .get::<GitHubOrganizationMembership>(
                &format!("/user/memberships/orgs/{}", account.login),
                access_token,
            )
            .await
        {
            Ok(membership) => {
                permission_for_account(&account.kind, &account.login, user_login, Some(&membership))
            }
            Err(error) => {
                tracing::warn!(organization = %account.login, error = ?error, "could not determine organization ownership; granting read-only access");
                "read"
            }
        }
    } else {
        permission_for_account(&account.kind, &account.login, user_login, None)
    }
}

fn permission_for_account(
    account_kind: &str,
    account_login: &str,
    user_login: &str,
    membership: Option<&GitHubOrganizationMembership>,
) -> &'static str {
    if account_kind == "Organization" {
        if membership
            .is_some_and(|membership| membership.state == "active" && membership.role == "admin")
        {
            "admin"
        } else {
            "read"
        }
    } else if account_login.eq_ignore_ascii_case(user_login) {
        "admin"
    } else {
        "read"
    }
}

async fn upsert_installation(state: &AppState, installation: &GitHubInstallation) -> ApiResult<()> {
    let Some(account) = &installation.account else {
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
    .bind(installation.id)
    .bind(account.id)
    .bind(&account.login)
    .bind(&account.kind)
    .bind(&account.avatar_url)
    .bind(&installation.target_type)
    .bind(&installation.repository_selection)
    .bind(installation.permissions.to_string())
    .bind(
        serde_json::to_string(&installation.events)
            .map_err(|error| ApiError::Internal(error.into()))?,
    )
    .bind(parse_date(installation.suspended_at.as_deref()))
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await?;
    if installation.suspended_at.is_some() {
        sqlx::query(
            r#"UPDATE runner_pools SET paused=1,state='draining',updated_at=?
              WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped
                WHERE mapped.pool_id=runner_pools.id AND mapped.installation_id=?)"#,
        )
        .bind(now)
        .bind(installation.id)
        .execute(&state.database)
        .await?;
    }
    Ok(())
}

pub(crate) async fn upsert_repository(
    state: &AppState,
    installation_id: i64,
    repository: &GitHubRepository,
) -> ApiResult<()> {
    let permission = repository.permissions.as_ref().and_then(|permissions| {
        permissions
            .iter()
            .find_map(|(name, allowed)| allowed.then_some(name.as_str()))
    });
    let now = now_millis();
    sqlx::query(
        r#"INSERT INTO repositories (
          id,installation_id,owner,name,full_name,private,archived,default_branch,html_url,
          permission,github_updated_at,last_synced_at,created_at,updated_at
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT(id) DO UPDATE SET installation_id=excluded.installation_id,
          owner=excluded.owner,name=excluded.name,full_name=excluded.full_name,
          private=excluded.private,archived=excluded.archived,default_branch=excluded.default_branch,
          html_url=excluded.html_url,permission=excluded.permission,
          github_updated_at=excluded.github_updated_at,last_synced_at=excluded.last_synced_at,
          updated_at=excluded.updated_at"#,
    )
    .bind(repository.id)
    .bind(installation_id)
    .bind(&repository.owner.login)
    .bind(&repository.name)
    .bind(&repository.full_name)
    .bind(repository.private)
    .bind(repository.archived)
    .bind(&repository.default_branch)
    .bind(&repository.html_url)
    .bind(permission)
    .bind(parse_date(repository.updated_at.as_deref()))
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&state.database)
    .await?;
    if repository.archived {
        sqlx::query("DELETE FROM runner_pool_repositories WHERE repository_id=?")
            .bind(repository.id)
            .execute(&state.database)
            .await?;
        sqlx::query(
            r#"UPDATE runner_pools SET
               repository_id=(SELECT repository_id FROM runner_pool_repositories membership
                 WHERE membership.pool_id=runner_pools.id ORDER BY created_at,repository_id LIMIT 1),
               installation_id=COALESCE((SELECT repo.installation_id
                 FROM runner_pool_repositories membership JOIN repositories repo ON repo.id=membership.repository_id
                 WHERE membership.pool_id=runner_pools.id ORDER BY membership.created_at,repo.id LIMIT 1),installation_id),
               paused=CASE WHEN NOT EXISTS (SELECT 1 FROM runner_pool_repositories membership
                 WHERE membership.pool_id=runner_pools.id) THEN 1 ELSE paused END,
               state=CASE WHEN NOT EXISTS (SELECT 1 FROM runner_pool_repositories membership
                 WHERE membership.pool_id=runner_pools.id) THEN 'draining' ELSE 'updating' END,
               configuration_version=configuration_version+1,updated_at=?
               WHERE scope='repository' AND (repository_id=? OR EXISTS (
                 SELECT 1 FROM runners runner WHERE runner.pool_id=runner_pools.id
                   AND runner.target_repository_id=? AND runner.deleted_at IS NULL))"#,
        )
        .bind(now)
        .bind(repository.id)
        .bind(repository.id)
        .execute(&state.database)
        .await?;
    }
    Ok(())
}

async fn oauth_credentials(state: &AppState) -> ApiResult<(String, String)> {
    state
        .github_oauth_credentials()
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::BadRequest("GitHub OAuth is not configured.".into()))
}

fn callback_url(state: &AppState) -> String {
    state
        .config
        .base_url()
        .join("/auth/github/callback")
        .map_or_else(
            |_| format!("{}/auth/github/callback", state.config.base_url()),
            |url| url.to_string(),
        )
}

fn safe_return_to(value: Option<&str>) -> String {
    value
        .filter(|path| path.starts_with('/') && !path.starts_with("//"))
        .unwrap_or("/")
        .to_owned()
}

fn parse_date(value: Option<&str>) -> Option<i64> {
    value
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
        .map(|date| date.timestamp_millis())
}

fn error_redirect(state: &AppState, message: &str) -> Response {
    let mut url = state.config.base_url().clone();
    url.set_path("/login");
    url.query_pairs_mut().append_pair("authError", message);
    (StatusCode::FOUND, [(header::LOCATION, url.to_string())]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_permissions_follow_account_ownership() {
        let owner = GitHubOrganizationMembership {
            state: "active".into(),
            role: "admin".into(),
        };
        let member = GitHubOrganizationMembership {
            state: "active".into(),
            role: "member".into(),
        };
        assert_eq!(
            permission_for_account("Organization", "octo-org", "octocat", Some(&owner)),
            "admin"
        );
        assert_eq!(
            permission_for_account("Organization", "octo-org", "octocat", Some(&member)),
            "read"
        );
        assert_eq!(
            permission_for_account("User", "OctoCat", "octocat", None),
            "admin"
        );
        assert_eq!(
            permission_for_account("User", "someone-else", "octocat", None),
            "read"
        );
    }
}
