//! Migration and direct-SQL invariants for durable inventory provenance.
//!
//! These checks exercise the actual file-backed `SQLite` schema independently of
//! the service, including the legacy-compatible sample provenance trigger.

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use sqlx::{
    Row as _, SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

struct Database {
    directory: PathBuf,
    pool: SqlitePool,
}

impl Database {
    async fn new() -> Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("gridops-fleet-inventory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(directory.join("inventory.sqlite"))
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { directory, pool })
    }

    async fn close(self) -> Result<()> {
        self.pool.close().await;
        std::fs::remove_dir_all(self.directory)?;
        Ok(())
    }
}

#[tokio::test]
async fn migration_adds_observation_tables_and_sample_provenance_guard() -> Result<()> {
    let db = Database::new().await?;
    for table in [
        "fleet_observation_sessions",
        "fleet_observed_domains",
        "fleet_observed_backends",
        "fleet_observed_interactive_sessions",
        "fleet_inventory_snapshots",
    ] {
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_one(&db.pool)
                .await?;
        assert_eq!(exists, 1, "missing {table}");
    }
    let columns = sqlx::query("SELECT name FROM pragma_table_info('host_samples') ORDER BY cid")
        .fetch_all(&db.pool)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    assert!(
        columns
            .iter()
            .any(|column| column == "authority_session_id")
    );
    assert!(columns.iter().any(|column| column == "boot_id"));
    assert!(
        columns
            .iter()
            .any(|column| column == "control_plane_incarnation")
    );
    db.close().await
}

#[tokio::test]
async fn observation_foreign_keys_and_sample_provenance_are_host_scoped() -> Result<()> {
    let db = Database::new().await?;
    let host = "10000000-0000-4000-8000-000000000001";
    let other_host = "20000000-0000-4000-8000-000000000001";
    let root = "11000000-0000-4000-8000-000000000001";
    let other_root = "21000000-0000-4000-8000-000000000001";
    let credential = "12000000-0000-4000-8000-000000000001";
    let session = "13000000-0000-4000-8000-000000000001";
    sqlx::query("INSERT INTO fleet_hosts (id,name,host_os,architecture,created_at,updated_at) VALUES (?,?,'linux','x64',1,1),(?,?,'windows','x64',1,1)")
        .bind(host).bind("host-a").bind(other_host).bind("host-b").execute(&db.pool).await?;
    sqlx::query("INSERT INTO host_resource_domains (id,host_id,parent_domain_id,name,cpu_millis,memory_mib,disk_bytes) VALUES (?,?,NULL,'root-a',0,0,0),(?,?,NULL,'root-b',0,0,0)")
        .bind(root).bind(host).bind(other_root).bind(other_host).execute(&db.pool).await?;
    sqlx::query("INSERT INTO host_credentials (id,host_id,host_epoch,generation,verifier,verifier_format,created_at) VALUES (?,?,1,1,?,'sha256',1)")
        .bind(credential).bind(host).bind("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").execute(&db.pool).await?;
    sqlx::query("INSERT INTO fleet_observation_sessions (session_id,host_id,host_epoch,boot_id,control_plane_incarnation,negotiated_protocol,initial_credential_id,initial_credential_generation,current_credential_id,current_credential_generation,initial_request_id,initial_body_digest,state,created_at,last_seen_at,lease_expires_at,handshake_received_at,handshake_lease_expires_at,handshake_state) VALUES (?,?,1,'boot','14000000-0000-4000-8000-000000000001',1,?,1,?,1,'15000000-0000-4000-8000-000000000001','AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA','observing',1,1,90001,1,90001,'observing')")
        .bind(session).bind(host).bind(credential).bind(credential).execute(&db.pool).await?;
    let missing_handshake_receipt = sqlx::query("INSERT INTO fleet_observation_sessions (session_id,host_id,host_epoch,boot_id,control_plane_incarnation,negotiated_protocol,initial_credential_id,initial_credential_generation,current_credential_id,current_credential_generation,initial_request_id,initial_body_digest,state,created_at,last_seen_at,lease_expires_at) VALUES ('16000000-0000-4000-8000-000000000001',?,1,'boot','14000000-0000-4000-8000-000000000001',1,?,1,?,1,'15000000-0000-4000-8000-000000000002','AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA','observing',1,1,90001)")
        .bind(host).bind(credential).bind(credential).execute(&db.pool).await;
    assert!(missing_handshake_receipt.is_err());
    let partial_heartbeat_receipt = sqlx::query("UPDATE fleet_observation_sessions SET current_heartbeat_sequence=1,current_heartbeat_digest='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA' WHERE session_id=?")
        .bind(session).execute(&db.pool).await;
    assert!(partial_heartbeat_receipt.is_err());
    let cross_host = sqlx::query("INSERT INTO fleet_observed_domains (host_id,local_key,domain_id,kind,observed_name,session_id,host_epoch,control_plane_incarnation,inventory_sequence,received_at) VALUES (?,?,?,'physical','wrong',?,1,'14000000-0000-4000-8000-000000000001',1,1)")
        .bind(other_host).bind("wrong").bind(root).bind(session).execute(&db.pool).await;
    assert!(cross_host.is_err());
    sqlx::query("INSERT INTO host_samples (host_id,domain_id,host_epoch,observed_at,received_at,coverage,authority_session_id,boot_id,control_plane_incarnation) VALUES (?,?,1,1,1,'partial',?,?,?)")
        .bind(host).bind(root).bind(session).bind("boot").bind("14000000-0000-4000-8000-000000000001").execute(&db.pool).await?;
    sqlx::query("INSERT INTO host_samples (host_id,domain_id,host_epoch,observed_at,received_at,coverage) VALUES (?,?,1,1,1,'partial')")
        .bind(host).bind(root).execute(&db.pool).await?;
    let legacy_update = sqlx::query(
        "UPDATE host_samples SET received_at=2 WHERE host_id=? AND authority_session_id IS NULL",
    )
    .bind(host)
    .execute(&db.pool)
    .await?;
    assert_eq!(legacy_update.rows_affected(), 1);
    let wrong_session = sqlx::query("INSERT INTO host_samples (host_id,domain_id,host_epoch,observed_at,received_at,coverage,authority_session_id,boot_id,control_plane_incarnation) VALUES (?,?,1,1,1,'partial',?,?,?)")
        .bind(host).bind(root).bind("16000000-0000-4000-8000-000000000001").bind("boot").bind("14000000-0000-4000-8000-000000000001").execute(&db.pool).await;
    assert!(wrong_session.is_err());
    let session_identity_update =
        sqlx::query("UPDATE fleet_observation_sessions SET boot_id='rewritten' WHERE session_id=?")
            .bind(session)
            .execute(&db.pool)
            .await;
    assert!(session_identity_update.is_err());
    sqlx::query("UPDATE fleet_observation_sessions SET ended_at=2 WHERE session_id=?")
        .bind(session)
        .execute(&db.pool)
        .await?;
    let second_session = "17000000-0000-4000-8000-000000000001";
    sqlx::query("INSERT INTO fleet_observation_sessions (session_id,host_id,host_epoch,boot_id,control_plane_incarnation,negotiated_protocol,initial_credential_id,initial_credential_generation,current_credential_id,current_credential_generation,initial_request_id,initial_body_digest,state,created_at,last_seen_at,lease_expires_at,handshake_received_at,handshake_lease_expires_at,handshake_state) VALUES (?,?,1,'boot-2','14000000-0000-4000-8000-000000000001',1,?,1,?,1,'15000000-0000-4000-8000-000000000003','AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA','observing',1,1,90001,1,90001,'observing')")
        .bind(second_session).bind(host).bind(credential).bind(credential).execute(&db.pool).await?;
    let erase_sample_provenance = sqlx::query("UPDATE host_samples SET authority_session_id=NULL,boot_id=NULL,control_plane_incarnation=NULL WHERE host_id=?")
        .bind(host).execute(&db.pool).await;
    assert!(erase_sample_provenance.is_err());
    let rebind_sample_provenance = sqlx::query(
        "UPDATE host_samples SET authority_session_id=?,boot_id='boot-2' WHERE host_id=?",
    )
    .bind(second_session)
    .bind(host)
    .execute(&db.pool)
    .await;
    assert!(rebind_sample_provenance.is_err());
    sqlx::query("INSERT INTO fleet_observed_domains (host_id,local_key,domain_id,parent_local_key,kind,observed_name,session_id,host_epoch,control_plane_incarnation,inventory_sequence,received_at) VALUES (?,?,?,NULL,'physical','root',?,1,?,1,1)")
        .bind(host).bind("root").bind(root).bind(session).bind("14000000-0000-4000-8000-000000000001")
        .execute(&db.pool).await?;
    let domain_identity_update = sqlx::query(
        "UPDATE fleet_observed_domains SET runtime_identity='rewritten' WHERE host_id=? AND local_key='root'",
    )
    .bind(host)
    .execute(&db.pool)
    .await;
    assert!(domain_identity_update.is_err());
    sqlx::query("INSERT INTO fleet_inventory_snapshots (host_id,authority_session_id,host_epoch,control_plane_incarnation,inventory_sequence,digest,snapshot_json,received_at,receipt_lease_expires_at,receipt_state,inventory_revision,domain_mappings_json,backend_mappings_json,interactive_mappings_json) VALUES (?,?,1,? ,1,?,'{}',1,90001,'reconciling',1,'[]','[]','[]')")
        .bind(host).bind(session).bind("14000000-0000-4000-8000-000000000001").bind("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        .execute(&db.pool).await?;
    let missing_snapshot_receipt = sqlx::query("INSERT INTO fleet_inventory_snapshots (host_id,authority_session_id,host_epoch,control_plane_incarnation,inventory_sequence,digest,snapshot_json,received_at) VALUES (?,?,1,?,2,?,'{}',1)")
        .bind(host).bind(session).bind("14000000-0000-4000-8000-000000000001").bind("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")
        .execute(&db.pool).await;
    assert!(missing_snapshot_receipt.is_err());
    let snapshot_update = sqlx::query(
        "UPDATE fleet_inventory_snapshots SET snapshot_json='{\"rewritten\":true}' WHERE host_id=? AND authority_session_id=? AND inventory_sequence=1",
    )
    .bind(host)
    .bind(session)
    .execute(&db.pool)
    .await;
    assert!(snapshot_update.is_err());
    db.close().await
}
