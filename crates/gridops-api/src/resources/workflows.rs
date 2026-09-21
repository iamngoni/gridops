//! Workflow-run inventory and action API handlers.

use super::*;

pub async fn workflow_runs(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM workflow_runs wr
        JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id AND ui.user_id=?"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT wr.id,wr.workflow_name,wr.run_number,wr.run_attempt,wr.event,wr.status,
          wr.conclusion,wr.head_branch,wr.head_sha,wr.actor_login,wr.html_url,wr.started_at,
          wr.completed_at,wr.github_created_at,repo.full_name,ui.permission AS installation_permission,
          COUNT(wj.id) AS job_count,COUNT(CASE WHEN wj.status='in_progress' THEN 1 END) AS active_jobs,
          COUNT(CASE WHEN wj.conclusion='failure' THEN 1 END) AS failed_jobs
        FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id AND ui.user_id=?
        LEFT JOIN workflow_jobs wj ON wj.run_id=wr.id GROUP BY wr.id
        ORDER BY wr.github_created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows
        .iter()
        .map(|row| workflow_run_json(row, user.role == "admin"))
        .collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn workflow_run(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let run = sqlx::query(
        r#"SELECT wr.id,wr.workflow_name,wr.run_number,wr.run_attempt,wr.event,wr.status,
          wr.conclusion,wr.head_branch,wr.head_sha,wr.actor_login,wr.html_url,wr.started_at,
          wr.completed_at,wr.github_created_at,repo.id AS repository_id,repo.installation_id,
          repo.full_name,ui.permission AS installation_permission
        FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id
        WHERE wr.id=? AND ui.user_id=?"#,
    )
    .bind(run_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Workflow run does not exist or is not accessible.".into())
    })?;
    let jobs = sqlx::query(
        r#"SELECT wj.id,wj.name,wj.status,wj.conclusion,wj.runner_name,wj.runner_group_name,
          wj.labels,wj.html_url,wj.started_at,wj.completed_at,
          (SELECT r.id FROM runners r WHERE r.current_job_id=wj.id AND r.deleted_at IS NULL
            ORDER BY r.updated_at DESC LIMIT 1) AS live_runner_id,
          (SELECT ls.id FROM log_streams ls WHERE ls.job_id=wj.id AND ls.complete=1
            ORDER BY ls.created_at DESC LIMIT 1) AS archived_log_id
          FROM workflow_jobs wj WHERE wj.run_id=? ORDER BY wj.created_at"#,
    )
    .bind(run_id)
    .fetch_all(&state.database)
    .await?;
    let mut job_items = Vec::with_capacity(jobs.len());
    for row in &jobs {
        let status = row.get::<String, _>("status");
        let labels = json_array(row.get::<&str, _>("labels"));
        let diagnosis = if status == "queued" {
            Some(
                diagnose_queued_job(
                    &state,
                    &user,
                    run.get::<i64, _>("repository_id"),
                    run.get::<i64, _>("installation_id"),
                    &labels,
                )
                .await?,
            )
        } else {
            None
        };
        job_items.push(json!({
            "id": row.get::<i64,_>("id"), "name": row.get::<String,_>("name"), "status": status,
            "conclusion": row.try_get::<Option<String>,_>("conclusion").ok().flatten(), "runnerName": row.try_get::<Option<String>,_>("runner_name").ok().flatten(),
            "runnerGroupName": row.try_get::<Option<String>,_>("runner_group_name").ok().flatten(), "labels": labels,
            "htmlUrl": row.get::<String,_>("html_url"), "startedAt": iso_optional(row.try_get::<Option<i64>,_>("started_at").ok().flatten()),
            "completedAt": iso_optional(row.try_get::<Option<i64>,_>("completed_at").ok().flatten()),
            "liveRunnerId": row.try_get::<Option<String>,_>("live_runner_id").ok().flatten(),
            "archivedLogId": row.try_get::<Option<String>,_>("archived_log_id").ok().flatten(),
            "diagnosis": diagnosis,
        }));
    }
    Ok(Json(json!({
        "id": run.get::<i64,_>("id"), "workflowName": run.get::<String,_>("workflow_name"), "runNumber": run.get::<i64,_>("run_number"),
        "runAttempt": run.get::<i64,_>("run_attempt"), "event": run.get::<String,_>("event"), "status": run.get::<String,_>("status"),
        "conclusion": run.try_get::<Option<String>,_>("conclusion").ok().flatten(), "headBranch": run.try_get::<Option<String>,_>("head_branch").ok().flatten(),
        "headSha": run.get::<String,_>("head_sha"), "actorLogin": run.try_get::<Option<String>,_>("actor_login").ok().flatten(),
        "htmlUrl": run.get::<String,_>("html_url"), "startedAt": iso_optional(run.try_get::<Option<i64>,_>("started_at").ok().flatten()),
        "completedAt": iso_optional(run.try_get::<Option<i64>,_>("completed_at").ok().flatten()), "createdAt": iso(run.get::<i64,_>("github_created_at")),
        "repository": run.get::<String,_>("full_name"), "jobs": job_items,
        "canManage": user.role == "admin" || run.get::<String,_>("installation_permission") == "admin",
    })))
}

pub async fn workflow_run_action(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    headers: HeaderMap,
    user: AuthUser,
    Json(input): Json<WorkflowAction>,
) -> ApiResult<Json<Value>> {
    assert_same_origin(&state, &headers)?;
    let run = sqlx::query(
        r#"SELECT repo.owner,repo.name,repo.installation_id FROM workflow_runs wr
        JOIN repositories repo ON repo.id=wr.repository_id JOIN user_installations ui ON ui.installation_id=repo.installation_id
        WHERE wr.id=? AND ui.user_id=?"#,
    ).bind(run_id).bind(&user.id).fetch_optional(&state.database).await?
        .ok_or_else(|| ApiError::NotFound("Workflow run does not exist or is not accessible.".into()))?;
    let endpoint = workflow_action_endpoint(&input.action)?;
    assert_installation_admin(&state, &user, run.get("installation_id")).await?;
    let token = control_token(&state, &user.id, run.get("installation_id")).await?;
    state
        .github
        .post_empty(
            &format!(
                "/repos/{}/{}/actions/runs/{run_id}/{endpoint}",
                run.get::<String, _>("owner"),
                run.get::<String, _>("name")
            ),
            &token,
            json!({}),
        )
        .await
        .map_err(ApiError::Internal)?;
    audit(
        &state,
        &user,
        &format!("workflow_run.{}", input.action),
        "workflow_run",
        Some(&run_id.to_string()),
        json!({}),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}
