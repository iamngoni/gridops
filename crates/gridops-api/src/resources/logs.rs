//! Live, archived, and structured workflow log API handlers.

use super::runner_pools::runner_access;
use super::*;

pub async fn runner_logs(
    State(state): State<AppState>,
    Path(runner_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let runner = runner_access(&state, &user, &runner_id).await?;
    let container_id = runner
        .container_id
        .ok_or_else(|| ApiError::Conflict("Runner has no active container log stream.".into()))?;
    let logs = manager_text(&state, &format!("v1/runners/{container_id}/logs")).await?;
    Ok(Json(
        json!({ "runnerId": runner_id, "name": runner.runner_name, "logs": logs }),
    ))
}

pub async fn runner_log_stream(
    State(state): State<AppState>,
    Path(runner_id): Path<String>,
    Query(query): Query<LogStreamQuery>,
    user: AuthUser,
) -> ApiResult<Response> {
    let runner = runner_access(&state, &user, &runner_id).await?;
    let container_id = runner
        .container_id
        .ok_or_else(|| ApiError::Conflict("Runner has no active container log stream.".into()))?;
    let tail = query.tail.as_deref().unwrap_or("500");
    if tail != "0"
        && !tail
            .parse::<u32>()
            .is_ok_and(|lines| (1..=5_000).contains(&lines))
    {
        return Err(ApiError::BadRequest(
            "Log stream tail must be from 0 to 5000.".into(),
        ));
    }
    let response = manager_get(
        &state,
        &format!("v1/runners/{container_id}/logs?follow=true&tail={tail}"),
    )
    .await?;
    let stream = response
        .bytes_stream()
        .take_until(tokio::time::sleep(Duration::from_secs(25)))
        .map_err(std::io::Error::other);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub async fn archived_logs(
    State(state): State<AppState>,
    Path(stream_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        r#"SELECT ls.path,ls.runner_name FROM log_streams ls
          JOIN user_installations ui ON ui.installation_id=ls.installation_id
          WHERE ls.id=? AND ui.user_id=? AND ls.complete=1"#,
    )
    .bind(&stream_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("Archived runner log does not exist.".into()))?;
    let filename = row.get::<String, _>("path");
    let path = safe_log_path(&state, &filename)?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let size = file
        .metadata()
        .await
        .map_err(|error| ApiError::Internal(error.into()))?
        .len();
    let start = size.saturating_sub(MAX_ARCHIVED_LOG_VIEW_BYTES);
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let mut bytes = Vec::with_capacity((size - start) as usize);
    file.read_to_end(&mut bytes)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let prefix = if start > 0 {
        "[GridOps is showing the final 1 MB of this retained log.]\n"
    } else {
        ""
    };
    Ok(Json(json!({
        "streamId": stream_id,
        "name": row.try_get::<Option<String>,_>("runner_name").ok().flatten().unwrap_or_else(|| "Archived runner".into()),
        "logs": format!("{prefix}{}", String::from_utf8_lossy(&bytes)),
    })))
}

pub async fn workflow_run_logs(
    State(state): State<AppState>,
    Path(run_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Response> {
    let run = sqlx::query(
        r#"SELECT repo.owner,repo.name,repo.installation_id FROM workflow_runs wr
          JOIN repositories repo ON repo.id=wr.repository_id
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
    let owner = run.get::<String, _>("owner");
    let repository = run.get::<String, _>("name");
    let token = control_token(&state, &user.id, run.get("installation_id")).await?;
    let response = state
        .http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repository}/actions/runs/{run_id}/logs"
        ))
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(ApiError::ServiceUnavailable(format!(
            "GitHub log download failed ({status}): {}",
            detail.chars().take(300).collect::<String>()
        )));
    }
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{repository}-{run_id}-logs.zip\""),
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub async fn workflow_job_log_view(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        r#"SELECT job.id,job.run_id,job.name,job.status,job.conclusion,job.started_at,
          job.completed_at,run.run_number,run.workflow_name,repo.owner,repo.name AS repository_name,
          repo.full_name,repo.installation_id
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
    let owner = row.get::<String, _>("owner");
    let repository = row.get::<String, _>("repository_name");
    let installation_id = row.get::<i64, _>("installation_id");
    let token = control_token(&state, &user.id, installation_id).await?;
    let job_endpoint = format!("/repos/{owner}/{repository}/actions/jobs/{job_id}");
    let (remote_job_result, remote_log_result) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(8),
            state.github.get::<GitHubWorkflowJob>(&job_endpoint, &token),
        ),
        tokio::time::timeout(
            Duration::from_secs(8),
            github_job_log_text(&state, &owner, &repository, job_id, &token),
        ),
    );
    let remote_job = match remote_job_result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("GitHub job metadata request timed out")),
    };
    let metadata_warning = remote_job.as_ref().err().map(
        |_| "GitHub job metadata is temporarily unavailable; GridOps is using retained job state.",
    );
    let remote_log = remote_log_result.ok().and_then(Result::ok).flatten();
    let (raw_logs, source, truncated) = if let Some((logs, truncated)) = remote_log {
        (logs, "github", truncated)
    } else if let Some((logs, truncated)) = local_job_log_text(&state, &user, job_id).await? {
        (logs, "runner", truncated)
    } else {
        (String::new(), "pending", false)
    };
    let fallback_step;
    let (steps, status, conclusion, started_at, completed_at) = if let Ok(job) = &remote_job {
        (
            job.steps.as_slice(),
            job.status.as_str(),
            job.conclusion.as_deref(),
            job.started_at.as_deref(),
            job.completed_at.as_deref(),
        )
    } else {
        fallback_step = vec![GitHubWorkflowStep {
            name: row.get::<String, _>("name"),
            status: row.get::<String, _>("status"),
            conclusion: row.try_get::<Option<String>, _>("conclusion")?,
            number: 1,
            started_at: row.try_get::<Option<i64>, _>("started_at")?.map(iso),
            completed_at: row.try_get::<Option<i64>, _>("completed_at")?.map(iso),
        }];
        (
            fallback_step.as_slice(),
            row.get::<&str, _>("status"),
            row.try_get::<Option<&str>, _>("conclusion")?,
            None,
            None,
        )
    };
    let parsed = structure_job_log(&raw_logs, steps, source == "runner");
    let started_at = started_at.map(ToOwned::to_owned).or_else(|| {
        row.try_get::<Option<i64>, _>("started_at")
            .ok()
            .flatten()
            .map(iso)
    });
    let completed_at = completed_at.map(ToOwned::to_owned).or_else(|| {
        row.try_get::<Option<i64>, _>("completed_at")
            .ok()
            .flatten()
            .map(iso)
    });
    Ok(Json(json!({
        "id": job_id,
        "runId": row.get::<i64, _>("run_id"),
        "runNumber": row.get::<i64, _>("run_number"),
        "workflowName": row.get::<String, _>("workflow_name"),
        "repository": row.get::<String, _>("full_name"),
        "name": row.get::<String, _>("name"),
        "status": status,
        "conclusion": conclusion,
        "startedAt": started_at,
        "completedAt": completed_at,
        "source": source,
        "truncated": truncated,
        "metadataWarning": metadata_warning,
        "hiddenDiagnosticLines": parsed.hidden_diagnostic_lines,
        "lineCount": parsed.line_count,
        "annotations": parsed.annotations.iter().map(log_annotation_json).collect::<Vec<_>>(),
        "steps": parsed.steps.iter().map(structured_step_json).collect::<Vec<_>>(),
    })))
}

async fn github_job_log_text(
    state: &AppState,
    owner: &str,
    repository: &str,
    job_id: i64,
    token: &str,
) -> anyhow::Result<Option<(String, bool)>> {
    let response = state
        .http
        .get(format!(
            "https://api.github.com/repos/{owner}/{repository}/actions/jobs/{job_id}/logs"
        ))
        .bearer_auth(token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .send()
        .await?;
    if matches!(response.status().as_u16(), 404 | 409 | 410) {
        return Ok(None);
    }
    let response = response.error_for_status()?;
    let bytes = response.bytes().await?;
    let truncated = bytes.len() > MAX_STRUCTURED_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_STRUCTURED_LOG_BYTES);
    Ok(Some((
        String::from_utf8_lossy(&bytes[start..]).into_owned(),
        truncated,
    )))
}

async fn local_job_log_text(
    state: &AppState,
    user: &AuthUser,
    job_id: i64,
) -> ApiResult<Option<(String, bool)>> {
    let container_id = sqlx::query_scalar::<_, String>(
        r#"SELECT runner.container_id FROM runners runner
          JOIN runner_pools pool ON pool.id=runner.pool_id
          WHERE runner.deleted_at IS NULL AND runner.container_id IS NOT NULL
            AND (runner.current_job_id=? OR runner.last_job_id=?)
            AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=pool.id)
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=pool.id
                AND NOT EXISTS (SELECT 1 FROM user_installations access
                  WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
            )
          ORDER BY CASE WHEN runner.current_job_id=? THEN 0 ELSE 1 END,runner.updated_at DESC
          LIMIT 1"#,
    )
    .bind(job_id)
    .bind(job_id)
    .bind(&user.id)
    .bind(job_id)
    .fetch_optional(&state.database)
    .await?;
    if let Some(container_id) = container_id {
        let logs = manager_text(
            state,
            &format!("v1/runners/{container_id}/logs?tail=100000"),
        )
        .await?;
        return Ok(Some(truncate_log_text(logs)));
    }
    let row = sqlx::query(
        r#"SELECT stream.path FROM log_streams stream
          JOIN user_installations access ON access.installation_id=stream.installation_id
          WHERE stream.job_id=? AND stream.complete=1 AND access.user_id=?
          ORDER BY stream.created_at DESC LIMIT 1"#,
    )
    .bind(job_id)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let path = safe_log_path(state, row.get::<&str, _>("path"))?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    let truncated = bytes.len() > MAX_STRUCTURED_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_STRUCTURED_LOG_BYTES);
    Ok(Some((
        String::from_utf8_lossy(&bytes[start..]).into_owned(),
        truncated,
    )))
}

fn truncate_log_text(logs: String) -> (String, bool) {
    if logs.len() <= MAX_STRUCTURED_LOG_BYTES {
        return (logs, false);
    }
    let mut start = logs.len() - MAX_STRUCTURED_LOG_BYTES;
    while !logs.is_char_boundary(start) {
        start += 1;
    }
    (logs[start..].to_owned(), true)
}
