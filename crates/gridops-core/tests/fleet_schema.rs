//! Exercises fleet constraints on real `SQLite` files with legacy rows and WAL.
//! Migration success alone is insufficient: identity, history, and charged
//! resources must survive upgrade and reject writes that break ownership.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use anyhow::Result;
use sqlx::{
    Row, SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");
const HOST: &str = "10000000-0000-4000-8000-000000000001";
const OTHER_HOST: &str = "20000000-0000-4000-8000-000000000001";
const ROOT: &str = "11000000-0000-4000-8000-000000000001";
const BACKEND: &str = "13000000-0000-4000-8000-000000000001";
const TARGET: &str = "30000000-0000-4000-8000-000000000001";
const OTHER_TARGET: &str = "30000000-0000-4000-8000-000000000002";
const PLACEMENT: &str = "60000000-0000-4000-8000-000000000001";

struct Database {
    directory: PathBuf,
    pool: SqlitePool,
}

impl Database {
    async fn legacy() -> Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("gridops-fleet-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let options = SqliteConnectOptions::new()
            .filename(directory.join("test.sqlite"))
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        MIGRATOR.run_to(21, &pool).await?;
        sqlx::raw_sql(include_str!("fixtures/fleet_legacy.sql"))
            .execute(&pool)
            .await?;
        Ok(Self { directory, pool })
    }

    async fn ready() -> Result<Self> {
        let database = Self::legacy().await?;
        MIGRATOR.run(&database.pool).await?;
        sqlx::raw_sql(include_str!("fixtures/fleet_ready.sql"))
            .execute(&database.pool)
            .await?;
        Ok(database)
    }

    async fn close(self) -> Result<()> {
        self.pool.close().await;
        std::fs::remove_dir_all(self.directory)?;
        Ok(())
    }
}

async fn schema(pool: &SqlitePool) -> Result<BTreeMap<String, String>> {
    let rows = sqlx::query("SELECT name,sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'")
        .fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|row| (row.get("name"), row.get("sql")))
        .collect())
}

async fn records(pool: &SqlitePool, tables: &[String]) -> Result<BTreeMap<String, Vec<String>>> {
    let mut result = BTreeMap::new();
    for table in tables {
        // Names come exclusively from SQLite's schema, never request input.
        let columns = sqlx::query("SELECT name FROM pragma_table_info(?) ORDER BY cid")
            .bind(table)
            .fetch_all(pool)
            .await?;
        let columns = columns
            .iter()
            .map(|row| format!("\"{}\"", row.get::<String, _>("name").replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "SELECT json_array({columns}) AS record FROM \"{}\" ORDER BY record",
            table.replace('"', "\"\"")
        );
        let rows = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .fetch_all(pool)
            .await?;
        result.insert(table.clone(), rows);
    }
    Ok(result)
}

async fn rejected(pool: &SqlitePool, sql: &'static str) {
    let outcome = sqlx::query(sql).execute(pool).await;
    assert!(outcome.is_err(), "unsafe write was accepted: {sql}");
}

#[tokio::test]
async fn additive_upgrade_preserves_every_legacy_row_index_and_trigger() -> Result<()> {
    let db = Database::legacy().await?;
    let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY name")
        .fetch_all(&db.pool).await?;
    let old_schema = schema(&db.pool).await?;
    let old_records = records(&db.pool, &tables).await?;
    MIGRATOR.run(&db.pool).await?;
    assert_eq!(records(&db.pool, &tables).await?, old_records);
    let new_schema = schema(&db.pool).await?;
    for (name, definition) in old_schema {
        assert_eq!(
            new_schema.get(&name),
            Some(&definition),
            "changed legacy schema: {name}"
        );
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&db.pool)
            .await?
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&db.pool)
            .await?,
        "ok"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
            .fetch_one(&db.pool)
            .await?,
        "wal"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(&db.pool)
            .await?,
        1
    );
    // Reopening/rerunning the migration neither replays DDL nor loses history.
    MIGRATOR.run(&db.pool).await?;
    assert_eq!(records(&db.pool, &tables).await?, old_records);
    db.close().await
}

#[tokio::test]
async fn domains_are_integer_bounded_and_cannot_cross_physical_hosts() -> Result<()> {
    let db = Database::ready().await?;
    for sql in [
        "UPDATE host_resource_domains SET cpu_millis=0.5",
        "UPDATE host_resource_domains SET memory_mib=-1",
        "UPDATE fleet_hosts SET epoch=0",
        "UPDATE fleet_hosts SET revision=1.5",
        "UPDATE host_resource_domains SET parent_domain_id=id",
        "INSERT INTO host_resource_domains (id,host_id,name,cpu_millis,memory_mib,disk_bytes) SELECT 'duplicate-root',host_id,'Duplicate',1,1,1 FROM host_resource_domains WHERE parent_domain_id IS NULL LIMIT 1",
    ] {
        rejected(&db.pool, sql).await;
    }
    let result = sqlx::query("INSERT INTO host_resource_domains (id,host_id,parent_domain_id,name,cpu_millis,memory_mib,disk_bytes) VALUES ('cross-host',?,?,'Invalid',1,1,1)")
        .bind(OTHER_HOST).bind(ROOT).execute(&db.pool).await;
    assert!(result.is_err());
    let result = sqlx::query("UPDATE host_backends SET host_id=? WHERE id=?")
        .bind(OTHER_HOST)
        .bind(BACKEND)
        .execute(&db.pool)
        .await;
    assert!(result.is_err());
    // The Mac owns the physical budget while its Docker execution is Linux.
    let row = sqlx::query("SELECT h.host_os,b.execution_os FROM host_backends b JOIN fleet_hosts h ON h.id=b.host_id WHERE b.id=?")
        .bind(BACKEND).fetch_one(&db.pool).await?;
    assert_eq!(row.get::<String, _>("host_os"), "macos");
    assert_eq!(row.get::<String, _>("execution_os"), "linux");
    db.close().await
}

#[tokio::test]
async fn unknown_capacity_cannot_be_deleted_reassigned_or_released_without_proof() -> Result<()> {
    let db = Database::ready().await?;
    sqlx::query("UPDATE capacity_allocations SET state='uncertain'")
        .execute(&db.pool)
        .await?;
    for sql in [
        "DELETE FROM capacity_allocations",
        "UPDATE capacity_allocations SET host_epoch=2",
        "UPDATE capacity_allocations SET cpu_millis=1",
        "UPDATE workload_placements SET host_epoch=2",
        "UPDATE workload_placements SET generation=2",
        "UPDATE capacity_allocations SET state='released',released_at=100",
    ] {
        rejected(&db.pool, sql).await;
    }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT SUM(cpu_millis) FROM capacity_allocations WHERE domain_id=? AND state!='released'")
        .bind(ROOT).fetch_one(&db.pool).await?, 1000);
    sqlx::query("UPDATE workload_placements SET local_absence_proven_at=100,upstream_cleanup_state='pending'")
        .execute(&db.pool).await?;
    rejected(
        &db.pool,
        "UPDATE capacity_allocations SET state='released',released_at=100",
    )
    .await;
    sqlx::query("UPDATE workload_placements SET delayed_starts_excluded_at=101")
        .execute(&db.pool)
        .await?;
    sqlx::query("UPDATE capacity_allocations SET state='released',released_at=102")
        .execute(&db.pool)
        .await?;
    rejected(
        &db.pool,
        "UPDATE capacity_allocations SET state='reserved',released_at=NULL",
    )
    .await;
    rejected(
        &db.pool,
        "UPDATE workload_placements SET local_absence_proven_at=NULL",
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT upstream_cleanup_state FROM workload_placements WHERE id=?"
        )
        .bind(PLACEMENT)
        .fetch_one(&db.pool)
        .await?,
        "pending"
    );
    db.close().await
}

#[tokio::test]
async fn immutable_commands_keep_idempotency_and_distinguish_host_scope() -> Result<()> {
    let db = Database::ready().await?;
    for sql in [
        "UPDATE host_commands SET kind='start_runner'",
        "UPDATE host_commands SET payload_json='{\"changed\":true}'",
        "UPDATE host_commands SET deadline_at=9999999",
        "UPDATE fleet_operations SET request_hash='changed'",
        "UPDATE workload_intents SET target_id='30000000-0000-4000-8000-000000000002'",
        "INSERT INTO host_commands (id,operation_id,scope,host_id,host_epoch,kind,protocol_version,request_hash,payload_json,expected_revision,issued_at,deadline_at,idempotency_key) SELECT 'invalid-start',operation_id,'host',host_id,host_epoch,'start_runner',1,'hash','{}',1,1,2,'other-key' FROM host_commands LIMIT 1",
    ] {
        rejected(&db.pool, sql).await;
    }
    sqlx::query("INSERT INTO fleet_operations (id,kind,host_id,principal_kind,principal_scope,idempotency_key,request_hash,created_at,updated_at) VALUES ('80000000-0000-4000-8000-000000000001','upgrade_agent',?,'reconciler','controller:reconciler','upgrade-fixture','hash',1,1)")
        .bind(HOST).execute(&db.pool).await?;
    sqlx::query("INSERT INTO host_commands (id,operation_id,scope,host_id,host_epoch,kind,protocol_version,request_hash,payload_json,expected_revision,issued_at,deadline_at,idempotency_key) VALUES ('81000000-0000-4000-8000-000000000001','80000000-0000-4000-8000-000000000001','host',?,1,'upgrade_agent',1,'hash','{}',1,1,2,'upgrade-command')")
        .bind(HOST).execute(&db.pool).await?;
    db.close().await
}

#[tokio::test]
async fn target_identity_and_event_projection_cannot_be_rebound() -> Result<()> {
    let db = Database::ready().await?;
    for sql in [
        "INSERT INTO fleet_ci_targets (id,kind,canonical_key) VALUES ('incomplete','github_repository','incomplete')",
        "INSERT INTO fleet_ci_targets (id,kind,canonical_key,installation_id,organization_id,repository_id) VALUES ('mixed','github_organization','mixed',1,123,10)",
        "UPDATE fleet_ci_targets SET repository_id=11",
        "DELETE FROM profile_ci_targets",
    ] {
        rejected(&db.pool, sql).await;
    }
    // Revocation must preserve the historical mapping rather than require a delete.
    sqlx::query("UPDATE profile_ci_targets SET enabled=0,revoked_at=10")
        .execute(&db.pool)
        .await?;
    for (host, target) in [(OTHER_HOST, TARGET), (HOST, OTHER_TARGET)] {
        let result = sqlx::query("INSERT INTO fleet_events (id,host_id,placement_id,target_id,kind,created_at) VALUES (?, ?, ?, ?, 'test',1)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(host).bind(PLACEMENT).bind(target).execute(&db.pool).await;
        assert!(result.is_err());
    }
    db.close().await
}

#[tokio::test]
async fn linux_backends_belong_to_windows_macos_and_linux_physical_hosts() -> Result<()> {
    let db = Database::ready().await?;
    let hosts: Vec<String> = sqlx::query_scalar(
        "SELECT h.host_os FROM host_backends b JOIN fleet_hosts h ON h.id=b.host_id WHERE b.execution_os='linux' ORDER BY h.host_os",
    ).fetch_all(&db.pool).await?;
    assert_eq!(hosts, ["linux", "macos", "windows"]);
    for host in [HOST, OTHER_HOST] {
        let roots: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM host_resource_domains WHERE host_id=? AND parent_domain_id IS NULL",
        ).bind(host).fetch_one(&db.pool).await?;
        assert_eq!(roots, 1);
        let backends: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM host_backends WHERE host_id=?")
                .bind(host)
                .fetch_one(&db.pool)
                .await?;
        assert_eq!(
            backends, 2,
            "native and Linux execution share one physical host"
        );
    }
    db.close().await
}

#[tokio::test]
async fn epoch_replacement_and_retirement_preserve_original_ownership() -> Result<()> {
    let db = Database::ready().await?;
    sqlx::query(
        "UPDATE fleet_hosts SET epoch=2,lifecycle_state='retired',retired_at=10 WHERE id=?",
    )
    .bind(HOST)
    .execute(&db.pool)
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT host_epoch FROM workload_placements WHERE id=?")
            .bind(PLACEMENT)
            .fetch_one(&db.pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM capacity_allocations WHERE host_epoch=1"
        )
        .fetch_one(&db.pool)
        .await?,
        2
    );
    rejected(&db.pool, "DELETE FROM fleet_hosts").await;
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&db.pool)
            .await?
            .is_empty()
    );
    db.close().await
}

#[tokio::test]
async fn inventory_is_invalidated_on_upgrade_and_every_authority_change() -> Result<()> {
    let db = Database::legacy().await?;
    MIGRATOR.run_to(22, &db.pool).await?;
    sqlx::raw_sql(include_str!("fixtures/fleet_ready.sql"))
        .execute(&db.pool)
        .await?;
    sqlx::query("UPDATE fleet_hosts SET inventory_complete=1,last_inventory_at=1 WHERE id=?")
        .bind(HOST)
        .execute(&db.pool)
        .await?;
    MIGRATOR.run(&db.pool).await?;
    let snapshot: (i64, Option<i64>) =
        sqlx::query_as("SELECT inventory_complete,last_inventory_at FROM fleet_hosts WHERE id=?")
            .bind(HOST)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(snapshot, (0, None));
    rejected(&db.pool, "INSERT INTO fleet_hosts (id,name,host_os,architecture,inventory_complete,last_inventory_at,created_at,updated_at) VALUES ('a0000000-0000-4000-8000-000000000001','bad','linux','x64',1,1,1,1)").await;
    for change in [
        "UPDATE fleet_hosts SET epoch=epoch+1 WHERE id=?",
        "UPDATE fleet_hosts SET boot_id='next-boot' WHERE id=?",
        "UPDATE fleet_hosts SET agent_session_id='next-session' WHERE id=?",
        "UPDATE fleet_hosts SET authority_incarnation='next-incarnation' WHERE id=?",
    ] {
        sqlx::query("UPDATE fleet_hosts SET agent_session_id='current-session',boot_id='current-boot',authority_incarnation='current-incarnation' WHERE id=?")
            .bind(HOST).execute(&db.pool).await?;
        sqlx::query("UPDATE fleet_hosts SET inventory_complete=1,last_inventory_at=2,inventory_epoch=epoch,inventory_session_id=agent_session_id,inventory_boot_id=boot_id WHERE id=?")
            .bind(HOST).execute(&db.pool).await?;
        sqlx::query("UPDATE fleet_hosts SET epoch=epoch,boot_id=boot_id,agent_session_id=agent_session_id,authority_incarnation=authority_incarnation WHERE id=?")
            .bind(HOST).execute(&db.pool).await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT inventory_complete FROM fleet_hosts WHERE id=?")
                .bind(HOST)
                .fetch_one(&db.pool)
                .await?,
            1
        );
        sqlx::query(change).bind(HOST).execute(&db.pool).await?;
        let snapshot: (i64, Option<i64>, Option<i64>, Option<String>, Option<String>) = sqlx::query_as("SELECT inventory_complete,last_inventory_at,inventory_epoch,inventory_session_id,inventory_boot_id FROM fleet_hosts WHERE id=?")
            .bind(HOST).fetch_one(&db.pool).await?;
        assert_eq!(snapshot, (0, None, None, None, None));
        assert!(
            sqlx::query("UPDATE fleet_hosts SET inventory_complete=1 WHERE id=?")
                .bind(HOST)
                .execute(&db.pool)
                .await
                .is_err()
        );
        sqlx::query("UPDATE fleet_hosts SET inventory_complete=1,last_inventory_at=3,inventory_epoch=epoch,inventory_session_id=agent_session_id,inventory_boot_id=boot_id WHERE id=?")
            .bind(HOST).execute(&db.pool).await?;
    }
    // Even one UPDATE that attempts to refresh inventory while replacing the
    // session must wait for a later authenticated inventory acknowledgment.
    sqlx::query("UPDATE fleet_hosts SET agent_session_id='replacement',inventory_session_id='replacement',inventory_complete=1 WHERE id=?")
        .bind(HOST).execute(&db.pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT inventory_complete FROM fleet_hosts WHERE id=?")
            .bind(HOST)
            .fetch_one(&db.pool)
            .await?,
        0
    );
    sqlx::query("INSERT INTO fleet_hosts (id,name,host_os,architecture,inventory_complete,last_inventory_at,inventory_epoch,agent_session_id,inventory_session_id,boot_id,inventory_boot_id,authority_incarnation,created_at,updated_at) VALUES ('a0000000-0000-4000-8000-000000000002','valid snapshot','linux','x64',1,1,1,'session','session','boot','boot','incarnation',1,1)")
        .execute(&db.pool).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements")
            .fetch_one(&db.pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM capacity_allocations")
            .fetch_one(&db.pool)
            .await?,
        2
    );
    db.close().await
}
