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
    adopt_legacy_drizzle_schema(&pool).await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("failed to migrate GridOps database")?;
    Ok(pool)
}

async fn adopt_legacy_drizzle_schema(pool: &SqlitePool) -> Result<()> {
    let has_drizzle_history = table_exists(pool, "__drizzle_migrations").await?;
    if !has_drizzle_history {
        return Ok(());
    }

    let applied_sqlx_migrations = if table_exists(pool, "_sqlx_migrations").await? {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(pool)
            .await?
    } else {
        0
    };
    if applied_sqlx_migrations > 0 {
        return Ok(());
    }

    for table in ["users", "runner_pools", "runners", "workflow_runs"] {
        anyhow::ensure!(
            table_exists(pool, table).await?,
            "legacy GridOps database is incomplete: missing {table} table"
        );
    }

    tracing::info!("adopting legacy Drizzle migration history into SQLx");
    MIGRATOR
        .skip(pool, Some(1))
        .await
        .context("failed to adopt the legacy base schema")?;

    if column_exists(pool, "runner_pools", "runner_group_id").await? {
        MIGRATOR
            .skip(pool, Some(2))
            .await
            .context("failed to adopt the legacy runner-group migration")?;
    } else {
        MIGRATOR
            .run_to(2, pool)
            .await
            .context("failed to apply the runner-group migration")?;
    }

    let autoscaling_columns = [
        "autoscaling_enabled",
        "queue_scale_factor",
        "idle_timeout_minutes",
    ];
    let mut present = 0;
    for column in autoscaling_columns {
        if column_exists(pool, "runner_pools", column).await? {
            present += 1;
        }
    }

    match present {
        0 => MIGRATOR
            .run_to(3, pool)
            .await
            .context("failed to apply the autoscaling migration")?,
        3 => MIGRATOR
            .skip(pool, Some(3))
            .await
            .context("failed to adopt the legacy autoscaling migration")?,
        _ => anyhow::bail!("legacy GridOps database has a partially applied autoscaling migration"),
    }

    Ok(())
}

async fn table_exists(pool: &SqlitePool, table: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind(table)
    .fetch_one(pool)
    .await?
        > 0)
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
            .bind(table)
            .bind(column)
            .fetch_one(pool)
            .await?
            > 0,
    )
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
    use sqlx::{Connection as _, Executor as _, Row as _, SqliteConnection};

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

    #[tokio::test]
    async fn complete_drizzle_schema_is_adopted_without_losing_data() -> Result<()> {
        let directory =
            std::env::temp_dir().join(format!("gridops-legacy-db-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory)?;
        let path = directory.join("gridops.sqlite");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true);
        let mut connection = SqliteConnection::connect_with(&options).await?;
        connection
            .execute(sqlx::raw_sql(include_str!(
                "../../../migrations/0001_initial.sql"
            )))
            .await?;
        connection
            .execute(sqlx::raw_sql(include_str!(
                "../../../migrations/0002_runner_group.sql"
            )))
            .await?;
        connection
            .execute(sqlx::raw_sql(include_str!(
                "../../../migrations/0003_autoscaling.sql"
            )))
            .await?;
        connection
            .execute(
                "CREATE TABLE __drizzle_migrations (id INTEGER PRIMARY KEY, hash TEXT NOT NULL, created_at INTEGER)",
            )
            .await?;
        connection
            .execute(
                "INSERT INTO settings (key, value, updated_at) VALUES ('legacy', 'preserved', 1)",
            )
            .await?;
        connection.close().await?;

        let pool = connect_database_path(&path).await?;
        let migration_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await?;
        let preserved =
            sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = 'legacy'")
                .fetch_one(&pool)
                .await?;
        assert_eq!(migration_count, 3);
        assert_eq!(preserved, "preserved");
        pool.close().await;
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
