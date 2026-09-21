//! Overview and capacity API handlers.

use super::*;

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.database).await?;
    Ok(Json(json!({
        "status": "ok",
        "service": "gridops-api",
        "database": "sqlite",
        "time": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    })))
}

pub async fn overview(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let configuration = configuration(&state).await?;
    let Some(user) = user else {
        return Ok(Json(json!({
            "authenticated": false,
            "configuration": configuration,
            "metrics": { "runners": 0, "online": 0, "busy": 0, "queuedJobs": 0, "successRate": null },
            "slo": {
              "windowHours": 24,
              "queue": { "oldestSeconds": null, "p95Seconds": null },
              "startLatency": { "sampleSize": 0, "p50Seconds": null, "p95Seconds": null },
              "failures": [], "alerts": [],
            },
            "pools": [], "runs": [], "activity": [], "installations": 0,
        })));
    };
    let metrics = sqlx::query(
        r#"SELECT
          COUNT(DISTINCT CASE WHEN r.deleted_at IS NULL THEN r.id END) AS runners,
          COUNT(DISTINCT CASE WHEN r.deleted_at IS NULL AND r.status IN ('idle','busy','online') THEN r.id END) AS online,
          COUNT(DISTINCT CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN r.id END) AS busy,
          (SELECT COUNT(*) FROM workflow_jobs wj JOIN workflow_runs wr ON wr.id=wj.run_id
            JOIN repositories repo ON repo.id=wr.repository_id
            JOIN user_installations ui2 ON ui2.installation_id=repo.installation_id
            WHERE ui2.user_id=? AND wj.status='queued') AS queued_jobs,
          (SELECT COUNT(*) FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
            JOIN user_installations ui3 ON ui3.installation_id=repo.installation_id
            WHERE ui3.user_id=? AND wr.completed_at IS NOT NULL) AS completed_runs,
          (SELECT COUNT(*) FROM workflow_runs wr JOIN repositories repo ON repo.id=wr.repository_id
            JOIN user_installations ui4 ON ui4.installation_id=repo.installation_id
            WHERE ui4.user_id=? AND wr.conclusion='success') AS successful_runs
        FROM runner_pools p
        LEFT JOIN runners r ON r.pool_id=p.id
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )"#,
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(&user.id)
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let completed = metrics.get::<i64, _>("completed_runs");
    let success_rate = (completed > 0).then(|| {
        ((metrics.get::<i64, _>("successful_runs") as f64 / completed as f64) * 1_000.0).round()
            / 10.0
    });
    let now = now_millis();
    let slo_rows = sqlx::query(
        r#"SELECT wj.status,wj.conclusion,wj.created_at,wj.started_at
          FROM workflow_jobs wj
          JOIN workflow_runs wr ON wr.id=wj.run_id
          JOIN repositories repo ON repo.id=wr.repository_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE ui.user_id=? AND wj.created_at>=?"#,
    )
    .bind(&user.id)
    .bind(now.saturating_sub(24 * 60 * 60 * 1_000))
    .fetch_all(&state.database)
    .await?;
    let mut queue_ages = Vec::new();
    let mut start_latencies = Vec::new();
    let mut failure_reasons = HashMap::<String, i64>::new();
    for row in &slo_rows {
        let created_at = row.get::<i64, _>("created_at");
        if row.get::<String, _>("status") == "queued" {
            queue_ages.push(now.saturating_sub(created_at));
        }
        if let Some(started_at) = row.try_get::<Option<i64>, _>("started_at").ok().flatten()
            && started_at >= created_at
        {
            start_latencies.push(started_at.saturating_sub(created_at));
        }
        if let Some(conclusion) = row
            .try_get::<Option<String>, _>("conclusion")
            .ok()
            .flatten()
            && conclusion != "success"
            && conclusion != "skipped"
        {
            *failure_reasons.entry(conclusion).or_default() += 1;
        }
    }
    let oldest_queue_age_ms = queue_ages.iter().copied().max();
    let queue_p95_ms = percentile_millis(&queue_ages, 95);
    let start_p50_ms = percentile_millis(&start_latencies, 50);
    let start_p95_ms = percentile_millis(&start_latencies, 95);
    let mut failures = failure_reasons.into_iter().collect::<Vec<_>>();
    failures.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    failures.truncate(3);

    let provisioning = sqlx::query(
        r#"SELECT
          SUM(CASE WHEN p.provision_circuit_open=1 THEN 1 ELSE 0 END) AS open_circuits,
          SUM(p.provision_failure_count) AS failures
        FROM runner_pools p
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
    let failed_webhooks = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM webhook_deliveries wd
          WHERE wd.status='failed' AND wd.received_at>=?
            AND (wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui
              WHERE ui.installation_id=wd.installation_id AND ui.user_id=?))"#,
    )
    .bind(now.saturating_sub(24 * 60 * 60 * 1_000))
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let mut alerts = Vec::new();
    if oldest_queue_age_ms.is_some_and(|age| age >= 5 * 60 * 1_000) {
        alerts.push(json!({
            "level": "warning", "title": "Jobs have been waiting for over five minutes",
            "detail": format_duration_millis(oldest_queue_age_ms.unwrap_or_default()),
            "href": "/workflow-runs",
        }));
    }
    let open_circuits = provisioning.get::<i64, _>("open_circuits");
    if open_circuits > 0 {
        alerts.push(json!({
            "level": "error", "title": format!("{open_circuits} provisioning circuit(s) open"),
            "detail": "GridOps is pausing repeated failed runner starts until the pool is retried.",
            "href": "/runner-pools",
        }));
    }
    if failed_webhooks > 0 {
        alerts.push(json!({
            "level": "warning", "title": format!("{failed_webhooks} webhook delivery failure(s) in 24h"),
            "detail": "Polling continues to synchronize Actions data, but delivery failures should be reviewed.",
            "href": "/webhooks",
        }));
    }

    let pool_rows = sqlx::query(
        r#"SELECT p.id,p.name,p.scope,p.desired_count,p.mode,p.state,p.paused,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.status IN ('idle','busy','online') THEN 1 END) AS online,
          COUNT(CASE WHEN r.deleted_at IS NULL AND r.busy=1 THEN 1 END) AS busy,
          (SELECT COUNT(*) FROM workflow_jobs wj
            JOIN workflow_runs wr ON wr.id=wj.run_id
            JOIN repositories queued_repo ON queued_repo.id=wr.repository_id
            WHERE wj.status='queued' AND (
              EXISTS (SELECT 1 FROM runner_pool_repositories membership
                WHERE membership.pool_id=p.id AND membership.repository_id=queued_repo.id) OR
              queued_repo.id=p.repository_id OR
              (p.scope='organization' AND queued_repo.installation_id=p.installation_id)
            )) AS queued
        FROM runner_pools p LEFT JOIN runners r ON r.pool_id=p.id
        WHERE EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
          AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
        GROUP BY p.id ORDER BY p.created_at DESC LIMIT 8"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let pools = pool_rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"), "name": row.get::<String, _>("name"),
                "scope": row.get::<String, _>("scope"), "desired": row.get::<i64, _>("desired_count"),
                "online": row.get::<i64, _>("online"), "busy": row.get::<i64, _>("busy"),
                "queue": row.get::<i64, _>("queued"),
                "mode": row.get::<String, _>("mode"),
                "status": if row.get::<bool, _>("paused") { "paused".into() } else { row.get::<String, _>("state") },
            })
        })
        .collect::<Vec<_>>();
    let run_rows = sqlx::query(
        r#"SELECT wr.id,repo.full_name,wr.workflow_name,wr.head_branch,wr.status,wr.conclusion,
          wr.started_at,wr.completed_at,wr.html_url FROM workflow_runs wr
        JOIN repositories repo ON repo.id=wr.repository_id
        JOIN user_installations ui ON ui.installation_id=repo.installation_id
        WHERE ui.user_id=? ORDER BY wr.github_created_at DESC LIMIT 6"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let runs = run_rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<i64, _>("id"), "repository": row.get::<String, _>("full_name"),
                "workflow": row.get::<String, _>("workflow_name"), "branch": row.try_get::<Option<String>, _>("head_branch").ok().flatten(),
                "status": row.get::<String, _>("status"), "conclusion": row.try_get::<Option<String>, _>("conclusion").ok().flatten(),
                "startedAt": iso_optional(row.try_get::<Option<i64>, _>("started_at").ok().flatten()),
                "completedAt": iso_optional(row.try_get::<Option<i64>, _>("completed_at").ok().flatten()),
                "htmlUrl": row.get::<String, _>("html_url"),
            })
        })
        .collect::<Vec<_>>();
    let activity_rows = sqlx::query(
        r#"SELECT re.id,re.level,re.event,re.message,re.runner_id,re.pool_id,re.created_at FROM runner_events re
        WHERE EXISTS (
          SELECT 1 FROM runner_pools p WHERE p.id=re.pool_id
            AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                AND NOT EXISTS (SELECT 1 FROM user_installations access
                  WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
            )
        )
        ORDER BY re.created_at DESC LIMIT 8"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let activity = activity_rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("id"), "level": row.get::<String, _>("level"),
                "event": row.get::<String, _>("event"), "message": row.get::<String, _>("message"),
                "runnerId": row.try_get::<Option<String>, _>("runner_id").ok().flatten(),
                "poolId": row.try_get::<Option<String>, _>("pool_id").ok().flatten(),
                "createdAt": iso(row.get::<i64, _>("created_at")),
            })
        })
        .collect::<Vec<_>>();
    let installations = sqlx::query(
        "SELECT COUNT(*) AS count FROM user_installations ui JOIN installations i ON i.id=ui.installation_id WHERE ui.user_id=? AND i.suspended_at IS NULL",
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?
    .get::<i64, _>("count");
    Ok(Json(json!({
        "authenticated": true,
        "configuration": configuration,
        "metrics": {
            "runners": metrics.get::<i64, _>("runners"), "online": metrics.get::<i64, _>("online"),
            "busy": metrics.get::<i64, _>("busy"), "queuedJobs": metrics.get::<i64, _>("queued_jobs"),
            "successRate": success_rate,
        },
        "slo": {
            "windowHours": 24,
            "queue": {
                "oldestSeconds": oldest_queue_age_ms.map(millis_to_seconds),
                "p95Seconds": queue_p95_ms.map(millis_to_seconds),
            },
            "startLatency": {
                "sampleSize": start_latencies.len(),
                "p50Seconds": start_p50_ms.map(millis_to_seconds),
                "p95Seconds": start_p95_ms.map(millis_to_seconds),
            },
            "failures": failures.into_iter().map(|(reason, count)| json!({ "reason": reason, "count": count })).collect::<Vec<_>>(),
            "alerts": alerts,
        },
        "pools": pools, "runs": runs, "activity": activity, "installations": installations,
    })))
}

pub async fn capacity_history(
    State(state): State<AppState>,
    Query(query): Query<CapacityQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let window = query.window.as_deref().unwrap_or("24h");
    let (window_millis, bucket_millis) = capacity_window(window)
        .ok_or_else(|| ApiError::BadRequest("Capacity window must be 24h, 7d, or 30d.".into()))?;
    let Some(user) = user else {
        return Ok(Json(json!({ "window": window, "points": [] })));
    };
    let cutoff = now_millis().saturating_sub(window_millis);
    let rows = sqlx::query(
        r#"SELECT bucket,SUM(available) AS available,SUM(busy) AS busy,SUM(queued) AS queued
          FROM (
            SELECT cs.pool_id,(cs.recorded_at / ?) * ? AS bucket,
              CAST(ROUND(AVG(cs.available)) AS INTEGER) AS available,
              CAST(ROUND(AVG(cs.busy)) AS INTEGER) AS busy,
              CAST(ROUND(AVG(cs.queued)) AS INTEGER) AS queued
            FROM capacity_samples cs JOIN runner_pools p ON p.id=cs.pool_id
            WHERE cs.recorded_at>=?
              AND EXISTS (SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id)
              AND NOT EXISTS (
                SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
                  AND NOT EXISTS (SELECT 1 FROM user_installations access
                    WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
              )
            GROUP BY cs.pool_id,bucket
          ) samples GROUP BY bucket ORDER BY bucket"#,
    )
    .bind(bucket_millis)
    .bind(bucket_millis)
    .bind(cutoff)
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let points = rows
        .iter()
        .map(|row| {
            json!({
                "recordedAt": iso(row.get::<i64, _>("bucket")),
                "available": row.get::<i64, _>("available"),
                "busy": row.get::<i64, _>("busy"),
                "queued": row.get::<i64, _>("queued"),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "window": window, "points": points })))
}
