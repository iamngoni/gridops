use std::{
    fs,
    path::Path,
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::Config;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

pub async fn connect_database(config: &Config) -> Result<SqlitePool> {
    connect_database_path(config.database_path()).await
}

pub async fn connect_database_path(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create database directory")?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("failed to migrate GridOps database")?;
    Ok(pool)
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row as _;

    #[tokio::test]
    async fn fresh_database_applies_every_migration() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("gridops-db-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("gridops.sqlite");
        let pool = connect_database_path(&path).await?;
        let tables = sqlx::query(
            "SELECT COUNT(*) AS count FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
        )
        .fetch_one(&pool)
        .await?
        .get::<i64, _>("count");
        assert_eq!(tables, 15);
        let columns = sqlx::query("PRAGMA table_info(runner_pools)")
            .fetch_all(&pool)
            .await?;
        assert!(
            columns
                .iter()
                .any(|row| { row.get::<String, _>("name") == "autoscaling_enabled" })
        );
        pool.close().await;
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
