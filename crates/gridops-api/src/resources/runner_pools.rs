//! Runner-pool lifecycle, provisioning, and access helpers.

use super::repositories::{
    bitbucket_runner_labels, next_bitbucket_connection, selected_available_repositories,
    selected_bitbucket_connections,
};
use super::*;

pub async fn runner_pools(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM runner_pools p
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT p.id,p.name,p.scope,p.mode,p.provider,p.providers,p.labels,p.image,
          p.docker_image,p.tart_image,p.macos_runtime,p.desired_count,p.min_count,
          p.max_count,CAST(p.cpu_limit AS REAL) AS cpu_limit,p.memory_limit_mb,p.paused,p.state,
          p.provision_failure_count,p.provision_retry_at,p.provision_circuit_open,i.account_login,
          CASE WHEN NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations manage
                WHERE manage.user_id=? AND manage.installation_id=mapped.installation_id
                  AND manage.permission='admin')
          ) THEN 'admin' ELSE 'read' END AS installation_permission,
          repo.full_name AS repository,
          (SELECT COUNT(*) FROM runner_pool_repositories membership WHERE membership.pool_id=p.id) AS repository_count,
          (SELECT COUNT(*) FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id) AS account_count,
          COUNT(CASE WHEN r.deleted_at IS NULL THEN 1 END) AS total_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('online','idle','busy') THEN 1 END) AS online_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN 1 END) AS busy_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status='failed' THEN 1 END) AS failed_runners,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.configuration_version < p.configuration_version THEN 1 END) AS outdated_runners,
          p.created_at FROM runner_pools p
        JOIN installations i ON i.id=p.installation_id LEFT JOIN repositories repo ON repo.id=p.repository_id
        LEFT JOIN runners r ON r.pool_id=p.id
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
        GROUP BY p.id ORDER BY p.created_at DESC
        LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "name": row.get::<String,_>("name"), "scope": row.get::<String,_>("scope"),
        "mode": row.get::<String,_>("mode"), "provider": row.get::<String,_>("provider"),
        "providers": json_array(row.get::<&str,_>("providers")),
        "labels": json_array(row.get::<&str,_>("labels")), "image": row.get::<String,_>("image"),
        "dockerImage": row.get::<String,_>("docker_image"), "tartImage": row.get::<String,_>("tart_image"),
        "macosRuntime": row.get::<String,_>("macos_runtime"),
        "desiredCount": row.get::<i64,_>("desired_count"), "minCount": row.get::<i64,_>("min_count"), "maxCount": row.get::<i64,_>("max_count"),
        "cpuLimit": row.get::<f64,_>("cpu_limit"), "memoryLimitMb": row.get::<i64,_>("memory_limit_mb"),
        "paused": row.get::<bool,_>("paused"), "state": row.get::<String,_>("state"), "accountLogin": row.get::<String,_>("account_login"),
        "provisionFailureCount": row.get::<i64,_>("provision_failure_count"),
        "provisionRetryAt": iso_optional(row.try_get::<Option<i64>,_>("provision_retry_at").ok().flatten()),
        "provisionCircuitOpen": row.get::<bool,_>("provision_circuit_open"),
        "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(), "repositoryCount": row.get::<i64,_>("repository_count"),
        "accountCount": row.get::<i64,_>("account_count"), "totalRunners": row.get::<i64,_>("total_runners"),
        "onlineRunners": row.get::<i64,_>("online_runners"), "busyRunners": row.get::<i64,_>("busy_runners"),
        "failedRunners": row.get::<i64,_>("failed_runners"), "outdatedRunners": row.get::<i64,_>("outdated_runners"),
        "canManage": user.role == "admin" || row.get::<String,_>("installation_permission") == "admin",
        "createdAt": iso(row.get::<i64,_>("created_at")),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn runner_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let pool = pool_access(&state, &user, &pool_id).await?;
    let additional_labels = json_array(&pool.labels)
        .into_iter()
        .filter(|label| label != &pool.name)
        .collect::<Vec<_>>();
    let repository = pool
        .repository_owner
        .as_ref()
        .zip(pool.repository_name.as_ref())
        .map(|(owner, repository)| format!("{owner}/{repository}"));
    let mut repositories = sqlx::query(
        r#"SELECT repo.id,repo.installation_id,repo.full_name,repo.private,
          installation.account_login,installation.account_type
          FROM runner_pool_repositories membership
           JOIN repositories repo ON repo.id=membership.repository_id
           JOIN installations installation ON installation.id=repo.installation_id
           WHERE membership.pool_id=? ORDER BY membership.created_at,repo.id"#,
    )
    .bind(&pool_id)
    .fetch_all(&state.database)
    .await?
    .into_iter()
    .map(|row| {
        json!({
            "id": row.get::<i64, _>("id"),
            "installationId": row.get::<i64, _>("installation_id"),
            "accountLogin": row.get::<String, _>("account_login"),
            "accountType": row.get::<String, _>("account_type"),
            "fullName": row.get::<String, _>("full_name"),
            "private": row.get::<bool, _>("private"),
        })
    })
    .collect::<Vec<_>>();
    if repositories.is_empty()
        && let Some(repository_id) = pool.repository_id
        && let Some(repository) = &repository
    {
        repositories.push(json!({
            "id": repository_id, "installationId": pool.installation_id,
            "accountLogin": pool.account_login, "accountType": "Unknown",
            "fullName": repository, "private": false
        }));
    }
    let repository_ids = repositories
        .iter()
        .filter_map(|repository| repository.get("id").and_then(Value::as_i64))
        .collect::<Vec<_>>();
    let bitbucket_connections = sqlx::query(
        r#"SELECT connection.id,connection.name,connection.workspace,connection.workspace_uuid
           FROM runner_pool_bitbucket_connections membership
           JOIN bitbucket_connections connection ON connection.id=membership.connection_id
           WHERE membership.pool_id=? ORDER BY membership.created_at,connection.id"#,
    )
    .bind(&pool_id)
    .fetch_all(&state.database)
    .await?
    .into_iter()
    .map(|row| {
        json!({
            "id": row.get::<String,_>("id"),
            "name": row.get::<String,_>("name"),
            "workspace": row.get::<String,_>("workspace"),
            "workspaceUuid": row.get::<String,_>("workspace_uuid"),
        })
    })
    .collect::<Vec<_>>();
    let bitbucket_connection_ids = bitbucket_connections
        .iter()
        .filter_map(|connection| connection.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let (max_cpu_limit, max_memory_limit_mb) = manager_resource_capacity(&state).await;
    Ok(Json(json!({
        "id": pool_id,
        "installationId": pool.installation_id,
        "repositoryId": pool.repository_id,
        "repository": repository,
        "repositoryIds": repository_ids,
        "repositories": repositories,
        "bitbucketConnectionIds": bitbucket_connection_ids,
        "bitbucketConnections": bitbucket_connections,
        "accountLogin": pool.account_login,
        "name": pool.name,
        "scope": pool.scope,
        "mode": pool.mode,
        "provider": pool.provider,
        "providers": json_array(&pool.providers),
        "labels": additional_labels,
        "image": pool.image,
        "dockerImage": pool.docker_image,
        "tartImage": pool.tart_image,
        "macosRuntime": pool.macos_runtime,
        "desiredCount": pool.desired_count,
        "minCount": pool.min_count,
        "maxCount": pool.max_count,
        "cpuLimit": pool.cpu_limit,
        "memoryLimitMb": pool.memory_limit_mb,
        "runnerGroupId": pool.runner_group_id,
        "paused": pool.paused,
        "state": pool.state,
        "autoscalingEnabled": pool.autoscaling_enabled,
        "queueScaleFactor": pool.queue_scale_factor,
        "idleTimeoutMinutes": pool.idle_timeout_minutes,
        "maxCpuLimit": max_cpu_limit,
        "maxMemoryLimitMb": max_memory_limit_mb,
        "configurationVersion": pool.configuration_version,
        "provisionFailureCount": pool.provision_failure_count,
        "provisionRetryAt": iso_optional(pool.provision_retry_at),
        "provisionCircuitOpen": pool.provision_circuit_open,
        "canManage": user.role == "admin" || pool.installation_permission == "admin",
    })))
}

pub async fn runner_pool_events(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    Query(query): Query<PaginationQuery>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    // Reuse the pool access check so lifecycle details never cross an
    // installation boundary, even when a user knows another pool's id.
    pool_access(&state, &user, &pool_id).await?;
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM runner_events WHERE pool_id=?")
        .bind(&pool_id)
        .fetch_one(&state.database)
        .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT id,runner_id,level,event,message,metadata,created_at
        FROM runner_events WHERE pool_id=?
        ORDER BY created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&pool_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let capacity_snapshot = if rows
        .iter()
        .any(|row| row.get::<String, _>("event") == "Runner provisioning waiting")
    {
        manager_json(&state, Method::GET, "v1/health", None)
            .await
            .ok()
            .and_then(|value| value.get("capacity").cloned())
    } else {
        None
    };
    let items = rows
        .iter()
        .map(|row| {
            let mut item = json!({
                "id": row.get::<String, _>("id"),
                "runnerId": row.try_get::<Option<String>, _>("runner_id").ok().flatten(),
                "level": row.get::<String, _>("level"),
                "event": row.get::<String, _>("event"),
                "message": row.get::<String, _>("message"),
                "metadata": row.get::<String, _>("metadata"),
                "createdAt": iso(row.get::<i64, _>("created_at")),
            });
            if row.get::<String, _>("event") == "Runner provisioning waiting"
                && let Some(snapshot) = &capacity_snapshot
            {
                item["capacitySnapshot"] = snapshot.clone();
            }
            item
        })
        .collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn update_runner_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<UpdateRunnerPool>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    input.validate().map_err(ApiError::BadRequest)?;
    let pool = pool_access(&state, &user, &pool_id).await?;
    assert_pool_admin(&state, &user, &pool_id).await?;
    let existing_repository_ids = sqlx::query_scalar::<_, i64>(
        "SELECT repository_id FROM runner_pool_repositories WHERE pool_id=? ORDER BY created_at,repository_id",
    )
    .bind(&pool_id)
    .fetch_all(&state.database)
    .await?;
    let existing_bitbucket_connection_ids = sqlx::query_scalar::<_, String>(
        "SELECT connection_id FROM runner_pool_bitbucket_connections WHERE pool_id=? ORDER BY created_at,connection_id",
    )
    .bind(&pool_id)
    .fetch_all(&state.database)
    .await?;
    let bitbucket_connection_ids = input
        .bitbucket_connection_ids
        .clone()
        .unwrap_or_else(|| existing_bitbucket_connection_ids.clone());
    let bitbucket_connections =
        selected_bitbucket_connections(&state, &user, &bitbucket_connection_ids).await?;
    let repository_ids = if pool.scope == "repository" {
        input.repository_ids.clone().unwrap_or_else(|| {
            if existing_repository_ids.is_empty() {
                pool.repository_id.into_iter().collect()
            } else {
                existing_repository_ids.clone()
            }
        })
    } else {
        Vec::new()
    };
    if pool.scope == "repository" && repository_ids.is_empty() {
        return Err(ApiError::BadRequest(
            "A repository pool requires at least one repository.".into(),
        ));
    }
    if i64::try_from(repository_ids.len()).unwrap_or(i64::MAX) > input.max_count {
        return Err(ApiError::BadRequest(
            "Repository count cannot exceed maximum runner capacity.".into(),
        ));
    }
    if input.repository_ids.is_some() && pool.scope != "repository" {
        return Err(ApiError::BadRequest(
            "Organization pools use runner-group repository access.".into(),
        ));
    }
    let selected = if input.repository_ids.is_some() {
        selected_available_repositories(&state, &user, &repository_ids).await?
    } else {
        Vec::new()
    };
    for item in &selected {
        upsert_repository(&state, item.installation.id, &item.repository).await?;
    }
    let primary_installation_id = selected
        .first()
        .map_or(pool.installation_id, |item| item.installation.id);
    let labels = normalized_pool_labels(&input.name, &input.labels)?;
    if !bitbucket_connections.is_empty() {
        bitbucket_runner_labels(&input.name, &labels)?;
    }
    let encoded_labels =
        serde_json::to_string(&labels).map_err(|error| ApiError::Internal(error.into()))?;
    let providers = input.selected_providers();
    let primary_provider = providers
        .first()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("Choose at least one runner provider.".into()))?;
    let encoded_providers =
        serde_json::to_string(&providers).map_err(|error| ApiError::Internal(error.into()))?;
    let docker_image = input.selected_docker_image();
    let tart_image = input.selected_tart_image();
    let macos_runtime = input.selected_macos_runtime();
    let primary_image = if primary_provider == "tart" {
        &tart_image
    } else {
        &docker_image
    };
    let runner_group_id = if pool.scope == "repository" {
        1
    } else {
        input.runner_group_id
    };
    let existing = existing_repository_ids.into_iter().collect::<HashSet<_>>();
    let requested = repository_ids.iter().copied().collect::<HashSet<_>>();
    let repositories_changed = existing != requested;
    let bitbucket_connections_changed = existing_bitbucket_connection_ids
        .iter()
        .collect::<HashSet<_>>()
        != bitbucket_connections.iter().collect::<HashSet<_>>();
    let runtime_changed = pool.name != input.name
        || pool.mode != input.mode
        || pool.provider != primary_provider
        || pool.providers != encoded_providers
        || pool.labels != encoded_labels
        || pool.image != primary_image.as_str()
        || pool.docker_image != docker_image
        || pool.tart_image != tart_image
        // Switching between VM and native changes how every runner executes, so
        // it has to roll the existing ones rather than apply to new ones only.
        || pool.macos_runtime != macos_runtime
        || (pool.cpu_limit - input.cpu_limit).abs() > f64::EPSILON
        || pool.memory_limit_mb != input.memory_limit_mb
        || pool.runner_group_id != runner_group_id
        || repositories_changed
        || bitbucket_connections_changed;
    let version_increment = i64::from(runtime_changed);
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    let result = sqlx::query(
        r#"UPDATE runner_pools SET installation_id=?,name=?,mode=?,provider=?,providers=?,labels=?,image=?,docker_image=?,tart_image=?,macos_runtime=?,desired_count=?,min_count=?,
          max_count=?,cpu_limit=?,memory_limit_mb=?,ephemeral=?,runner_group_id=?,
          autoscaling_enabled=?,queue_scale_factor=?,idle_timeout_minutes=?,
          repository_id=?,
          configuration_version=configuration_version+?,
          provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0,
          state=CASE WHEN ?=1 AND paused=0 THEN 'updating' ELSE state END,updated_at=? WHERE id=?"#,
    )
    .bind(primary_installation_id)
    .bind(&input.name)
    .bind(&input.mode)
    .bind(&primary_provider)
    .bind(&encoded_providers)
    .bind(&encoded_labels)
    .bind(primary_image)
    .bind(&docker_image)
    .bind(&tart_image)
    .bind(&macos_runtime)
    .bind(input.desired_count)
    .bind(input.min_count)
    .bind(input.max_count)
    .bind(input.cpu_limit)
    .bind(input.memory_limit_mb)
    .bind(input.mode == "ephemeral")
    .bind(runner_group_id)
    .bind(input.autoscaling_enabled)
    .bind(input.queue_scale_factor)
    .bind(input.idle_timeout_minutes)
    .bind(repository_ids.first().copied())
    .bind(version_increment)
    .bind(runtime_changed)
    .bind(now)
    .bind(&pool_id)
    .execute(&mut *transaction)
    .await;
    if let Err(sqlx::Error::Database(error)) = &result
        && error.is_unique_violation()
    {
        return Err(ApiError::Conflict(
            "A runner pool with this name already exists for the installation.".into(),
        ));
    }
    result?;
    if pool.scope == "repository" {
        sqlx::query("DELETE FROM runner_pool_repositories WHERE pool_id=?")
            .bind(&pool_id)
            .execute(&mut *transaction)
            .await?;
        for (position, repository_id) in repository_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES (?,?,?)",
            )
            .bind(&pool_id)
            .bind(repository_id)
            .bind(now.saturating_add(i64::try_from(position).unwrap_or(i64::MAX)))
            .execute(&mut *transaction)
            .await?;
        }
    }
    sqlx::query("DELETE FROM runner_pool_bitbucket_connections WHERE pool_id=?")
        .bind(&pool_id)
        .execute(&mut *transaction)
        .await?;
    for (position, connection_id) in bitbucket_connections.iter().enumerate() {
        sqlx::query(
            "INSERT INTO runner_pool_bitbucket_connections (pool_id,connection_id,created_at) VALUES (?,?,?)",
        )
        .bind(&pool_id)
        .bind(connection_id)
        .bind(now.saturating_add(i64::try_from(position).unwrap_or(i64::MAX)))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    audit(
        &state,
        &user,
        "runner_pool.updated",
        "runner_pool",
        Some(&pool_id),
        json!({
            "name": input.name,
            "providers": providers,
            "desiredCount": input.desired_count,
            "runtimeConfigurationChanged": runtime_changed,
            "repositoryIds": repository_ids,
            "bitbucketConnectionIds": bitbucket_connections,
            "configurationVersion": pool.configuration_version + version_increment,
        }),
    )
    .await?;
    Ok(Json(json!({
        "ok": true,
        "configurationVersion": pool.configuration_version + version_increment,
        "rollingReplacement": runtime_changed,
    })))
}

pub async fn create_runner_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<CreateRunnerPool>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    assert_same_origin(&state, &headers)?;
    input.validate().map_err(ApiError::BadRequest)?;
    let bitbucket_connections =
        selected_bitbucket_connections(&state, &user, &input.bitbucket_connection_ids).await?;
    let repository_ids = input.selected_repository_ids();
    let selected = if input.scope == "repository" {
        selected_available_repositories(&state, &user, &repository_ids).await?
    } else {
        assert_installation_admin(&state, &user, input.installation_id).await?;
        Vec::new()
    };
    for item in &selected {
        upsert_repository(&state, item.installation.id, &item.repository).await?;
    }
    let primary_installation_id = selected
        .first()
        .map_or(input.installation_id, |item| item.installation.id);
    let pool_id = uuid::Uuid::new_v4().to_string();
    let labels = normalized_pool_labels(&input.name, &input.labels)?;
    if !bitbucket_connections.is_empty() {
        bitbucket_runner_labels(&input.name, &labels)?;
    }
    let providers = input.selected_providers();
    let primary_provider = providers
        .first()
        .cloned()
        .ok_or_else(|| ApiError::BadRequest("Choose at least one runner provider.".into()))?;
    let encoded_providers =
        serde_json::to_string(&providers).map_err(|error| ApiError::Internal(error.into()))?;
    let docker_image = input.selected_docker_image();
    let tart_image = input.selected_tart_image();
    let macos_runtime = input.selected_macos_runtime();
    let primary_image = if primary_provider == "tart" {
        &tart_image
    } else {
        &docker_image
    };
    let runner_group_id = if input.scope == "repository" {
        1
    } else {
        input.runner_group_id
    };
    let now = now_millis();
    let mut transaction = state.database.begin().await?;
    let result = sqlx::query(
        r#"INSERT INTO runner_pools (
          id,installation_id,repository_id,name,scope,mode,provider,providers,labels,image,docker_image,tart_image,macos_runtime,desired_count,min_count,
          max_count,cpu_limit,memory_limit_mb,ephemeral,paused,state,created_by,created_at,updated_at,
          runner_group_id,autoscaling_enabled,queue_scale_factor,idle_timeout_minutes
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,0,'active',?,?,?,?,?,?,?)"#,
    )
    .bind(&pool_id).bind(primary_installation_id).bind(repository_ids.first().copied()).bind(&input.name)
    .bind(&input.scope).bind(&input.mode).bind(&primary_provider).bind(&encoded_providers)
    .bind(serde_json::to_string(&labels).map_err(|error| ApiError::Internal(error.into()))?)
    .bind(primary_image).bind(&docker_image).bind(&tart_image).bind(&macos_runtime)
    .bind(input.desired_count).bind(input.min_count).bind(input.max_count).bind(input.cpu_limit)
    .bind(input.memory_limit_mb).bind(input.mode == "ephemeral").bind(&user.id).bind(now).bind(now)
    .bind(runner_group_id).bind(input.autoscaling_enabled).bind(input.queue_scale_factor).bind(input.idle_timeout_minutes)
    .execute(&mut *transaction).await;
    if let Err(sqlx::Error::Database(error)) = &result
        && error.is_unique_violation()
    {
        return Err(ApiError::Conflict(
            "A runner pool with this name already exists for the installation.".into(),
        ));
    }
    result?;
    for (position, repository_id) in repository_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO runner_pool_repositories (pool_id,repository_id,created_at) VALUES (?,?,?)",
        )
        .bind(&pool_id)
        .bind(repository_id)
        .bind(now.saturating_add(i64::try_from(position).unwrap_or(i64::MAX)))
        .execute(&mut *transaction)
        .await?;
    }
    for (position, connection_id) in bitbucket_connections.iter().enumerate() {
        sqlx::query(
            "INSERT INTO runner_pool_bitbucket_connections (pool_id,connection_id,created_at) VALUES (?,?,?)",
        )
        .bind(&pool_id)
        .bind(connection_id)
        .bind(now.saturating_add(i64::try_from(position).unwrap_or(i64::MAX)))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    audit(
        &state,
        &user,
        "runner_pool.created",
        "runner_pool",
        Some(&pool_id),
        json!({ "name": input.name, "scope": input.scope, "providers": providers, "repositoryIds": repository_ids, "bitbucketConnectionIds": bitbucket_connections, "desiredCount": input.desired_count }),
    )
    .await?;
    let mut provisioned = Vec::new();
    for _ in 0..input.desired_count {
        match provision(&state, &user, &pool_id, None).await {
            Ok(runner) => provisioned.push(runner),
            Err(error) => provisioned
                .push(json!({ "runnerId": null, "status": "failed", "error": error.to_string() })),
        }
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": pool_id, "provisioned": provisioned })),
    ))
}

pub async fn runner_pool_action(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<PoolAction>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    match input.action.as_str() {
        "pause" => set_pool_paused(&state, &user, &pool_id, true).await?,
        "resume" => set_pool_paused(&state, &user, &pool_id, false).await?,
        "retry" => {
            pool_access(&state, &user, &pool_id).await?;
            assert_pool_admin(&state, &user, &pool_id).await?;
            sqlx::query("UPDATE runner_pools SET provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0,state='active',updated_at=? WHERE id=?")
                .bind(now_millis())
                .bind(&pool_id)
                .execute(&state.database)
                .await?;
            audit(
                &state,
                &user,
                "runner_pool.provisioning_retried",
                "runner_pool",
                Some(&pool_id),
                json!({}),
            )
            .await?;
        }
        "reconcile" => return Ok(Json(reconcile_pool(&state, &user, &pool_id).await?)),
        "scale" => {
            let pool = pool_access(&state, &user, &pool_id).await?;
            assert_pool_admin(&state, &user, &pool_id).await?;
            let desired = input
                .desired_count
                .ok_or_else(|| ApiError::BadRequest("desiredCount is required.".into()))?;
            if desired < pool.min_count || desired > pool.max_count {
                return Err(ApiError::BadRequest(format!(
                    "Desired capacity must be between {} and {}.",
                    pool.min_count, pool.max_count
                )));
            }
            sqlx::query("UPDATE runner_pools SET desired_count=?,updated_at=? WHERE id=?")
                .bind(desired)
                .bind(now_millis())
                .bind(&pool_id)
                .execute(&state.database)
                .await?;
            let result = reconcile_pool(&state, &user, &pool_id).await?;
            audit(
                &state,
                &user,
                "runner_pool.scaled",
                "runner_pool",
                Some(&pool_id),
                json!({ "desiredCount": desired }),
            )
            .await?;
            return Ok(Json(result));
        }
        _ => {
            return Err(ApiError::BadRequest(
                "Runner pool action is invalid.".into(),
            ));
        }
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_runner_pool(
    State(state): State<AppState>,
    Path(pool_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    pool_access(&state, &user, &pool_id).await?;
    assert_pool_admin(&state, &user, &pool_id).await?;
    sqlx::query("UPDATE runner_pools SET paused=1,state='deleting',updated_at=? WHERE id=?")
        .bind(now_millis())
        .bind(&pool_id)
        .execute(&state.database)
        .await?;
    let runners = runners_for_pool(&state, &pool_id).await?;
    for runner in &runners {
        delete_runner_resources(&state, &user, runner).await?;
    }
    audit(
        &state,
        &user,
        "runner_pool.deleted",
        "runner_pool",
        Some(&pool_id),
        json!({ "runners": runners.len() }),
    )
    .await?;
    sqlx::query("DELETE FROM runner_pools WHERE id=?")
        .bind(&pool_id)
        .execute(&state.database)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

pub(super) async fn provision(
    state: &AppState,
    user: &AuthUser,
    pool_id: &str,
    preferred_repository_id: Option<i64>,
) -> ApiResult<Value> {
    let pool = pool_access(state, user, pool_id).await?;
    assert_pool_admin(state, user, pool_id).await?;
    if pool.paused {
        return Err(ApiError::Conflict("Runner pool is paused.".into()));
    }
    if setting_bool(state, "provisioningPaused", false).await {
        return Err(ApiError::Conflict(
            "Runner provisioning is globally paused.".into(),
        ));
    }
    let runner_id = uuid::Uuid::new_v4().to_string();
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let runner_name = format!("{}-{suffix}", pool.name);
    let target_repository = if pool.scope == "repository" {
        let preferred = if let Some(repository_id) = preferred_repository_id {
            sqlx::query(
                r#"SELECT repo.id,repo.installation_id,repo.owner,repo.name FROM runner_pool_repositories membership
                   JOIN repositories repo ON repo.id=membership.repository_id
                   WHERE membership.pool_id=? AND membership.repository_id=? AND repo.archived=0"#,
            )
            .bind(pool_id)
            .bind(repository_id)
            .fetch_optional(&state.database)
            .await?
            .map(|row| RepositoryCapacity {
                repository_id: row.get("id"),
                installation_id: row.get("installation_id"),
                owner: row.get("owner"),
                name: row.get("name"),
                queued: 0,
                active: 0,
                busy: 0,
            })
        } else {
            None
        };
        Some(
            match preferred {
                Some(repository) => Some(repository),
                None => {
                    next_runner_repository(&state.database, pool_id, pool.queue_scale_factor)
                        .await?
                }
            }
            .ok_or_else(|| {
                ApiError::Conflict("Runner pool has no available repositories.".into())
            })?,
        )
    } else {
        None
    };
    let providers = json_array(&pool.providers);
    let labels = json_array(&pool.labels);
    let pending_bitbucket_connection = next_bitbucket_connection(state, pool_id).await?;
    let provider =
        if pending_bitbucket_connection.is_some() && providers.iter().any(|item| item == "tart") {
            "tart".to_owned()
        } else {
            next_runner_provider(
                &state.database,
                pool_id,
                target_repository
                    .as_ref()
                    .map(|repository| repository.repository_id),
                &providers,
                &labels,
                pool.queue_scale_factor,
            )
            .await?
            .ok_or_else(|| ApiError::Conflict("Runner pool has no configured providers.".into()))?
        };
    let bitbucket_connection = (provider == "tart")
        .then_some(pending_bitbucket_connection)
        .flatten();
    let plan = gridops_core::ProvisioningPlan::for_selected_provider(
        &provider,
        bitbucket_connection.is_some(),
        &pool.mode,
        pool.ephemeral,
        &pool.macos_runtime,
        &pool.docker_image,
        &pool.tart_image,
    );
    let platform = plan.platform;
    let runner_mode = plan.mode.as_str();
    let runner_ephemeral = plan.ephemeral;
    let runtime = plan.runtime.as_str();
    let image = plan.image.as_str();
    let capacity_lease = reserve_runner_capacity(
        state,
        &runner_id,
        pool_id,
        &provider,
        pool.cpu_limit,
        pool.memory_limit_mb,
    )
    .await?;
    let now = now_millis();
    if let Err(error) = sqlx::query("INSERT INTO runners (id,pool_id,target_repository_id,name,provider,ci_platform,bitbucket_connection_id,status,ephemeral,configuration_version,runtime,created_at,updated_at) VALUES (?,?,?,?,?,?,?,'starting',?,?,?,?,?)")
        .bind(&runner_id).bind(pool_id)
        .bind((platform == "github").then(|| target_repository.as_ref().map(|repository| repository.repository_id)).flatten())
        .bind(&runner_name).bind(&provider).bind(platform).bind(bitbucket_connection.as_ref().map(|connection| &connection.id)).bind(runner_ephemeral)
        .bind(pool.configuration_version).bind(runtime).bind(now).bind(now).execute(&state.database).await
    {
        if gridops_core::failure_cleanup(true, false, false).release_capacity {
            release_runner_capacity(state, &capacity_lease).await;
        }
        return Err(error.into());
    }
    let result = async {
        let mut request = json!({
            "runnerId": runner_id, "poolId": pool_id, "name": runner_name, "image": image,
            "mode": runner_mode, "provider": provider, "runtime": runtime, "platform": platform, "labels": &labels, "cpuLimit": pool.cpu_limit,
            "memoryLimitMb": pool.memory_limit_mb, "network": state.config.runner_network(),
            "capacityLease": &capacity_lease,
            "pullImage": setting_bool(state, "autoUpdateImages", false).await,
        });
        let (github_runner_id, bitbucket_runner_uuid) = if let Some(connection) = &bitbucket_connection {
            let access_token = state
                .runtime_secret(&connection.access_token_key)
                .await
                .map_err(ApiError::Internal)?
                .ok_or_else(|| ApiError::Conflict("Bitbucket workspace credentials are unavailable.".into()))?;
            let bitbucket_labels = bitbucket_runner_labels(&pool.name, &labels)?;
            let runner = state
                .bitbucket
                .create_runner(
                    &gridops_core::BitbucketRunnerTarget::Workspace { workspace: connection.workspace.clone() },
                    &access_token,
                    &runner_name,
                    &bitbucket_labels,
                )
                .await
                .map_err(ApiError::Internal)?;
            let oauth_secret = runner.oauth_client.secret.ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!("Bitbucket did not return the runner OAuth secret."))
            })?;
            request["bitbucket"] = json!({
                "accountUuid": connection.workspace_uuid,
                "runnerUuid": runner.uuid,
                "oauthClientId": runner.oauth_client.id,
            });
            request["bitbucketOauthClientSecret"] = Value::String(oauth_secret);
            (None, Some(runner.uuid))
        } else {
            let target_installation_id = target_repository
                .as_ref()
                .map_or(pool.installation_id, |repository| repository.installation_id);
            let token = control_token(state, &user.id, target_installation_id).await?;
            let target = match &target_repository {
                Some(repository) => RunnerTarget::Repository {
                    owner: &repository.owner,
                    repository: &repository.name,
                },
                None => RunnerTarget::Organization { organization: &pool.account_login },
            };
            if pool.ephemeral {
                let jit = state.github.generate_jit_config(target, &token, &JitRequest {
                    name: runner_name.clone(), runner_group_id: pool.runner_group_id,
                    labels: effective_runner_labels(&provider, &labels), work_folder: "_work".into(),
                }).await.map_err(ApiError::Internal)?;
                request["jitConfig"] = Value::String(jit.encoded_jit_config);
                (Some(jit.runner.id), None)
            } else {
                let registration = state.github.generate_registration_token(target, &token)
                    .await.map_err(ApiError::Internal)?;
                request["registrationToken"] = Value::String(registration.token);
                request["registrationUrl"] = Value::String(runner_registration_url(
                    &pool.account_login,
                    target_repository.as_ref(),
                ));
                if pool.scope == "organization" && pool.runner_group_id != 1 {
                    let group = state.github.runner_group_name(
                        &pool.account_login, pool.runner_group_id, &token,
                    ).await.map_err(ApiError::Internal)?;
                    request["runnerGroup"] = Value::String(group);
                }
                (None, None)
            }
        };
        if let Some(github_runner_id) = github_runner_id {
            sqlx::query("UPDATE runners SET github_runner_id=?,updated_at=? WHERE id=?")
                .bind(github_runner_id)
                .bind(now_millis())
                .bind(&runner_id)
                .execute(&state.database)
                .await?;
        }
        let manager = match manager_json(state, Method::POST, "v1/runners", Some(request)).await {
            Ok(manager) => manager,
            Err(error) => {
                if gridops_core::failure_cleanup(true, bitbucket_runner_uuid.is_some(), false)
                    .remove_provider_runner
                    && let (Some(connection), Some(bitbucket_runner_uuid)) = (&bitbucket_connection, &bitbucket_runner_uuid)
                    && let Ok(Some(access_token)) = state.runtime_secret(&connection.access_token_key).await
                {
                    let _ = state.bitbucket.delete_runner(
                        &gridops_core::BitbucketRunnerTarget::Workspace { workspace: connection.workspace.clone() },
                        &access_token,
                        bitbucket_runner_uuid,
                    ).await;
                }
                return Err(error);
            }
        };
        let container_id = manager.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::ServiceUnavailable("Runner manager returned an invalid container identifier.".into()))?;
        let status = if manager.get("state").and_then(Value::as_str) == Some("running") { "online" } else { "starting" };
        let updated = now_millis();
        sqlx::query("UPDATE runners SET github_runner_id=?,bitbucket_runner_uuid=?,container_id=?,container_name=?,status=?,registered_at=?,last_heartbeat_at=?,updated_at=? WHERE id=?")
            .bind(github_runner_id).bind(&bitbucket_runner_uuid).bind(container_id).bind(manager.get("name").and_then(Value::as_str)).bind(status)
            .bind(updated).bind(updated).bind(updated).bind(&runner_id).execute(&state.database).await?;
        sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,event,message,metadata,created_at) VALUES (?,?,?,'Runner started',?,?,?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&runner_id).bind(pool_id).bind(format!("{runner_name} started in pool {}", pool.name))
            .bind(json!({ "containerId": container_id, "platform": platform, "githubRunnerId": github_runner_id, "bitbucketRunnerUuid": bitbucket_runner_uuid, "mode": runner_mode, "provider": provider, "repositoryId": (platform == "github").then(|| target_repository.as_ref().map(|repository| repository.repository_id)).flatten() }).to_string()).bind(updated).execute(&state.database).await?;
        audit(state, user, "runner.provisioned", "runner", Some(&runner_id), json!({ "poolId": pool_id, "containerId": container_id, "platform": platform, "githubRunnerId": github_runner_id, "bitbucketRunnerUuid": bitbucket_runner_uuid, "mode": runner_mode, "provider": provider, "repositoryId": (platform == "github").then(|| target_repository.as_ref().map(|repository| repository.repository_id)).flatten() })).await?;
        Ok::<Value, ApiError>(json!({ "runnerId": runner_id, "status": status }))
    }.await;
    if let Err(error) = &result {
        if gridops_core::failure_cleanup(true, false, false).release_capacity {
            release_runner_capacity(state, &capacity_lease).await;
        }
        let message = error.to_string().chars().take(2_000).collect::<String>();
        sqlx::query("UPDATE runners SET status='failed',failure_reason=?,updated_at=? WHERE id=?")
            .bind(&message)
            .bind(now_millis())
            .bind(&runner_id)
            .execute(&state.database)
            .await?;
        sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,level,event,message,created_at) VALUES (?,?,?,'error','Runner provisioning failed',?,?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&runner_id).bind(pool_id).bind(message).bind(now_millis()).execute(&state.database).await?;
    }
    result
}

async fn set_pool_paused(
    state: &AppState,
    user: &AuthUser,
    pool_id: &str,
    paused: bool,
) -> ApiResult<()> {
    pool_access(state, user, pool_id).await?;
    assert_pool_admin(state, user, pool_id).await?;
    sqlx::query("UPDATE runner_pools SET paused=?,state=?,updated_at=? WHERE id=?")
        .bind(paused)
        .bind(if paused { "draining" } else { "active" })
        .bind(now_millis())
        .bind(pool_id)
        .execute(&state.database)
        .await?;
    if !paused {
        sqlx::query("UPDATE runner_pools SET provision_failure_count=0,provision_retry_at=NULL,provision_circuit_open=0 WHERE id=?")
            .bind(pool_id)
            .execute(&state.database)
            .await?;
    }
    if paused {
        for runner in runners_for_pool(state, pool_id)
            .await?
            .iter()
            .filter(|runner| !runner.busy)
        {
            delete_runner_resources(state, user, runner).await?;
        }
    } else {
        reconcile_pool(state, user, pool_id).await?;
    }
    audit(
        state,
        user,
        if paused {
            "runner_pool.paused"
        } else {
            "runner_pool.resumed"
        },
        "runner_pool",
        Some(pool_id),
        json!({}),
    )
    .await?;
    Ok(())
}

async fn reconcile_pool(state: &AppState, user: &AuthUser, pool_id: &str) -> ApiResult<Value> {
    let pool = pool_access(state, user, pool_id).await?;
    assert_pool_admin(state, user, pool_id).await?;
    let known = runners_for_pool(state, pool_id).await?;
    let managed = match manager_json(state, Method::GET, "v1/runners", None).await {
        Ok(value) => value,
        Err(error) if known.iter().all(|runner| runner.container_id.is_none()) => {
            tracing::warn!(pool_id, error = ?error, "runner manager unavailable before first provision");
            json!({ "runners": [] })
        }
        Err(error) => return Err(error),
    };
    let states = managed
        .get("runners")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|runner| {
            Some((
                runner.get("id")?.as_str()?.to_owned(),
                runner.get("state")?.as_str()?.to_owned(),
            ))
        })
        .collect::<HashMap<_, _>>();
    for runner in &known {
        if let Some(container_id) = &runner.container_id {
            let docker_state = states
                .get(container_id)
                .map(String::as_str)
                .unwrap_or("missing");
            let status = match docker_state {
                "running" if runner.busy => "busy",
                "running" => "online",
                "paused" => "paused",
                "exited" | "dead" if runner.runner_status == "stopped" => "stopped",
                "exited" | "dead" | "missing" => "failed",
                other => other,
            };
            let heartbeat = now_millis();
            sqlx::query(
                "UPDATE runners SET status=?,last_heartbeat_at=?,updated_at=CASE WHEN status<>? THEN ? ELSE updated_at END WHERE id=?",
            )
                .bind(status)
                .bind(heartbeat)
                .bind(status)
                .bind(heartbeat)
                .bind(&runner.runner_id)
                .execute(&state.database)
                .await?;
        }
    }
    let failed = runners_for_pool(state, pool_id)
        .await?
        .into_iter()
        .filter(|runner| runner.runner_status == "failed")
        .collect::<Vec<_>>();
    for runner in &failed {
        delete_runner_resources(state, user, runner).await?;
    }
    let mut rotated = 0;
    if let Some(stale) = runners_for_pool(state, pool_id)
        .await?
        .into_iter()
        .find(|runner| {
            runner.ci_platform != "bitbucket"
                && !runner.busy
                && runner.configuration_version < pool.configuration_version
        })
    {
        delete_runner_resources(state, user, &stale).await?;
        rotated = 1;
    }
    if pool.paused {
        let idle = runners_for_pool(state, pool_id)
            .await?
            .into_iter()
            .filter(|runner| !runner.busy && active_status(&runner.runner_status))
            .collect::<Vec<_>>();
        let removed = idle.len();
        for runner in &idle {
            delete_runner_resources(state, user, runner).await?;
        }
        let active = runners_for_pool(state, pool_id)
            .await?
            .into_iter()
            .filter(|runner| active_status(&runner.runner_status))
            .count();
        sqlx::query("UPDATE runner_pools SET state=?,updated_at=? WHERE id=?")
            .bind(if active == 0 { "paused" } else { "draining" })
            .bind(now_millis())
            .bind(pool_id)
            .execute(&state.database)
            .await?;
        return Ok(json!({
            "ok": true, "desired": pool.desired_count, "active": active,
            "provisioned": 0, "removed": removed,
        }));
    }
    let refreshed = runners_for_pool(state, pool_id).await?;
    let active = refreshed
        .iter()
        .filter(|runner| active_status(&runner.runner_status))
        .collect::<Vec<_>>();
    let mut provisioned = 0;
    let mut removed = 0;
    if active.len() < pool.desired_count as usize {
        for _ in active.len()..pool.desired_count as usize {
            provision(state, user, pool_id, None).await?;
            provisioned += 1;
        }
    } else if active.len() > pool.desired_count as usize {
        let count = active.len() - pool.desired_count as usize;
        for runner in active
            .into_iter()
            .filter(|runner| runner.ci_platform != "bitbucket" && !runner.busy)
            .take(count)
        {
            delete_runner_resources(state, user, runner).await?;
            removed += 1;
        }
    }
    let final_runners = runners_for_pool(state, pool_id).await?;
    let final_active = final_runners
        .iter()
        .filter(|runner| active_status(&runner.runner_status))
        .count();
    let outdated = final_runners
        .iter()
        .filter(|runner| runner.configuration_version < pool.configuration_version)
        .count();
    sqlx::query("UPDATE runner_pools SET state=?,updated_at=? WHERE id=?")
        .bind(if outdated > 0 {
            "updating"
        } else if final_active > pool.desired_count as usize {
            "draining"
        } else {
            "active"
        })
        .bind(now_millis())
        .bind(pool_id)
        .execute(&state.database)
        .await?;
    Ok(
        json!({ "ok": true, "desired": pool.desired_count, "active": final_active, "provisioned": provisioned, "removed": removed, "rotated": rotated, "outdated": outdated }),
    )
}

pub(super) async fn delete_runner_resources(
    state: &AppState,
    user: &AuthUser,
    runner: &RunnerAccess,
) -> ApiResult<()> {
    if runner.container_id.is_some()
        && let Err(error) = archive_runner_logs(state, runner).await
    {
        tracing::warn!(runner_id = %runner.runner_id, error = ?error, "could not archive runner logs");
    }
    let github_cleanup = if runner.ci_platform == "bitbucket" {
        cleanup_bitbucket_runner(state, runner).await
    } else {
        cleanup_github_runner(state, user, runner).await
    };
    let github_cleanup_error = github_cleanup
        .err()
        .map(|error| error.to_string().chars().take(2_000).collect::<String>());
    if let Some(error) = &github_cleanup_error {
        if runner.ci_platform == "bitbucket" {
            return Err(ApiError::ServiceUnavailable(format!(
                "Bitbucket runner cleanup failed; the local runner was left intact: {error}"
            )));
        }
        tracing::warn!(runner_id = %runner.runner_id, error, "GitHub runner cleanup deferred");
        let now = now_millis();
        sqlx::query(
            r#"INSERT INTO github_runner_cleanup (
              id,installation_id,target_owner,target_repository,github_runner_id,runner_name,
              attempts,last_error,next_attempt_at,created_at,updated_at
            ) VALUES (?,?,?,?,?,?,0,?,?,?,?) ON CONFLICT(id) DO UPDATE SET
              installation_id=excluded.installation_id,target_owner=excluded.target_owner,
              target_repository=excluded.target_repository,
              github_runner_id=COALESCE(excluded.github_runner_id,github_runner_cleanup.github_runner_id),
              runner_name=excluded.runner_name,last_error=excluded.last_error,
              next_attempt_at=excluded.next_attempt_at,updated_at=excluded.updated_at"#,
        )
        .bind(&runner.runner_id)
        .bind(runner.installation_id)
        .bind(
            runner
                .repository_owner
                .as_deref()
                .unwrap_or(&runner.account_login),
        )
        .bind(&runner.repository_name)
        .bind(runner.github_runner_id)
        .bind(&runner.runner_name)
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&state.database)
        .await?;
    } else {
        sqlx::query("DELETE FROM github_runner_cleanup WHERE id=?")
            .bind(&runner.runner_id)
            .execute(&state.database)
            .await?;
    }
    if let Some(container_id) = &runner.container_id {
        if let Err(error) = manager_json(
            state,
            Method::DELETE,
            &format!("v1/runners/{container_id}"),
            None,
        )
        .await
        {
            if !matches!(error, ApiError::NotFound(_)) {
                return Err(error);
            }
        }
    }
    let now = now_millis();
    sqlx::query("UPDATE runners SET status='deleted',busy=0,deleted_at=?,updated_at=? WHERE id=?")
        .bind(now)
        .bind(now)
        .bind(&runner.runner_id)
        .execute(&state.database)
        .await?;
    sqlx::query("INSERT INTO runner_events (id,runner_id,pool_id,level,event,message,metadata,created_at) VALUES (?,?,?,?, 'Runner deleted',?,?,?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(&runner.runner_id).bind(&runner.pool_id)
        .bind(if github_cleanup_error.is_some() { "warn" } else { "info" })
        .bind(if github_cleanup_error.is_some() {
            format!("{} was removed locally; GitHub cleanup will retry", runner.runner_name)
        } else {
            format!("{} was removed", runner.runner_name)
        })
        .bind(json!({
            "githubCleanup": if github_cleanup_error.is_some() { "deferred" } else { "complete" },
            "error": github_cleanup_error,
        }).to_string())
        .bind(now).execute(&state.database).await?;
    Ok(())
}

async fn cleanup_github_runner(
    state: &AppState,
    user: &AuthUser,
    runner: &RunnerAccess,
) -> ApiResult<()> {
    let token = control_token(state, &user.id, runner.installation_id).await?;
    let target = match (&runner.repository_owner, &runner.repository_name) {
        (Some(owner), Some(repository)) => RunnerTarget::Repository { owner, repository },
        _ => RunnerTarget::Organization {
            organization: &runner.account_login,
        },
    };
    let github_runner_id = match runner.github_runner_id {
        Some(id) => Some(id),
        None => state
            .github
            .runner_by_name(target, &token, &runner.runner_name)
            .await
            .map_err(ApiError::Internal)?
            .map(|runner| runner.id),
    };
    if let Some(github_runner_id) = github_runner_id {
        let path = match (&runner.repository_owner, &runner.repository_name) {
            (Some(owner), Some(repository)) => {
                format!("/repos/{owner}/{repository}/actions/runners/{github_runner_id}")
            }
            _ => format!(
                "/orgs/{}/actions/runners/{github_runner_id}",
                runner.account_login
            ),
        };
        state
            .github
            .delete(&path, &token)
            .await
            .map_err(ApiError::Internal)?;
    }
    Ok(())
}

async fn cleanup_bitbucket_runner(state: &AppState, runner: &RunnerAccess) -> ApiResult<()> {
    let connection_id = runner.bitbucket_connection_id.as_deref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "Bitbucket runner is missing its connection."
        ))
    })?;
    let runner_uuid = runner.bitbucket_runner_uuid.as_deref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "Bitbucket runner is missing its remote identity."
        ))
    })?;
    let connection = sqlx::query_as::<_, BitbucketPoolConnection>(
        "SELECT id,workspace,workspace_uuid,access_token_key FROM bitbucket_connections WHERE id=?",
    )
    .bind(connection_id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("Bitbucket workspace connection no longer exists.".into()))?;
    let access_token = state
        .runtime_secret(&connection.access_token_key)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!(
                "Bitbucket workspace credentials are unavailable."
            ))
        })?;
    state
        .bitbucket
        .delete_runner(
            &gridops_core::BitbucketRunnerTarget::Workspace {
                workspace: connection.workspace,
            },
            &access_token,
            runner_uuid,
        )
        .await
        .map_err(ApiError::Internal)
}

async fn pool_access(state: &AppState, user: &AuthUser, pool_id: &str) -> ApiResult<PoolAccess> {
    sqlx::query_as::<_, PoolAccess>(
        r#"SELECT p.installation_id,
      CASE WHEN NOT EXISTS (
        SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
          AND NOT EXISTS (SELECT 1 FROM user_installations manage
            WHERE manage.user_id=? AND manage.installation_id=mapped.installation_id
              AND manage.permission='admin')
      ) THEN 'admin' ELSE 'read' END AS installation_permission,
      i.account_login,
      p.repository_id,repo.owner AS repository_owner,repo.name AS repository_name,p.name,p.scope,
      p.mode,p.provider,p.providers,p.labels,p.image,p.docker_image,p.tart_image,p.macos_runtime,
      p.desired_count,p.min_count,p.max_count,
      CAST(p.cpu_limit AS REAL) AS cpu_limit,p.memory_limit_mb,
      p.runner_group_id,p.ephemeral,p.paused,p.state,p.autoscaling_enabled,p.queue_scale_factor,
      p.idle_timeout_minutes,p.configuration_version,p.provision_failure_count,
      p.provision_retry_at,p.provision_circuit_open
      FROM runner_pools p JOIN installations i ON i.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=p.repository_id
      WHERE p.id=?
        AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
        AND NOT EXISTS (
          SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
            AND NOT EXISTS (SELECT 1 FROM user_installations access
              WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
        )"#,
    )
    .bind(&user.id)
    .bind(pool_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("Runner pool does not exist or is not accessible.".into()))
}

pub(super) async fn runner_access(
    state: &AppState,
    user: &AuthUser,
    runner_id: &str,
) -> ApiResult<RunnerAccess> {
    sqlx::query_as::<_, RunnerAccess>(r#"SELECT r.id AS runner_id,r.name AS runner_name,r.container_id,
      r.github_runner_id,r.ci_platform,r.bitbucket_connection_id,r.bitbucket_runner_uuid,r.provider,r.status AS runner_status,r.busy,r.ephemeral,r.configuration_version,
      r.last_job_id,p.id AS pool_id,p.name AS pool_name,
      COALESCE(repo.installation_id,p.installation_id) AS installation_id,
      COALESCE(target_installation.account_login,primary_installation.account_login) AS account_login,
      r.target_repository_id,
      repo.owner AS repository_owner,repo.name AS repository_name FROM runners r
      JOIN runner_pools p ON p.id=r.pool_id
      JOIN installations primary_installation ON primary_installation.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=r.target_repository_id
      LEFT JOIN installations target_installation ON target_installation.id=repo.installation_id
      WHERE r.id=? AND r.deleted_at IS NULL
        AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
        AND NOT EXISTS (
          SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
            AND NOT EXISTS (SELECT 1 FROM user_installations access
              WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
        )"#)
        .bind(runner_id).bind(&user.id).fetch_optional(&state.database).await?
        .ok_or_else(|| ApiError::NotFound("Runner does not exist or is not accessible.".into()))
}

async fn runners_for_pool(state: &AppState, pool_id: &str) -> ApiResult<Vec<RunnerAccess>> {
    Ok(sqlx::query_as::<_, RunnerAccess>(r#"SELECT r.id AS runner_id,r.name AS runner_name,r.container_id,
      r.github_runner_id,r.ci_platform,r.bitbucket_connection_id,r.bitbucket_runner_uuid,r.provider,r.status AS runner_status,r.busy,r.ephemeral,r.configuration_version,
      r.last_job_id,p.id AS pool_id,p.name AS pool_name,
      COALESCE(repo.installation_id,p.installation_id) AS installation_id,
      COALESCE(target_installation.account_login,primary_installation.account_login) AS account_login,
      r.target_repository_id,
      repo.owner AS repository_owner,repo.name AS repository_name FROM runners r
      JOIN runner_pools p ON p.id=r.pool_id
      JOIN installations primary_installation ON primary_installation.id=p.installation_id
      LEFT JOIN repositories repo ON repo.id=r.target_repository_id
      LEFT JOIN installations target_installation ON target_installation.id=repo.installation_id
      WHERE p.id=? AND r.deleted_at IS NULL ORDER BY r.created_at DESC"#)
        .bind(pool_id).fetch_all(&state.database).await?)
}
