//! Audit and archived-log listing API handlers.

use super::*;

pub async fn audit_events(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM audit_events WHERE actor_user_id=? OR actor_label='system'",
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT id,actor_label,action,target_type,target_id,metadata,ip_address,created_at
        FROM audit_events WHERE actor_user_id=? OR actor_label='system'
        ORDER BY created_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "actorLabel": row.get::<String,_>("actor_label"), "action": row.get::<String,_>("action"),
        "targetType": row.get::<String,_>("target_type"), "targetId": row.try_get::<Option<String>,_>("target_id").ok().flatten(),
        "metadata": row.get::<String,_>("metadata"), "ipAddress": row.try_get::<Option<String>,_>("ip_address").ok().flatten(),
        "createdAt": iso(row.get::<i64,_>("created_at")),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn log_targets(
    State(state): State<AppState>,
    Query(query): Query<LogTargetsQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT
          (SELECT COUNT(*) FROM runners r
            JOIN runner_pools p ON p.id=r.pool_id
            JOIN workflow_jobs job ON job.id=COALESCE(r.current_job_id,r.last_job_id)
            WHERE r.deleted_at IS NULL AND r.container_id IS NOT NULL
              AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
              AND NOT EXISTS (
                SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                  AND NOT EXISTS (SELECT 1 FROM user_installations access
                    WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
              ))
          +
          (SELECT COUNT(*) FROM log_streams ls JOIN user_installations ui
            ON ui.installation_id=ls.installation_id AND ui.user_id=?
            WHERE ls.complete=1 AND ls.job_id IS NOT NULL)"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT id,runner_id,name,status,busy,container_id,updated_at,pool_name,repository,
          kind,size_bytes,job_id,run_id,job_name,job_status,job_conclusion,run_number,workflow_name
        FROM (
          SELECT r.id,CAST(NULL AS TEXT) AS runner_id,r.name,r.status,r.busy,r.container_id,
            r.updated_at,p.name AS pool_name,repo.full_name AS repository,'live' AS kind,
            CAST(NULL AS INTEGER) AS size_bytes,job.id AS job_id,job.run_id,job.name AS job_name,
            job.status AS job_status,job.conclusion AS job_conclusion,run.run_number,run.workflow_name
          FROM runners r JOIN runner_pools p ON p.id=r.pool_id
          JOIN workflow_jobs job ON job.id=COALESCE(r.current_job_id,r.last_job_id)
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=run.repository_id
          WHERE r.deleted_at IS NULL AND r.container_id IS NOT NULL
            AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                AND NOT EXISTS (SELECT 1 FROM user_installations access
                  WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
            )
          UNION ALL
          SELECT ls.id,ls.runner_id,COALESCE(ls.runner_name,'Archived runner'),'archived',0,NULL,
            ls.created_at,COALESCE(ls.pool_name,'Deleted pool'),COALESCE(repo.full_name,ls.repository),
            'archive',ls.size_bytes,job.id,job.run_id,job.name,job.status,job.conclusion,
            run.run_number,run.workflow_name
          FROM log_streams ls JOIN user_installations ui
            ON ui.installation_id=ls.installation_id AND ui.user_id=?
          JOIN workflow_jobs job ON job.id=ls.job_id
          JOIN workflow_runs run ON run.id=job.run_id
          JOIN repositories repo ON repo.id=run.repository_id
          WHERE ls.complete=1 AND ls.job_id IS NOT NULL
        ) targets ORDER BY
          CASE WHEN id=? OR runner_id=? THEN 0 ELSE 1 END,
          CASE kind WHEN 'live' THEN 0 ELSE 1 END,busy DESC,updated_at DESC
        LIMIT ? OFFSET ?"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(query.target.as_deref())
    .bind(query.target.as_deref())
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "runnerId": row.try_get::<Option<String>,_>("runner_id").ok().flatten(),
        "name": row.get::<String,_>("name"), "status": row.get::<String,_>("status"),
        "busy": row.get::<bool,_>("busy"), "containerId": row.try_get::<Option<String>,_>("container_id").ok().flatten(),
        "updatedAt": iso(row.get::<i64,_>("updated_at")), "poolName": row.get::<String,_>("pool_name"),
        "repository": row.try_get::<Option<String>,_>("repository").ok().flatten(),
        "sizeBytes": row.try_get::<Option<i64>,_>("size_bytes").ok().flatten(), "kind": row.get::<String,_>("kind"),
        "jobId": row.get::<i64,_>("job_id"), "runId": row.get::<i64,_>("run_id"),
        "jobName": row.get::<String,_>("job_name"), "jobStatus": row.get::<String,_>("job_status"),
        "jobConclusion": row.try_get::<Option<String>,_>("job_conclusion").ok().flatten(),
        "runNumber": row.get::<i64,_>("run_number"), "workflowName": row.get::<String,_>("workflow_name"),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}
