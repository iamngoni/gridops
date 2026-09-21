//! Repository discovery and runner-pool option API handlers.

use super::*;

pub async fn repositories(
    State(state): State<AppState>,
    Query(query): Query<RepositoryQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let requested_page = query_page(query.page);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let query = query.q.unwrap_or_default().trim().to_owned();
    if query.len() > 100 {
        return Err(ApiError::BadRequest(
            "Repository search is limited to 100 characters.".into(),
        ));
    }
    let Some(user) = user else {
        return Ok(Json(json!({
            "authenticated": false, "items": [], "total": 0, "page": requested_page,
            "perPage": per_page, "query": query,
        })));
    };
    let (_, mut available) = available_repositories(&state, &user, false).await?;
    if !query.is_empty() {
        let needle = query.to_lowercase();
        available.retain(|item| item.repository.full_name.to_lowercase().contains(&needle));
    }
    available.sort_by(|left, right| {
        left.repository
            .full_name
            .to_lowercase()
            .cmp(&right.repository.full_name.to_lowercase())
    });
    let total = i64::try_from(available.len()).unwrap_or(i64::MAX);
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let limit = usize::try_from(per_page).unwrap_or(100);
    let stored_rows = sqlx::query(
        r#"SELECT repo.id,repo.last_synced_at,
          (SELECT COUNT(*) FROM runner_pool_repositories membership
            WHERE membership.repository_id=repo.id) AS pool_count,
          COUNT(DISTINCT wr.id) AS run_count,MAX(wr.github_updated_at) AS last_run_at
        FROM repositories repo JOIN user_installations ui ON ui.installation_id=repo.installation_id
        LEFT JOIN workflow_runs wr ON wr.repository_id=repo.id
        WHERE ui.user_id=? GROUP BY repo.id"#,
    )
    .bind(&user.id)
    .fetch_all(&state.database)
    .await?;
    let stored = stored_rows
        .iter()
        .map(|row| {
            (
                row.get::<i64, _>("id"),
                RepositoryStats {
                    last_synced_at: row.get::<i64, _>("last_synced_at"),
                    pool_count: row.get::<i64, _>("pool_count"),
                    run_count: row.get::<i64, _>("run_count"),
                    last_run_at: row.try_get::<Option<i64>, _>("last_run_at").ok().flatten(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let fetched_at = now_millis();
    let items = available
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|item| {
            let repository = item.repository;
            let stats = stored.get(&repository.id);
            let permission = repository.permissions.as_ref().and_then(|permissions| {
                permissions
                    .iter()
                    .find_map(|(name, allowed)| allowed.then_some(name))
            });
            json!({
                "id": repository.id, "fullName": repository.full_name,
                "private": repository.private, "archived": repository.archived,
                "defaultBranch": repository.default_branch, "htmlUrl": repository.html_url,
                "permission": permission, "connected": stats.is_some(),
                "lastSyncedAt": iso(stats.map_or(fetched_at, |value| value.last_synced_at)),
                "installationId": item.installation.id, "accountLogin": item.installation.account_login,
                "accountType": item.installation.account_type,
                "repositorySelection": item.installation.repository_selection,
                "poolCount": stats.map_or(0, |value| value.pool_count),
                "runCount": stats.map_or(0, |value| value.run_count),
                "lastRunAt": iso_optional(stats.and_then(|value| value.last_run_at)),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "authenticated": true, "items": items, "total": total, "page": page,
        "perPage": per_page, "query": query,
    })))
}

async fn available_repositories(
    state: &AppState,
    user: &AuthUser,
    require_admin: bool,
) -> ApiResult<(Vec<InstallationAccess>, Vec<AvailableRepository>)> {
    let installations = available_installations(state, user, require_admin).await?;
    let groups = stream::iter(installations.iter().cloned())
        .map(|installation| async move {
            let repositories =
                available_repositories_for_installation(state, user, &installation).await?;
            Ok::<Vec<AvailableRepository>, ApiError>(
                repositories
                    .into_iter()
                    .map(|repository| AvailableRepository {
                        installation: installation.clone(),
                        repository,
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .buffered(4)
        .try_collect::<Vec<_>>()
        .await?;
    let mut available = Vec::new();
    for group in groups {
        available.extend(group);
    }
    Ok((installations, available))
}

async fn available_installations(
    state: &AppState,
    user: &AuthUser,
    require_admin: bool,
) -> ApiResult<Vec<InstallationAccess>> {
    let rows = sqlx::query(
        r#"SELECT i.id,i.account_login,i.account_type,i.repository_selection
           FROM user_installations ui JOIN installations i ON i.id=ui.installation_id
           WHERE ui.user_id=? AND i.suspended_at IS NULL
             AND (?=0 OR ui.permission='admin' OR ?='admin')
           ORDER BY i.account_login"#,
    )
    .bind(&user.id)
    .bind(require_admin)
    .bind(&user.role)
    .fetch_all(&state.database)
    .await?;
    Ok(rows
        .iter()
        .map(|row| InstallationAccess {
            id: row.get::<i64, _>("id"),
            account_login: row.get::<String, _>("account_login"),
            account_type: row.get::<String, _>("account_type"),
            repository_selection: row.get::<String, _>("repository_selection"),
        })
        .collect::<Vec<_>>())
}

async fn available_repositories_for_installation(
    state: &AppState,
    user: &AuthUser,
    installation: &InstallationAccess,
) -> ApiResult<Vec<GitHubRepository>> {
    let token = control_token(state, &user.id, installation.id).await?;
    let first_page: RepositoryPage = state
        .github
        .get("/installation/repositories?per_page=100&page=1", &token)
        .await
        .map_err(ApiError::Internal)?;
    let expected_total = first_page.total_count;
    let mut repositories = first_page.repositories;
    let last_page = expected_total
        .saturating_add(99)
        .div_euclid(100)
        .clamp(1, 100);
    if last_page > 1 {
        let github = &state.github;
        let token = &token;
        let remaining = stream::iter(2..=last_page)
            .map(|page| async move {
                github
                    .get::<RepositoryPage>(
                        &format!("/installation/repositories?per_page=100&page={page}"),
                        token,
                    )
                    .await
                    .map_err(ApiError::Internal)
            })
            .buffered(4)
            .try_collect::<Vec<_>>()
            .await?;
        for page in remaining {
            repositories.extend(page.repositories);
        }
    }
    let loaded = i64::try_from(repositories.len()).unwrap_or(i64::MAX);
    if loaded < expected_total {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "GitHub returned {loaded} of {expected_total} repositories for installation {}",
            installation.id
        )));
    }
    Ok(repositories)
}

pub(super) async fn selected_available_repositories(
    state: &AppState,
    user: &AuthUser,
    repository_ids: &[i64],
) -> ApiResult<Vec<AvailableRepository>> {
    let requested = repository_ids.iter().copied().collect::<HashSet<_>>();
    let (_, available) = available_repositories(state, user, true).await?;
    let mut selected = available
        .into_iter()
        .filter(|item| requested.contains(&item.repository.id) && !item.repository.archived)
        .collect::<Vec<_>>();
    if selected.len() != requested.len() {
        return Err(ApiError::BadRequest(
            "One or more selected repositories are unavailable or you cannot administer their GitHub App installation."
                .into(),
        ));
    }
    selected.sort_by_key(|item| item.repository.id);
    Ok(selected)
}

pub(super) async fn selected_bitbucket_connections(
    state: &AppState,
    user: &AuthUser,
    connection_ids: &[String],
) -> ApiResult<Vec<String>> {
    if connection_ids.is_empty() {
        return Ok(Vec::new());
    }
    require_system_admin(user)?;
    let mut requested = connection_ids.to_vec();
    requested.sort();
    requested.dedup();
    let mut selected = Vec::with_capacity(requested.len());
    for connection_id in &requested {
        let connection =
            sqlx::query_scalar::<_, String>("SELECT id FROM bitbucket_connections WHERE id=?")
                .bind(connection_id)
                .fetch_optional(&state.database)
                .await?;
        if let Some(connection) = connection {
            selected.push(connection);
        }
    }
    if selected.len() != requested.len() {
        return Err(ApiError::BadRequest(
            "One or more Bitbucket workspace connections do not exist.".into(),
        ));
    }
    Ok(selected)
}

pub(super) async fn next_bitbucket_connection(
    state: &AppState,
    pool_id: &str,
) -> ApiResult<Option<BitbucketPoolConnection>> {
    sqlx::query_as(
        r#"SELECT connection.id,connection.workspace,connection.workspace_uuid,connection.access_token_key
           FROM runner_pool_bitbucket_connections membership
           JOIN bitbucket_connections connection ON connection.id=membership.connection_id
           WHERE membership.pool_id=?
             AND NOT EXISTS (
               SELECT 1 FROM runners runner
               WHERE runner.pool_id=membership.pool_id
                 AND runner.ci_platform='bitbucket'
                 AND runner.bitbucket_connection_id=connection.id
                 AND runner.deleted_at IS NULL
                 AND runner.status IN ('starting','online','idle','busy','paused','stopped')
             )
           ORDER BY membership.created_at,connection.id LIMIT 1"#,
    )
    .bind(pool_id)
    .fetch_optional(&state.database)
    .await
    .map_err(Into::into)
}

pub(super) fn bitbucket_runner_labels(
    pool_name: &str,
    labels: &[String],
) -> ApiResult<Vec<String>> {
    let mut output = vec![format!("gridops.{}", pool_name.replace('-', "."))];
    output.extend(
        labels
            .iter()
            .filter(|label| label.as_str() != pool_name)
            .cloned(),
    );
    output.sort();
    output.dedup();
    if output.len() > 10
        || output.iter().any(|label| {
            label.is_empty()
                || !label.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '.'
                })
        })
    {
        return Err(ApiError::BadRequest(
            "Bitbucket runner labels must use lowercase letters, numbers, and dots; at most 10 are allowed."
                .into(),
        ));
    }
    Ok(output)
}

pub async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let query = query.q.trim();
    if !(2..=100).contains(&query.len()) {
        return Err(ApiError::BadRequest(
            "Search requires 2-100 characters.".into(),
        ));
    }
    let pattern = like_pattern(query);
    let rows = sqlx::query(
        r#"SELECT kind,id,title,subtitle,href FROM (
          SELECT 'repository' AS kind,CAST(repo.id AS TEXT) AS id,repo.full_name AS title,
            i.account_login AS subtitle,'/repositories' AS href,repo.full_name AS sort_value
          FROM repositories repo JOIN installations i ON i.id=repo.installation_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE ui.user_id=? AND repo.full_name LIKE ? ESCAPE '\'
          UNION ALL
          SELECT 'runner pool',p.id,p.name,COALESCE(repo.full_name,i.account_login),
            '/runner-pools/' || p.id,p.name
          FROM runner_pools p JOIN installations i ON i.id=p.installation_id
          LEFT JOIN repositories repo ON repo.id=p.repository_id
          WHERE p.name LIKE ? ESCAPE '\' AND EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
          ) AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
          UNION ALL
          SELECT 'runner',r.id,r.name,p.name,'/runners',r.name FROM runners r
          JOIN runner_pools p ON p.id=r.pool_id
          WHERE r.deleted_at IS NULL AND r.name LIKE ? ESCAPE '\' AND EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
          ) AND NOT EXISTS (
            SELECT 1 FROM runner_pool_installations mapped WHERE mapped.pool_id=p.id
              AND NOT EXISTS (SELECT 1 FROM user_installations access
                WHERE access.user_id=? AND access.installation_id=mapped.installation_id)
          )
          UNION ALL
          SELECT 'workflow run',CAST(wr.id AS TEXT),wr.workflow_name,repo.full_name,
            '/workflow-runs/' || wr.id,wr.workflow_name FROM workflow_runs wr
          JOIN repositories repo ON repo.id=wr.repository_id
          JOIN user_installations ui ON ui.installation_id=repo.installation_id
          WHERE ui.user_id=? AND (wr.workflow_name LIKE ? ESCAPE '\' OR repo.full_name LIKE ? ESCAPE '\')
        ) ORDER BY sort_value LIMIT 12"#,
    )
    .bind(&user.id).bind(&pattern).bind(&pattern).bind(&user.id)
    .bind(&pattern).bind(&user.id).bind(&user.id).bind(&pattern).bind(&pattern)
    .fetch_all(&state.database).await?;
    let items = rows
        .iter()
        .map(|row| {
            json!({
                "kind": row.get::<String,_>("kind"), "id": row.get::<String,_>("id"),
                "title": row.get::<String,_>("title"), "subtitle": row.get::<String,_>("subtitle"),
                "href": row.get::<String,_>("href"),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!(items)))
}

pub async fn runner_pool_options(
    State(state): State<AppState>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let Some(user) = user else {
        return Ok(Json(
            json!({ "authenticated": false, "installations": [], "repositories": [], "runnerGroups": [], "bitbucketConnections": [], "installUrl": null, "defaults": null }),
        ));
    };
    let installation_access = available_installations(&state, &user, true).await?;
    let installations = installation_access
        .iter()
        .map(|installation| {
            json!({
                "id": installation.id, "accountLogin": installation.account_login,
                "accountType": installation.account_type,
            })
        })
        .collect::<Vec<_>>();
    let app_slug = state.github_app_slug().await.map_err(ApiError::Internal)?;
    let bitbucket_connections = sqlx::query(
        "SELECT id,name,workspace,workspace_uuid FROM bitbucket_connections ORDER BY lower(name),id",
    )
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
    let (max_cpu_limit, max_memory_limit_mb) = manager_resource_capacity(&state).await;
    Ok(Json(json!({
        "authenticated": true, "installations": installations, "repositories": [], "runnerGroups": [], "bitbucketConnections": bitbucket_connections,
        "installUrl": format!("https://github.com/apps/{app_slug}/installations/new"),
        "defaults": runner_pool_defaults(state.config.runner_image(), max_cpu_limit, max_memory_limit_mb)
    })))
}

pub async fn runner_pool_repository_options(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let (_, mut available) = available_repositories(&state, &user, true).await?;
    available.retain(|item| !item.repository.archived);
    available.sort_by(|left, right| {
        left.repository
            .full_name
            .to_lowercase()
            .cmp(&right.repository.full_name.to_lowercase())
    });
    let items = available
        .into_iter()
        .map(|item| {
            json!({
                "id": item.repository.id,
                "installationId": item.installation.id,
                "accountLogin": item.installation.account_login,
                "accountType": item.installation.account_type,
                "fullName": item.repository.full_name,
                "private": item.repository.private,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

pub async fn installation_repositories(
    State(state): State<AppState>,
    Path(installation_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_installation_admin(&state, &user, installation_id).await?;
    let installation = available_installations(&state, &user, true)
        .await?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| ApiError::NotFound("GitHub installation does not exist.".into()))?;
    let mut repositories =
        available_repositories_for_installation(&state, &user, &installation).await?;
    repositories.retain(|repository| !repository.archived);
    repositories.sort_by(|left, right| {
        left.full_name
            .to_lowercase()
            .cmp(&right.full_name.to_lowercase())
    });
    let items = repositories
        .into_iter()
        .map(|repository| {
            json!({
                "id": repository.id,
                "installationId": installation.id,
                "accountLogin": installation.account_login,
                "accountType": installation.account_type,
                "fullName": repository.full_name,
                "private": repository.private,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}

pub async fn installation_runner_groups(
    State(state): State<AppState>,
    Path(installation_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    assert_installation_admin(&state, &user, installation_id).await?;
    let installation = sqlx::query(
        "SELECT account_login,account_type FROM installations WHERE id=? AND suspended_at IS NULL",
    )
    .bind(installation_id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| ApiError::NotFound("GitHub installation does not exist.".into()))?;
    let account_login = installation.get::<String, _>("account_login");
    let account_type = installation.get::<String, _>("account_type");
    if account_type != "Organization" {
        return Ok(Json(json!({ "items": [] })));
    }

    let token = control_token(&state, &user.id, installation_id).await?;
    let groups = state
        .github
        .runner_groups(&account_login, &token)
        .await
        .map_err(ApiError::Internal)?;
    let items = groups
        .into_iter()
        .map(|group| {
            json!({
                "id": group.id,
                "name": group.name,
                "visibility": group.visibility,
                "isDefault": group.is_default,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })))
}
