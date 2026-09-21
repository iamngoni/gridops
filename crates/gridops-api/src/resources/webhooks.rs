//! Webhook delivery API handlers.

use super::*;

pub async fn webhook_deliveries(
    State(state): State<AppState>,
    Query(query): Query<PaginationQuery>,
    OptionalAuth(user): OptionalAuth,
) -> ApiResult<Json<Value>> {
    let (requested_page, per_page) = pagination(query.page, query.per_page);
    let Some(user) = user else {
        return Ok(empty_paginated_page(requested_page, per_page));
    };
    let total = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM webhook_deliveries wd
        WHERE wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui
          WHERE ui.installation_id=wd.installation_id AND ui.user_id=?)"#,
    )
    .bind(&user.id)
    .fetch_one(&state.database)
    .await?;
    let (page, offset) = bounded_pagination(requested_page, total, per_page);
    let rows = sqlx::query(
        r#"SELECT wd.id,wd.event,wd.action,wd.installation_id,wd.repository_id,
          wd.signature_valid,wd.status,wd.error,wd.received_at,wd.processed_at,
          wd.payload IS NOT NULL AS has_payload,
          CASE WHEN wd.installation_id IS NULL THEN ?='admin' ELSE EXISTS (
            SELECT 1 FROM user_installations manage WHERE manage.installation_id=wd.installation_id
              AND manage.user_id=? AND (manage.permission='admin' OR ?='admin')
          ) END AS can_retry,
          i.account_login,repo.full_name FROM webhook_deliveries wd
        LEFT JOIN installations i ON i.id=wd.installation_id LEFT JOIN repositories repo ON repo.id=wd.repository_id
        WHERE wd.installation_id IS NULL OR EXISTS (SELECT 1 FROM user_installations ui
          WHERE ui.installation_id=wd.installation_id AND ui.user_id=?)
        ORDER BY wd.received_at DESC LIMIT ? OFFSET ?"#,
    )
    .bind(&user.role)
    .bind(&user.id)
    .bind(&user.role)
    .bind(&user.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.database)
    .await?;
    let items = rows.iter().map(|row| json!({
        "id": row.get::<String,_>("id"), "event": row.get::<String,_>("event"), "action": row.try_get::<Option<String>,_>("action").ok().flatten(),
        "installationId": row.try_get::<Option<i64>,_>("installation_id").ok().flatten(), "repositoryId": row.try_get::<Option<i64>,_>("repository_id").ok().flatten(),
        "signatureValid": row.get::<bool,_>("signature_valid"), "status": row.get::<String,_>("status"), "error": row.try_get::<Option<String>,_>("error").ok().flatten(),
        "receivedAt": iso(row.get::<i64,_>("received_at")), "processedAt": iso_optional(row.try_get::<Option<i64>,_>("processed_at").ok().flatten()),
        "accountLogin": row.try_get::<Option<String>,_>("account_login").ok().flatten(), "repository": row.try_get::<Option<String>,_>("full_name").ok().flatten(),
        "canRetry": row.get::<bool,_>("can_retry"), "hasPayload": row.get::<bool,_>("has_payload"),
    })).collect::<Vec<_>>();
    Ok(paginated_page(&items, total, page, per_page))
}

pub async fn webhook_delivery(
    State(state): State<AppState>,
    Path(delivery_id): Path<String>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let row = sqlx::query(
        r#"SELECT wd.id,wd.event,wd.payload FROM webhook_deliveries wd
        WHERE wd.id=? AND (
          (wd.installation_id IS NULL AND ?='admin') OR EXISTS (
            SELECT 1 FROM user_installations ui
            WHERE ui.installation_id=wd.installation_id AND ui.user_id=?
          )
        )"#,
    )
    .bind(&delivery_id)
    .bind(&user.role)
    .bind(&user.id)
    .fetch_optional(&state.database)
    .await?
    .ok_or_else(|| {
        ApiError::NotFound("Webhook delivery does not exist or is not accessible.".into())
    })?;
    let stored = row.try_get::<Option<String>, _>("payload")?;
    let payload = stored
        .as_deref()
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored webhook payload is invalid")))?;
    Ok(Json(json!({
        "id": row.get::<String, _>("id"),
        "event": row.get::<String, _>("event"),
        "payload": payload,
        "payloadBytes": stored.as_ref().map_or(0, String::len),
    })))
}
