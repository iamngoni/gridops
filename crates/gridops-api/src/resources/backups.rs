//! Database backup API handlers.

use super::*;

pub async fn database_backup(State(state): State<AppState>, user: AuthUser) -> ApiResult<Response> {
    require_system_admin(&user)?;
    let backup_name = format!(
        "gridops-backup-{}.sqlite",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let backup_path = state.config.database_path().with_file_name(format!(
        "{}.{}",
        backup_name,
        uuid::Uuid::new_v4()
    ));
    sqlx::query("VACUUM INTO ?")
        .bind(backup_path.to_string_lossy().as_ref())
        .execute(&state.database)
        .await?;
    let bytes = tokio::fs::read(&backup_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    tokio::fs::remove_file(&backup_path)
        .await
        .map_err(|error| ApiError::Internal(error.into()))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/vnd.sqlite3"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{backup_name}\""),
            ),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        Body::from(bytes),
    )
        .into_response())
}
