//! Runner inventory and lifecycle API handlers.

use super::runner_pools::{delete_runner_resources, provision, runner_access};
use super::*;

pub async fn runners(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM runners r JOIN runner_pools p ON p.id=r.pool_id
        WHERE r.deleted_at IS NULL
          AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
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
        r#"SELECT r.id,r.name,r.provider,r.ci_platform,r.status,r.busy,r.ephemeral,r.os,r.architecture,r.container_id,
          r.github_runner_id,r.bitbucket_runner_uuid,r.failure_reason,r.registered_at,r.last_heartbeat_at,r.created_at,
          p.id AS pool_id,p.name AS pool_name,p.paused AS pool_paused,
          COALESCE(target_installation.account_login,primary_installation.account_login) AS account_login,
          CASE WHEN NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations manage
                WHERE manage.user_id=? AND manage.installation_id=mapped.installation_id
                  AND manage.permission='admin')
          ) THEN 'admin' ELSE 'read' END AS installation_permission,
          repo.full_name AS repository,wj.name AS current_job_name,wj.run_id AS current_run_id
        FROM runners r JOIN runner_pools p ON p.id=r.pool_id
        JOIN installations primary_installation ON primary_installation.id=p.installation_id
        LEFT JOIN repositories repo ON repo.id=r.target_repository_id
        LEFT JOIN installations target_installation ON target_installation.id=repo.installation_id
        LEFT JOIN workflow_jobs wj ON wj.id=r.current_job_id
        WHERE r.deleted_at IS NULL
          AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
        ORDER BY r.created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "name": row.get::<String,_>("name"), "status": row.get::<String,_>("status"),
        "provider": row.get::<String,_>("provider"), "platform": row.get::<String,_>("ci_platform"), "busy": row.get::<bool,_>("busy"), "ephemeral": row.get::<bool,_>("ephemeral"), "os": row.get::<String,_>("os"),
        "architecture": row.get::<String,_>("architecture"), "containerId": row.try_get::<Option<String>,_>("container_id").ok().flatten(),
        "githubRunnerId": row.try_get::<Option<i64>,_>("github_runner_id").ok().flatten(),
        "bitbucketRunnerUuid": row.try_get::<Option<String>,_>("bitbucket_runner_uuid").ok().flatten(),
        "failureReason": row.try_get::<Option<String>,_>("failure_reason").ok().flatten(),
        "registeredAt": iso_optional(row.try_get::<Option<i64>,_>("registered_at").ok().flatten()),
        "lastHeartbeatAt": iso_optional(row.try_get::<Option<i64>,_>("last_heartbeat_at").ok().flatten()), "createdAt": iso(row.get::<i64,_>("created_at")),
        "poolId": row.get::<String,_>("pool_id"), "poolName": row.get::<String,_>("pool_name"), "poolPaused": row.get::<bool,_>("pool_paused"),
        "accountLogin": row.get::<String,_>("account_login"), "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(),
        "currentJobName": row.try_get::<Option<String>,_>("current_job_name").ok().flatten(), "currentRunId": row.try_get::<Option<i64>,_>("current_run_id").ok().flatten(),
        "canManage": user.role == "admin" || row.get::<String,_>("installation_permission") == "admin",
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn runner_action(
    State(state): State<AppState>,
    Path(runner_id): Path<String>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<RunnerAction>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    let runner = runner_access(&state, &user, &runner_id).await?;
    assert_installation_admin(&state, &user, runner.installation_id).await?;
    match input.action.as_str() {
        "delete" => delete_runner_resources(&state, &user, &runner).await?,
        "rebuild" => {
            delete_runner_resources(&state, &user, &runner).await?;
            provision(&state, &user, &runner.pool_id, runner.target_repository_id).await?;
        }
        "start" | "stop" | "pause" | "resume" | "restart" => {
            if runner.ephemeral && matches!(input.action.as_str(), "start" | "restart") {
                return Err(ApiError::Conflict(
                    "Ephemeral runners cannot be started or restarted; rebuild the runner instead."
                        .into(),
                ));
            }
            let container_id = runner
                .container_id
                .as_deref()
                .ok_or_else(|| ApiError::Conflict("Runner has no managed container.".into()))?;
            manager_json(
                &state,
                Method::POST,
                &format!("v1/runners/{container_id}/{}", input.action),
                None,
            )
            .await?;
            let status = match input.action.as_str() {
                "stop" => "stopped",
                "pause" => "paused",
                _ => "online",
            };
            sqlx::query("UPDATE runners SET status=?,updated_at=? WHERE id=?")
                .bind(status)
                .bind(now_millis())
                .bind(&runner_id)
                .execute(&state.database)
                .await?;
        }
        _ => return Err(ApiError::BadRequest("Runner action is invalid.".into())),
    }
    audit(
        &state,
        &user,
        &format!("runner.{}", input.action),
        "runner",
        Some(&runner_id),
        json!({ "poolId": runner.pool_id }),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}
