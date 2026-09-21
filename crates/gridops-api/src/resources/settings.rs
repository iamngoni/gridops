//! Settings and Bitbucket connection API handlers.

use super::*;

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

pub async fn bitbucket_connections(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(
        r#"SELECT id,name,workspace,workspace_uuid,created_at,updated_at
           FROM bitbucket_connections ORDER BY lower(name),id"#,
    )
    .fetch_all(&state.database)
    .await?;
    Ok(Json(json!({
        "canManage": user.role == "admin",
        "items": rows.into_iter().map(|row| json!({
            "id": row.get::<String,_>("id"),
            "provider": "bitbucket",
            "name": row.get::<String,_>("name"),
            "workspace": row.get::<String,_>("workspace"),
            "workspaceUuid": row.get::<String,_>("workspace_uuid"),
            "createdAt": iso(row.get::<i64,_>("created_at")),
            "updatedAt": iso(row.get::<i64,_>("updated_at")),
            "canManage": user.role == "admin",
        })).collect::<Vec<_>>()
    })))
}

pub async fn create_bitbucket_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<CreateBitbucketConnection>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    assert_same_origin(&state, &headers)?;
    require_system_admin(&user)?;
    let name = input.name.trim();
    let workspace = input.workspace.trim().to_lowercase();
    let access_token = input.access_token.trim();
    if !(2..=80).contains(&name.len()) || name.contains(['\r', '\n']) {
        return Err(ApiError::BadRequest(
            "Bitbucket connection name must contain 2-80 visible characters.".into(),
        ));
    }
    if workspace.is_empty()
        || workspace.len() > 255
        || !workspace.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        })
    {
        return Err(ApiError::BadRequest(
            "Bitbucket workspace must contain lowercase letters, numbers, hyphens, or underscores."
                .into(),
        ));
    }
    if !(20..=4_096).contains(&access_token.len()) || access_token.contains(['\r', '\n']) {
        return Err(ApiError::BadRequest(
            "Bitbucket API token is invalid.".into(),
        ));
    }

    let verified_workspace = state
        .bitbucket
        .workspace(&workspace, access_token)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    if verified_workspace.slug.is_empty() || verified_workspace.uuid.is_empty() {
        return Err(ApiError::BadRequest(
            "Bitbucket returned an incomplete workspace identity.".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let access_token_key = format!("bitbucket.connection.{id}.access_token");
    let now = now_millis();
    let sealed = state.vault.seal(access_token).map_err(ApiError::Internal)?;
    let mut transaction = state.database.begin().await?;
    let result = async {
        sqlx::query(
            r#"INSERT INTO runtime_secrets (key,value,updated_by,updated_at) VALUES (?,?,?,?)"#,
        )
        .bind(&access_token_key)
        .bind(sealed)
        .bind(&user.id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"INSERT INTO bitbucket_connections
              (id,name,workspace,workspace_uuid,access_token_key,created_by,created_at,updated_at)
              VALUES (?,?,?,?,?,?,?,?)"#,
        )
        .bind(&id)
        .bind(name)
        .bind(&verified_workspace.slug)
        .bind(&verified_workspace.uuid)
        .bind(&access_token_key)
        .bind(&user.id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(error) = result {
        transaction.rollback().await?;
        return Err(error);
    }
    transaction.commit().await?;
    audit(
        &state,
        &user,
        "bitbucket.connection_created",
        "bitbucket_connection",
        Some(&id),
        json!({ "name": name, "workspace": verified_workspace.slug }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "provider": "bitbucket",
            "name": name,
            "workspace": verified_workspace.slug,
            "workspaceUuid": verified_workspace.uuid,
        })),
    ))
}
