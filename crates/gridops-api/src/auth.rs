use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use gridops_core::{
    crypto::hash_token,
    models::{Alerts, Viewer},
    now_millis,
};
use rand::RngExt as _;
use sqlx::Row as _;

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

const SESSION_COOKIE: &str = "gridops_session";
const SESSION_TTL_MILLIS: i64 = 30 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: String,
    pub login: String,
    pub github_id: i64,
    pub role: String,
}

pub struct OptionalAuth(pub Option<AuthUser>);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let value = cookie(
            parts
                .headers
                .get(header::COOKIE)
                .and_then(|value| value.to_str().ok()),
            SESSION_COOKIE,
        )
        .ok_or(ApiError::Unauthorized)?;
        let (token, signature) = value.split_once('.').ok_or(ApiError::Unauthorized)?;
        if !state.vault.verify(token, signature) {
            return Err(ApiError::Unauthorized);
        }
        let row = sqlx::query(
            r#"
            SELECT u.id, u.login, u.github_id, u.role, s.id AS session_id, s.expires_at
            FROM sessions s JOIN users u ON u.id = s.user_id
            WHERE s.token_hash = ?
        "#,
        )
        .bind(hash_token(token))
        .fetch_optional(&state.database)
        .await?;
        let Some(row) = row else {
            return Err(ApiError::Unauthorized);
        };
        if row.get::<i64, _>("expires_at") <= now_millis() {
            return Err(ApiError::Unauthorized);
        }
        sqlx::query("UPDATE sessions SET last_seen_at = ? WHERE id = ?")
            .bind(now_millis())
            .bind(row.get::<String, _>("session_id"))
            .execute(&state.database)
            .await?;
        Ok(Self {
            id: row.get("id"),
            login: row.get("login"),
            github_id: row.get("github_id"),
            role: row.get("role"),
        })
    }
}

impl FromRequestParts<AppState> for OptionalAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match AuthUser::from_request_parts(parts, state).await {
            Ok(user) => Ok(Self(Some(user))),
            Err(ApiError::Unauthorized) => Ok(Self(None)),
            Err(error) => Err(error),
        }
    }
}

pub async fn me(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Viewer>> {
    let profile = sqlx::query("SELECT name, email, avatar_url FROM users WHERE id = ?")
        .bind(&user.id)
        .fetch_one(&state.database)
        .await?;
    let alerts = sqlx::query(r#"
      SELECT
        (SELECT COUNT(*) FROM runners r JOIN runner_pools p ON p.id=r.pool_id JOIN user_installations ui ON ui.installation_id=p.installation_id WHERE ui.user_id=? AND r.deleted_at IS NULL AND r.status='failed') AS failed_runners,
        (SELECT COUNT(*) FROM webhook_deliveries wd WHERE wd.status IN ('failed','rejected') AND (wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui WHERE ui.installation_id=wd.installation_id AND ui.user_id=?))) AS failed_webhooks,
        (SELECT COUNT(*) FROM workflow_jobs wj JOIN workflow_runs wr ON wr.id=wj.run_id JOIN repositories repo ON repo.id=wr.repository_id JOIN user_installations ui ON ui.installation_id=repo.installation_id WHERE ui.user_id=? AND wj.status='queued') AS queued_jobs,
        (SELECT COUNT(*) FROM github_runner_cleanup cleanup JOIN user_installations ui ON ui.installation_id=cleanup.installation_id WHERE ui.user_id=?) AS deferred_runner_cleanup
    "#).bind(&user.id).bind(&user.id).bind(&user.id).bind(&user.id).fetch_one(&state.database).await?;
    Ok(Json(Viewer {
        id: user.id,
        github_id: user.github_id,
        login: user.login,
        name: profile.try_get("name")?,
        email: profile.try_get("email")?,
        avatar_url: profile.try_get("avatar_url")?,
        role: user.role,
        alerts: Alerts {
            failed_runners: alerts.get("failed_runners"),
            failed_webhooks: alerts.get("failed_webhooks"),
            queued_jobs: alerts.get("queued_jobs"),
            deferred_runner_cleanup: alerts.get("deferred_runner_cleanup"),
        },
    }))
}

pub fn require_system_admin(user: &AuthUser) -> ApiResult<()> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

pub async fn assert_installation_admin(
    state: &AppState,
    user: &AuthUser,
    installation_id: i64,
) -> ApiResult<()> {
    let permission = sqlx::query_scalar::<_, String>(
        r#"SELECT ui.permission FROM user_installations ui
          JOIN installations i ON i.id=ui.installation_id
          WHERE ui.user_id=? AND ui.installation_id=? AND i.suspended_at IS NULL
          LIMIT 1"#,
    )
    .bind(&user.id)
    .bind(installation_id)
    .fetch_optional(&state.database)
    .await?;
    if permission
        .as_deref()
        .is_some_and(|permission| can_administer_installation(&user.role, permission))
    {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

fn can_administer_installation(system_role: &str, installation_permission: &str) -> bool {
    system_role == "admin" || installation_permission == "admin"
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> ApiResult<Response> {
    assert_same_origin(&state, &headers)?;
    if let Some(value) = cookie(
        headers
            .get(header::COOKIE)
            .and_then(|value| value.to_str().ok()),
        SESSION_COOKIE,
    ) && let Some((token, _)) = value.split_once('.')
    {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(hash_token(token))
            .execute(&state.database)
            .await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        clear_cookie(&state)?
            .parse()
            .map_err(|error| ApiError::Internal(anyhow::Error::new(error)))?,
    );
    audit(
        &state,
        &user,
        "auth.logout",
        "user",
        Some(&user.id),
        serde_json::json!({}),
    )
    .await?;
    Ok(response)
}

pub async fn create_session(
    state: &AppState,
    user_id: &str,
    user_agent: Option<&str>,
) -> ApiResult<String> {
    let token = random_token(32);
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_millis();
    sqlx::query("INSERT INTO sessions (id,token_hash,user_id,user_agent,expires_at,last_seen_at,created_at) VALUES (?,?,?,?,?,?,?)")
        .bind(id).bind(hash_token(&token)).bind(user_id).bind(user_agent).bind(now + SESSION_TTL_MILLIS).bind(now).bind(now)
        .execute(&state.database).await?;
    let signed = format!(
        "{token}.{}",
        state.vault.sign(&token).map_err(ApiError::Internal)?
    );
    Ok(format!(
        "{SESSION_COOKIE}={signed}; Path=/; HttpOnly; SameSite=Lax; Max-Age={};{}",
        SESSION_TTL_MILLIS / 1_000,
        if state.config.base_url().scheme() == "https" {
            " Secure;"
        } else {
            ""
        }
    ))
}

pub fn assert_same_origin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let expected = state.config.base_url().origin().ascii_serialization();
        if origin != expected {
            return Err(ApiError::Forbidden);
        }
    }
    Ok(())
}

pub fn random_token(bytes: usize) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut value = vec![0_u8; bytes];
    rand::rng().fill(&mut value[..]);
    URL_SAFE_NO_PAD.encode(value)
}

fn cookie<'a>(header: Option<&'a str>, name: &str) -> Option<&'a str> {
    header?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

fn clear_cookie(state: &AppState) -> anyhow::Result<String> {
    Ok(format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0;{}",
        if state.config.base_url().scheme() == "https" {
            " Secure;"
        } else {
            ""
        }
    ))
}

pub async fn audit(
    state: &AppState,
    user: &AuthUser,
    action: &str,
    target_type: &str,
    target_id: Option<&str>,
    metadata: serde_json::Value,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_events (id,actor_user_id,actor_label,action,target_type,target_id,metadata,created_at) VALUES (?,?,?,?,?,?,?,?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(&user.id).bind(&user.login).bind(action).bind(target_type).bind(target_id)
        .bind(metadata.to_string()).bind(now_millis()).execute(&state.database).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(role: &str) -> AuthUser {
        AuthUser {
            id: "user-1".into(),
            login: "octocat".into(),
            github_id: 1,
            role: role.into(),
        }
    }

    #[test]
    fn system_policy_requires_an_administrator() {
        assert!(require_system_admin(&user("admin")).is_ok());
        assert!(matches!(
            require_system_admin(&user("member")),
            Err(ApiError::Forbidden)
        ));
    }

    #[test]
    fn installation_policy_accepts_system_or_installation_administrators() {
        assert!(can_administer_installation("admin", "read"));
        assert!(can_administer_installation("member", "admin"));
        assert!(!can_administer_installation("member", "read"));
    }
}
