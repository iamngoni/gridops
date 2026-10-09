//! Application-service and admission checks over the real fleet schema.

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use super::{AdmissionOutcome, ScanLimit, admit_operation};
use crate::{
    fleet::ids::{AuthoritySessionId, ControlPlaneIncarnation},
    fleet::{
        CiTarget, CiTargetId, FleetError, FleetService, Generation, HostEpoch, Principal,
        ProfileId, SubmissionState, SubmitIntent, WorkloadId, WorkloadKind,
        protocol::{
            agent::{AgentCommandEnvelope, AgentOperation, ExpectedAgentSession, PreparedWorkload},
            primitives::WireTimestamp,
        },
    },
    now_millis,
};
use anyhow::Result;
use sqlx::{Row, SqlitePool, migrate::Migrator, sqlite::SqliteConnectOptions};
use uuid::Uuid;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");
const HOST: &str = "10000000-0000-4000-8000-000000000001";
const BACKEND: &str = "13000000-0000-4000-8000-000000000001";
const TARGET: &str = "30000000-0000-4000-8000-000000000001";
const PROFILE: &str = "40000000-0000-4000-8000-000000000001";
const WINDOWS_HOST: &str = "20000000-0000-4000-8000-000000000001";
const WINDOWS_BACKEND: &str = "23000000-0000-4000-8000-000000000001";
const SESSION: &str = "15000000-0000-4000-8000-000000000001";

struct Database {
    directory: PathBuf,
    pool: SqlitePool,
}

impl Database {
    async fn ready() -> Result<Self> {
        let directory = std::env::temp_dir().join(format!("gridops-admission-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let options = SqliteConnectOptions::new()
            .filename(directory.join("test.sqlite"))
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        MIGRATOR.run_to(21, &pool).await?;
        sqlx::raw_sql(include_str!("../../tests/fixtures/fleet_legacy.sql"))
            .execute(&pool)
            .await?;
        MIGRATOR.run(&pool).await?;
        sqlx::raw_sql(include_str!("../../tests/fixtures/fleet_ready.sql"))
            .execute(&pool)
            .await?;
        Ok(Self { directory, pool })
    }

    async fn close(self) -> Result<()> {
        self.pool.close().await;
        std::fs::remove_dir_all(self.directory)?;
        Ok(())
    }

    async fn open_pool(&self) -> Result<SqlitePool> {
        let options = SqliteConnectOptions::new()
            .filename(self.directory.join("test.sqlite"))
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        Ok(sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?)
    }
}

async fn configure_host(database: &Database, host_id: &str, target_id: &str) -> Result<()> {
    let now = now_millis();
    sqlx::query(
        "UPDATE fleet_hosts
            SET agent_session_id='88888888-8888-4888-8888-888888888888',boot_id='boot-1',authority_incarnation='77777777-7777-4777-8777-777777777777',
                last_heartbeat_at=?,authority_expires_at=?,inventory_complete=0,pressure_state='normal'
          WHERE id=?",
    )
    .bind(now)
    .bind(now + 90_000)
    .bind(host_id)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE fleet_hosts
            SET last_inventory_at=?,inventory_complete=1,inventory_epoch=epoch,
                inventory_session_id=agent_session_id,inventory_boot_id=boot_id
          WHERE id=?",
    )
    .bind(now)
    .bind(host_id)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_target_grants (host_id,target_id,allow_schedule,revision)
         VALUES (?,?,1,1)",
    )
    .bind(host_id)
    .bind(target_id)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_user_grants (host_id,user_id,permission,revision)
         VALUES (?,?,'operator',1)",
    )
    .bind(host_id)
    .bind("legacy-admin")
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_backends
            SET capabilities_json=json_object(
                'schema_version', 1,
                'runtime_kind', runtime_kind,
                'execution_os', execution_os,
                'architecture', architecture,
                'supported_runners', json_array(
                  json_object('target_kind','github_repository','mode','ephemeral'),
                  json_object('target_kind','github_organization','mode','ephemeral'),
                  json_object('target_kind','bitbucket_workspace','mode','persistent'),
                  json_object('target_kind','bitbucket_repository','mode','persistent')
                ),
                'environments', json_array(json_object(
                  'reference','runner:fixture','readiness','ready','version',NULL
                )),
                'resources', json_object(
                  'cpu','reservation','memory','reservation','disk','estimate'
                ),
                'interactive','requires_authorized_helper',
                'docker_socket','requires_explicit_grant'
            ),
            last_probed_at=?,
            probe_epoch=(SELECT epoch FROM fleet_hosts WHERE fleet_hosts.id=host_backends.host_id),
            probe_session_id=(SELECT agent_session_id FROM fleet_hosts WHERE fleet_hosts.id=host_backends.host_id),
            probe_boot_id=(SELECT boot_id FROM fleet_hosts WHERE fleet_hosts.id=host_backends.host_id),
            probe_incarnation=(SELECT authority_incarnation FROM fleet_hosts WHERE fleet_hosts.id=host_backends.host_id)
          WHERE host_id=?",
    )
    .bind(now)
    .bind(host_id)
    .execute(&database.pool)
    .await?;
    Ok(())
}

async fn configure_default(database: &Database) -> Result<()> {
    sqlx::query(
        "INSERT INTO fleet_control_plane
         (singleton,incarnation,readiness,schema_version,protocol_min,protocol_max,updated_at)
         VALUES (1,'77777777-7777-4777-8777-777777777777','active',1,1,1,?)",
    )
    .bind(now_millis())
    .execute(&database.pool)
    .await?;
    configure_host(database, HOST, TARGET).await?;
    sqlx::query("UPDATE pool_execution_profiles SET all_authorized_hosts=1 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "INSERT INTO bitbucket_user_grants (connection_id,user_id,permission,revision)
         VALUES ('legacy-bb','legacy-admin','admin',1)",
    )
    .execute(&database.pool)
    .await?;
    Ok(())
}

fn request(key: &str, workload_id: &str) -> Result<SubmitIntent> {
    request_for(key, workload_id, "legacy-pool", PROFILE, TARGET)
}

fn request_for(
    key: &str,
    workload_id: &str,
    pool_id: &str,
    profile_id: &str,
    target_id: &str,
) -> Result<SubmitIntent> {
    Ok(SubmitIntent {
        idempotency_key: key.to_owned(),
        pool_id: pool_id.to_owned(),
        profile_id: profile_id.parse::<ProfileId>()?,
        target_id: target_id.parse::<CiTargetId>()?,
        workload_id: workload_id.parse::<WorkloadId>()?,
        generation: Generation::new(1)?,
        kind: WorkloadKind::CiRunner,
        agent_run_id: None,
        expected_profile_revision: Some(1),
    })
}

fn principal() -> Principal {
    Principal::AuthenticatedUser {
        user_id: "legacy-admin".to_owned(),
    }
}

async fn assert_prepare_contract(
    database: &Database,
    operation_id: &str,
    expected_workload: PreparedWorkload,
    expected_profile: &str,
    expected_target: &str,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT p.id AS placement_id,p.host_id,p.backend_id,p.host_epoch,p.generation,
                p.profile_revision,p.target_id,p.session_id,p.cpu_millis,p.memory_mib,p.disk_bytes,
                p.config_snapshot_json,c.id AS command_id,c.request_hash,c.payload_json,h.agent_session_id,
                h.authority_incarnation,h.authority_expires_at
           FROM workload_placements p
           JOIN host_commands c ON c.placement_id=p.id AND c.operation_id=p.intent_id
           JOIN fleet_hosts h ON h.id=p.host_id
          WHERE p.intent_id=? AND c.kind='prepare_environment'",
    )
    .bind(operation_id)
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(row.get::<String, _>("target_id"), expected_target);
    assert_eq!(row.get::<String, _>("host_id"), HOST);
    assert_eq!(row.get::<String, _>("backend_id"), BACKEND);
    let profile = sqlx::query("SELECT * FROM pool_execution_profiles WHERE id=?")
        .bind(expected_profile)
        .fetch_one(&database.pool)
        .await?;

    let payload: serde_json::Value = serde_json::from_str(row.get("payload_json"))?;
    let envelope: AgentCommandEnvelope = serde_json::from_value(payload.clone())?;
    assert_eq!(envelope.operation_id.to_string(), operation_id);
    assert_eq!(
        envelope.command_id.to_string(),
        row.get::<String, _>("command_id")
    );
    assert_eq!(envelope.request_hash, envelope.canonical_hash()?);
    assert_eq!(
        row.get::<String, _>("request_hash"),
        envelope.request_hash.as_str()
    );
    let authority = ExpectedAgentSession {
        host_id: row.get::<String, _>("host_id").parse()?,
        host_epoch: HostEpoch::new(row.get::<i64, _>("host_epoch") as u64)?,
        control_plane_incarnation: row
            .get::<String, _>("authority_incarnation")
            .parse::<ControlPlaneIncarnation>()?,
        authority_session_id: row
            .get::<String, _>("agent_session_id")
            .parse::<AuthoritySessionId>()?,
        credential_generation: Generation::new(1)?,
        authority_expires_at: WireTimestamp::from_millis(
            row.get::<i64, _>("authority_expires_at"),
        )?,
    };
    envelope.validate_against(&authority, envelope.issued_at)?;
    assert_eq!(
        envelope.expected_revision.get(),
        row.get::<i64, _>("profile_revision") as u64
    );
    assert_eq!(
        envelope.host_id.to_string(),
        row.get::<String, _>("host_id")
    );
    assert_eq!(
        envelope.host_epoch.get(),
        row.get::<i64, _>("host_epoch") as u64
    );
    match &envelope.scope {
        crate::fleet::protocol::agent::CommandScope::Placement {
            placement_id,
            backend_id,
            generation,
        } => {
            assert_eq!(
                placement_id.to_string(),
                row.get::<String, _>("placement_id")
            );
            assert_eq!(backend_id.to_string(), row.get::<String, _>("backend_id"));
            assert_eq!(generation.get(), row.get::<i64, _>("generation") as u64);
        }
        crate::fleet::protocol::agent::CommandScope::Host => {
            anyhow::bail!("prepare command carried a non-placement scope")
        }
    }

    let allocation_rows = sqlx::query(
        "SELECT id,cpu_millis,memory_mib,disk_bytes FROM capacity_allocations
          WHERE placement_id=? ORDER BY id",
    )
    .bind(row.get::<String, _>("placement_id"))
    .fetch_all(&database.pool)
    .await?;
    assert_eq!(allocation_rows.len(), 2);
    for field in ["cpu_millis", "memory_mib", "disk_bytes"] {
        assert_eq!(row.get::<i64, _>(field), profile.get::<i64, _>(field));
        for allocation in &allocation_rows {
            assert_eq!(
                allocation.get::<i64, _>(field),
                profile.get::<i64, _>(field)
            );
        }
    }
    let allocation_ids: HashSet<String> = allocation_rows
        .iter()
        .map(|allocation| allocation.get::<String, _>("id"))
        .collect();

    let snapshot: serde_json::Value = serde_json::from_str(row.get("config_snapshot_json"))?;
    for field in ["cpu_millis", "memory_mib", "disk_bytes"] {
        assert_eq!(snapshot[field], profile.get::<i64, _>(field));
    }
    for field in ["requires_interactive", "requires_docker_socket"] {
        assert_eq!(snapshot[field], profile.get::<i64, _>(field) != 0);
    }
    for field in [
        "image",
        "runtime_kind",
        "execution_os",
        "architecture",
        "mode",
    ] {
        assert_eq!(snapshot[field], profile.get::<String, _>(field));
    }
    match envelope.operation {
        AgentOperation::PrepareEnvironment(prepare) => {
            assert_eq!(prepare.execution_profile_id.to_string(), expected_profile);
            assert_eq!(
                prepare.profile_revision.get(),
                row.get::<i64, _>("profile_revision") as u64
            );
            assert_eq!(prepare.workload, expected_workload);
            assert_eq!(prepare.image.as_str(), profile.get::<String, _>("image"));
            assert_eq!(
                prepare.configuration.requires_interactive,
                profile.get::<i64, _>("requires_interactive") != 0
            );
            assert_eq!(
                prepare.configuration.requires_docker_socket,
                profile.get::<i64, _>("requires_docker_socket") != 0
            );
            assert_eq!(
                prepare.resource_request.cpu_millis(),
                row.get::<i64, _>("cpu_millis") as u32
            );
            assert_eq!(
                prepare.resource_request.memory_mib(),
                row.get::<i64, _>("memory_mib") as u64
            );
            assert_eq!(
                prepare.resource_request.disk_bytes(),
                row.get::<i64, _>("disk_bytes") as u64
            );
            assert_eq!(
                prepare
                    .allocation_ids
                    .iter()
                    .copied()
                    .map(|value| value.to_string())
                    .collect::<HashSet<_>>(),
                allocation_ids
            );
            assert_eq!(
                serde_json::to_value(&prepare.configuration)?,
                snapshot["config"]
            );
            let persisted_session: Option<String> = row.get("session_id");
            assert_eq!(
                prepare
                    .configuration
                    .session_id
                    .map(|value| value.to_string()),
                persisted_session
            );
            let target = sqlx::query(
                "SELECT kind,installation_id,repository_id FROM fleet_ci_targets WHERE id=?",
            )
            .bind(expected_target)
            .fetch_one(&database.pool)
            .await?;
            assert_eq!(
                prepare.target,
                CiTarget::GithubRepository {
                    installation_id: crate::fleet::ExternalId::new(u64::try_from(
                        target.get::<i64, _>("installation_id"),
                    )?)?,
                    repository_id: crate::fleet::ExternalId::new(u64::try_from(
                        target.get::<i64, _>("repository_id"),
                    )?)?,
                }
            );
        }
        _ => anyhow::bail!("prepare command carried a non-prepare operation"),
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM host_commands WHERE operation_id=? AND kind IN ('start_runner','execute_sandbox')",
        )
        .bind(operation_id)
        .fetch_one(&database.pool)
        .await?,
        0
    );
    Ok(())
}

#[tokio::test]
async fn replays_intent_and_admits_linux_backend_on_macos_host() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;

    let service = FleetService::new(database.pool.clone());
    let principal = principal();
    let request = request("admission-test", "52000000-0000-4000-8000-000000000001")?;
    let first = service.submit_intent(&principal, request.clone()).await?;
    assert_eq!(first.state, SubmissionState::Queued);
    let replay = service.submit_intent(&principal, request).await?;
    assert_eq!(replay.operation_id, first.operation_id);
    assert_eq!(replay.state, SubmissionState::Replay);

    let report = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await?;
    assert_eq!(report.admitted, 1);
    let row = sqlx::query(
        "SELECT p.host_id,p.backend_id,i.status,o.status AS operation_status,c.kind,c.request_hash,o.request_hash AS operation_hash,
                c.payload_json
           FROM workload_placements p JOIN workload_intents i ON i.id=p.intent_id
           JOIN host_commands c ON c.placement_id=p.id JOIN fleet_operations o ON o.id=i.id
          WHERE i.workload_id=?",
    )
    .bind("52000000-0000-4000-8000-000000000001")
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(row.get::<String, _>("host_id"), HOST);
    assert_eq!(row.get::<String, _>("backend_id"), BACKEND);
    assert_eq!(row.get::<String, _>("status"), "admitted");
    assert_eq!(row.get::<String, _>("operation_status"), "running");
    assert_eq!(row.get::<String, _>("kind"), "prepare_environment");
    assert_ne!(
        row.get::<String, _>("request_hash"),
        row.get::<String, _>("operation_hash")
    );
    let payload: serde_json::Value = serde_json::from_str(row.get("payload_json"))?;
    assert_eq!(payload["operation"]["kind"], "prepare_environment");
    assert_eq!(payload["operation"]["execution_profile_id"], PROFILE);
    assert_eq!(
        payload["operation"]["allocation_ids"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let envelope: AgentCommandEnvelope = serde_json::from_value(payload.clone())?;
    let now = now_millis();
    let authority = ExpectedAgentSession {
        host_id: HOST.parse()?,
        host_epoch: HostEpoch::new(1)?,
        control_plane_incarnation: "77777777-7777-4777-8777-777777777777"
            .parse::<ControlPlaneIncarnation>()?,
        authority_session_id: "88888888-8888-4888-8888-888888888888"
            .parse::<AuthoritySessionId>()?,
        credential_generation: Generation::new(1)?,
        authority_expires_at: WireTimestamp::from_millis(now + 90_000)?,
    };
    envelope.validate_against(&authority, WireTimestamp::from_millis(now)?)?;
    assert_eq!(envelope.request_hash, envelope.canonical_hash()?);
    assert_prepare_contract(
        &database,
        &first.operation_id.to_string(),
        PreparedWorkload::CiRunner,
        PROFILE,
        TARGET,
    )
    .await?;
    let snapshot_before: String = sqlx::query_scalar(
        "SELECT config_snapshot_json FROM workload_placements WHERE intent_id=?",
    )
    .bind(first.operation_id.to_string())
    .fetch_one(&database.pool)
    .await?;
    assert!(matches!(
        &envelope.operation,
        AgentOperation::PrepareEnvironment(prepare)
            if prepare.workload == PreparedWorkload::CiRunner
                && prepare.allocation_ids.len() == 2
                && prepare.resource_request.cpu_millis() == 1000
    ));
    sqlx::query("UPDATE pool_execution_profiles SET image='runner:mutated' WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    let persisted_payload: String =
        sqlx::query_scalar("SELECT payload_json FROM host_commands WHERE operation_id=?")
            .bind(first.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?;
    let persisted: serde_json::Value = serde_json::from_str(&persisted_payload)?;
    assert_eq!(persisted, payload);
    assert_eq!(persisted["operation"]["image"], "runner:fixture");
    let snapshot_after: String = sqlx::query_scalar(
        "SELECT config_snapshot_json FROM workload_placements WHERE intent_id=?",
    )
    .bind(first.operation_id.to_string())
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(snapshot_after, snapshot_before);

    database.close().await
}

#[tokio::test]
async fn independently_opened_wal_pools_admit_only_one_last_pool_slot() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE runner_pools SET max_count=5 WHERE id='legacy-pool'")
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE host_id=?",
    )
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    let actor = principal();
    service_a
        .submit_intent(
            &actor,
            request("race-a", "52000000-0000-4000-8000-000000000002")?,
        )
        .await?;
    service_a
        .submit_intent(
            &actor,
            request("race-b", "52000000-0000-4000-8000-000000000003")?,
        )
        .await?;

    let (first, second) = tokio::join!(
        service_a.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?),
        service_b.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
    );
    let first = first?;
    let second = second?;
    assert_eq!(first.admitted + second.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workload_placements WHERE workload_id IN (?,?)",
        )
        .bind("52000000-0000-4000-8000-000000000002")
        .bind("52000000-0000-4000-8000-000000000003")
        .fetch_one(&database.pool)
        .await?,
        1
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn independently_opened_wal_pools_admit_only_one_last_physical_slot() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE runner_pools SET max_count=50 WHERE id='legacy-pool'")
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=2000,memory_mib=100000,disk_bytes=1000000000000
          WHERE id='11000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE id='12000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    let actor = principal();
    service_a
        .submit_intent(
            &actor,
            request("physical-race-a", &Uuid::new_v4().to_string())?,
        )
        .await?;
    service_a
        .submit_intent(
            &actor,
            request("physical-race-b", &Uuid::new_v4().to_string())?,
        )
        .await?;
    let (first, second) = tokio::join!(
        service_a.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?),
        service_b.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
    );
    assert_eq!(first?.admitted + second?.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements WHERE host_id=?")
            .bind(HOST)
            .fetch_one(&database.pool)
            .await?,
        2
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn independently_opened_wal_pools_admit_only_one_last_child_slot() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE runner_pools SET max_count=50 WHERE id='legacy-pool'")
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE id='11000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=2000,memory_mib=3000,disk_bytes=4000000000
          WHERE id='12000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    let actor = principal();
    service_a
        .submit_intent(
            &actor,
            request("child-race-a", &Uuid::new_v4().to_string())?,
        )
        .await?;
    service_a
        .submit_intent(
            &actor,
            request("child-race-b", &Uuid::new_v4().to_string())?,
        )
        .await?;
    let (first, second) = tokio::join!(
        service_a.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?),
        service_b.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
    );
    assert_eq!(first?.admitted + second?.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements WHERE host_id=?",)
            .bind(HOST)
            .fetch_one(&database.pool)
            .await?,
        2
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn ci_and_fix_admission_share_one_physical_budget() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE runner_pools SET max_count=50 WHERE id='legacy-pool'")
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=2000,memory_mib=100000,disk_bytes=1000000000000
          WHERE id='11000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE id='12000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query("UPDATE agent_runs SET status='running' WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    service_a
        .submit_intent(
            &principal(),
            request("ci-physical-race", &Uuid::new_v4().to_string())?,
        )
        .await?;
    let mut fix = request("fix-physical-race", &Uuid::new_v4().to_string())?;
    fix.kind = WorkloadKind::FixSandbox;
    fix.agent_run_id = Some("legacy-fix".to_owned());
    service_a.submit_intent(&Principal::FixAgent, fix).await?;
    let (first, second) = tokio::join!(
        service_a.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?),
        service_b.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
    );
    assert_eq!(first?.admitted + second?.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements WHERE host_id=?")
            .bind(HOST)
            .fetch_one(&database.pool)
            .await?,
        2
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn independently_opened_wal_pools_admit_only_one_last_interactive_slot() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE runner_pools SET max_count=50 WHERE id='legacy-pool'")
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE host_id=?",
    )
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    sqlx::query("UPDATE pool_execution_profiles SET requires_interactive=1 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_target_grants SET allow_interactive=1 WHERE host_id=? AND target_id=?",
    )
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let now = now_millis();
    sqlx::query(
        "INSERT INTO host_sessions
         (id,host_id,session_key,os_user_id,helper_state,authorized,max_jobs,last_seen_at)
         VALUES (?,?,?,?, 'ready',1,1,?)",
    )
    .bind(SESSION)
    .bind(HOST)
    .bind("fixture-session")
    .bind("fixture-user")
    .bind(now)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_session_target_grants(session_id,target_id)
         VALUES (?,?)",
    )
    .bind(SESSION)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    service_a
        .submit_intent(
            &principal(),
            request("session-race-a", &Uuid::new_v4().to_string())?,
        )
        .await?;
    service_a
        .submit_intent(
            &principal(),
            request("session-race-b", &Uuid::new_v4().to_string())?,
        )
        .await?;
    let (first, second) = tokio::join!(
        service_a.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?),
        service_b.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
    );
    assert_eq!(first?.admitted + second?.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workload_placements WHERE session_id=?",
        )
        .bind(SESSION)
        .fetch_one(&database.pool)
        .await?,
        1
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn independent_runner_pools_share_one_interactive_session_slot() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let secondary_pool = "interactive-pool";
    let secondary_profile = "40000000-0000-4000-8000-000000000002";
    sqlx::query(
        "INSERT INTO runner_pools
         (id,installation_id,name,scope,labels,image,provider,providers,macos_runtime,created_at,updated_at)
         VALUES (? ,1,'Independent interactive','repository','[\"self-hosted\"]','runner:fixture','docker','[\"docker\"]','native',1,1)",
    )
    .bind(secondary_pool)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO runner_pool_repositories(pool_id,repository_id,created_at)
         VALUES (?,10,1)",
    )
    .bind(secondary_pool)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO pool_execution_profiles
         (id,pool_id,name,enabled,runtime_kind,execution_os,architecture,mode,
          requires_interactive,all_authorized_hosts,image,labels_json,config_json,
          cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
         VALUES (?,?,'Independent interactive profile',1,'docker','linux','arm64','ephemeral',
                 1,1,'runner:fixture','[\"self-hosted\"]','{}',1000,1024,1073741824,1,1)",
    )
    .bind(secondary_profile)
    .bind(secondary_pool)
    .execute(&database.pool)
    .await?;
    sqlx::query("INSERT INTO profile_ci_targets(profile_id,target_id) VALUES (?,?)")
        .bind(secondary_profile)
        .bind(TARGET)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE runner_pools SET max_count=50 WHERE id IN ('legacy-pool','interactive-pool')",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query("UPDATE host_resource_domains SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000 WHERE host_id=?")
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE pool_execution_profiles SET requires_interactive=1,all_authorized_hosts=1 WHERE id=?",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_target_grants SET allow_interactive=1 WHERE host_id=? AND target_id=?",
    )
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let now = now_millis();
    sqlx::query(
        "INSERT INTO host_sessions
         (id,host_id,session_key,os_user_id,helper_state,authorized,max_jobs,last_seen_at)
         VALUES (?,?,'independent-session','desktop-user','ready',1,1,?)",
    )
    .bind(SESSION)
    .bind(HOST)
    .bind(now)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_session_target_grants(session_id,target_id,revision)
         VALUES (?, ?, 1)",
    )
    .bind(SESSION)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;

    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    let first = service_a
        .submit_intent(
            &principal(),
            request_for(
                "interactive-pool-a",
                &Uuid::new_v4().to_string(),
                "legacy-pool",
                PROFILE,
                TARGET,
            )?,
        )
        .await?;
    let second = service_a
        .submit_intent(
            &principal(),
            request_for(
                "interactive-pool-b",
                &Uuid::new_v4().to_string(),
                secondary_pool,
                secondary_profile,
                TARGET,
            )?,
        )
        .await?;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let barrier_a = Arc::clone(&barrier);
    let barrier_b = Arc::clone(&barrier);
    let (admission_a, admission_b) = tokio::join!(
        async {
            barrier_a.wait().await;
            service_a
                .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
                .await
        },
        async {
            barrier_b.wait().await;
            service_b
                .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
                .await
        }
    );
    let admitted = admission_a?.admitted + admission_b?.admitted;
    assert_eq!(admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workload_placements WHERE session_id=?",
        )
        .bind(SESSION)
        .fetch_one(&database.pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM host_commands WHERE kind='prepare_environment' AND operation_id IN (?,?)",
        )
        .bind(first.operation_id.to_string())
        .bind(second.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        1
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn command_outbox_failure_rolls_back_placement_and_allocations() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request("rollback", "52000000-0000-4000-8000-000000000004")?,
        )
        .await?;
    sqlx::query(
        "CREATE TRIGGER fleet_test_abort_command
         BEFORE INSERT ON host_commands
         BEGIN SELECT RAISE(ABORT,'forced test outbox failure'); END",
    )
    .execute(&database.pool)
    .await?;
    let result = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await;
    assert!(matches!(result, Err(FleetError::Database(_))));
    sqlx::query("DROP TRIGGER fleet_test_abort_command")
        .execute(&database.pool)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements WHERE intent_id=?",)
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM capacity_allocations
              WHERE placement_id IN (SELECT id FROM workload_placements WHERE intent_id=?)",
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_events WHERE target_id=? AND kind='placement_admitted'",
        )
        .bind(TARGET)
        .fetch_one(&database.pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_commands WHERE operation_id=?")
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        0
    );
    let status = service
        .load_operation(&principal(), submission.operation_id)
        .await?;
    assert_eq!(status.status, "queued");
    assert_eq!(status.reason_code, None);
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements WHERE intent_id=?")
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM capacity_allocations
              WHERE placement_id=(SELECT id FROM workload_placements WHERE intent_id=?)",
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_events WHERE target_id=? AND kind='placement_admitted'",
        )
        .bind(TARGET)
        .fetch_one(&database.pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_commands WHERE operation_id=?")
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        1
    );
    database.close().await
}

#[tokio::test]
async fn changed_idempotency_body_and_revoked_policy_are_rejected_at_the_right_stage() -> Result<()>
{
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let first_request = request("same-key", "52000000-0000-4000-8000-000000000005")?;
    service.submit_intent(&principal(), first_request).await?;
    let conflict = service
        .submit_intent(
            &principal(),
            request("same-key", "52000000-0000-4000-8000-000000000006")?,
        )
        .await;
    assert!(matches!(conflict, Err(FleetError::IdempotencyConflict)));
    sqlx::query(
        "UPDATE host_target_grants SET allow_schedule=0,revoked_at=? WHERE host_id=? AND target_id=?",
    )
    .bind(now_millis())
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let report = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await?;
    assert_eq!(report.admitted, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT reason_code FROM workload_intents WHERE workload_id=?",
        )
        .bind("52000000-0000-4000-8000-000000000005")
        .fetch_one(&database.pool)
        .await?,
        "no_eligible_host"
    );
    database.close().await
}

#[tokio::test]
async fn stale_profile_and_unknown_allocation_keep_demand_queued() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let stale = service
        .submit_intent(
            &principal(),
            request("stale-profile", "52000000-0000-4000-8000-000000000007")?,
        )
        .await?;
    sqlx::query("UPDATE pool_execution_profiles SET revision=2 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    let report = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await?;
    assert_eq!(report.admitted, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM workload_intents WHERE id=?",)
            .bind(stale.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        "queued"
    );
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(stale.operation_id.to_string())
        .execute(&database.pool)
        .await?;

    sqlx::query("UPDATE pool_execution_profiles SET revision=1 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    let admitted = service
        .submit_intent(
            &principal(),
            request("unknown-first", "52000000-0000-4000-8000-000000000008")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    sqlx::query(
        "UPDATE capacity_allocations SET state='uncertain'
          WHERE placement_id=(SELECT id FROM workload_placements WHERE intent_id=?)",
    )
    .bind(admitted.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    service
        .submit_intent(
            &principal(),
            request("unknown-second", "52000000-0000-4000-8000-000000000009")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workload_placements p JOIN capacity_allocations a ON a.placement_id=p.id
              JOIN workload_intents i ON i.id=p.intent_id
             WHERE i.workload_id=? AND a.state='uncertain'",
        )
        .bind("52000000-0000-4000-8000-000000000008")
        .fetch_one(&database.pool)
        .await?,
        2
    );
    database.close().await
}

#[tokio::test]
async fn linux_execution_uses_a_windows_physical_host() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    configure_host(&database, WINDOWS_HOST, TARGET).await?;
    sqlx::query(
        "UPDATE pool_execution_profiles SET architecture='x64',all_authorized_hosts=1 WHERE id=?",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request("windows-linux", "52000000-0000-4000-8000-000000000010")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    let row = sqlx::query("SELECT host_id,backend_id FROM workload_placements WHERE intent_id=?")
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?;
    assert_eq!(row.get::<String, _>("host_id"), WINDOWS_HOST);
    assert_eq!(row.get::<String, _>("backend_id"), WINDOWS_BACKEND);
    database.close().await
}

#[tokio::test]
async fn interactive_session_capacity_is_shared_across_queued_demand() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let now = now_millis();
    sqlx::query("UPDATE pool_execution_profiles SET requires_interactive=1 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_target_grants SET allow_interactive=1 WHERE host_id=? AND target_id=?",
    )
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=10000,memory_mib=10000,disk_bytes=107374182400
          WHERE id='12000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_sessions
         (id,host_id,session_key,os_user_id,helper_state,authorized,max_jobs,last_seen_at)
         VALUES (?,?,'session-key','desktop-user','ready',1,1,?)",
    )
    .bind(SESSION)
    .bind(HOST)
    .bind(now)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_session_target_grants (session_id,target_id,revision)
         VALUES (?, ?, 1)",
    )
    .bind(SESSION)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let service = FleetService::new(database.pool.clone());
    service
        .submit_intent(
            &principal(),
            request("session-a", "52000000-0000-4000-8000-000000000011")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    service
        .submit_intent(
            &principal(),
            request("session-b", "52000000-0000-4000-8000-000000000012")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workload_placements p JOIN workload_intents i ON i.id=p.intent_id
              WHERE i.workload_id=?",
        )
        .bind("52000000-0000-4000-8000-000000000012")
        .fetch_one(&database.pool)
        .await?,
        0
    );
    database.close().await
}

#[tokio::test]
async fn missing_backend_capability_evidence_keeps_demand_queued() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE host_backends SET capabilities_json='{}' WHERE host_id=?")
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    service
        .submit_intent(
            &principal(),
            request("missing-capability", "52000000-0000-4000-8000-000000000009")?,
        )
        .await?;

    let report = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await?;
    assert_eq!(report.admitted, 0);
    assert_eq!(report.queued, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT reason_code FROM workload_intents WHERE workload_id=?",
        )
        .bind("52000000-0000-4000-8000-000000000009")
        .fetch_one(&database.pool)
        .await?,
        "capability_unverified"
    );
    database.close().await
}

#[tokio::test]
async fn replay_and_status_recheck_current_installation_permission() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let intent = request("auth-recheck", "52000000-0000-4000-8000-000000000013")?;
    let submission = service.submit_intent(&principal(), intent.clone()).await?;

    sqlx::query("UPDATE users SET role='user' WHERE id='legacy-admin'")
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE user_installations SET permission='read' WHERE user_id='legacy-admin' AND installation_id=1",
    )
    .execute(&database.pool)
    .await?;
    assert!(matches!(
        service.submit_intent(&principal(), intent).await,
        Err(FleetError::Forbidden)
    ));
    assert!(matches!(
        service
            .load_operation(&principal(), submission.operation_id)
            .await,
        Err(FleetError::Forbidden)
    ));
    database.close().await
}

#[tokio::test]
async fn paused_control_plane_leaves_intent_queued() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE fleet_control_plane SET readiness='paused'")
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    service
        .submit_intent(
            &principal(),
            request(
                "paused-control-plane",
                "52000000-0000-4000-8000-000000000014",
            )?,
        )
        .await?;
    let report = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await?;
    assert_eq!(report.admitted, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM workload_intents WHERE workload_id=?",)
            .bind("52000000-0000-4000-8000-000000000014")
            .fetch_one(&database.pool)
            .await?,
        "queued"
    );
    database.close().await
}

#[tokio::test]
async fn missing_legacy_and_restoring_control_plane_states_keep_work_queued() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    sqlx::query("DELETE FROM fleet_control_plane")
        .execute(&database.pool)
        .await?;
    let missing = service
        .submit_intent(
            &principal(),
            request("control-plane-missing", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(missing.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(missing.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "INSERT INTO fleet_control_plane
         (singleton,incarnation,readiness,schema_version,protocol_min,protocol_max,updated_at)
         VALUES (1,'77777777-7777-4777-8777-777777777777','legacy',1,1,1,?)",
    )
    .bind(now_millis())
    .execute(&database.pool)
    .await?;
    let legacy = service
        .submit_intent(
            &principal(),
            request("control-plane-legacy", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(legacy.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(legacy.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE fleet_control_plane SET readiness='restoring'")
        .execute(&database.pool)
        .await?;
    let restoring = service
        .submit_intent(
            &principal(),
            request("control-plane-restoring", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        service
            .load_operation(&principal(), restoring.operation_id)
            .await?
            .reason_code
            .as_deref(),
        Some("control_plane_not_ready")
    );
    database.close().await
}

#[tokio::test]
async fn control_plane_storage_failure_propagates_without_queued_rejection() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request(
                "control-plane-storage-failure",
                "52000000-0000-4000-8000-000000000099",
            )?,
        )
        .await?;
    sqlx::query("DROP TABLE fleet_control_plane")
        .execute(&database.pool)
        .await?;

    let result = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await;
    assert!(matches!(result, Err(FleetError::Database(_))));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM workload_intents WHERE id=?")
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        "queued"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM workload_intents WHERE id=?",
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM fleet_operations WHERE id=?")
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        "queued"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM fleet_operations WHERE id=?",
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_events WHERE target_id=? AND kind='placement_admitted'",
        )
        .bind(TARGET)
        .fetch_one(&database.pool)
        .await?,
        0
    );
    for query in [
        "SELECT COUNT(*) FROM workload_placements WHERE intent_id=?",
        "SELECT COUNT(*) FROM capacity_allocations WHERE placement_id IN (SELECT id FROM workload_placements WHERE intent_id=?)",
        "SELECT COUNT(*) FROM host_commands WHERE operation_id=?",
    ] {
        let value = sqlx::query_scalar::<_, i64>(query)
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?;
        assert_eq!(value, 0, "{query}");
    }
    database.close().await
}

#[tokio::test]
async fn future_heartbeat_and_stale_inventory_identity_are_not_fresh() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let now = now_millis();
    sqlx::query("UPDATE fleet_hosts SET last_heartbeat_at=? WHERE id=?")
        .bind(now + 1_000)
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    service
        .submit_intent(
            &principal(),
            request("future-heartbeat", "52000000-0000-4000-8000-000000000018")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query(
        "UPDATE fleet_hosts
            SET last_heartbeat_at=?,inventory_complete=0,inventory_session_id='stale-session'
          WHERE id=?",
    )
    .bind(now_millis())
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    database.close().await
}

#[tokio::test]
async fn service_enforces_strict_freshness_and_probe_authority_identity() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let exact = service
        .submit_intent(
            &principal(),
            request("exact-heartbeat", &Uuid::new_v4().to_string())?,
        )
        .await?;
    sqlx::query("UPDATE fleet_hosts SET last_heartbeat_at=? WHERE id=?")
        .bind(now_millis() - 45_000)
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(exact.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(exact.operation_id.to_string())
        .execute(&database.pool)
        .await?;

    let now = now_millis();
    sqlx::query("UPDATE fleet_hosts SET last_heartbeat_at=? WHERE id=?")
        .bind(now)
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_backends
            SET probe_epoch=(SELECT epoch FROM fleet_hosts WHERE id=host_backends.host_id)+1
          WHERE host_id=?",
    )
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    let mismatched = service
        .submit_intent(
            &principal(),
            request("probe-epoch-mismatch", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        service
            .load_operation(&principal(), mismatched.operation_id)
            .await?
            .reason_code
            .as_deref(),
        Some("stale_inventory")
    );
    database.close().await
}

#[tokio::test]
async fn service_rejects_probe_boot_session_and_incarnation_rebindings() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    for (key, column, value) in [
        ("probe-boot-mismatch", "probe_boot_id", "other-boot"),
        (
            "probe-session-mismatch",
            "probe_session_id",
            "99999999-9999-4999-8999-999999999999",
        ),
        (
            "probe-incarnation-mismatch",
            "probe_incarnation",
            "99999999-9999-4999-8999-999999999998",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE host_backends SET {column}=? WHERE host_id=?"
        )))
        .bind(value)
        .bind(HOST)
        .execute(&database.pool)
        .await?;
        let submission = service
            .submit_intent(&principal(), request(key, &Uuid::new_v4().to_string())?)
            .await?;
        assert_eq!(
            service
                .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
                .await?
                .admitted,
            0
        );
        assert_eq!(
            service
                .load_operation(&principal(), submission.operation_id)
                .await?
                .reason_code
                .as_deref(),
            Some("stale_inventory")
        );
        sqlx::query(
            "UPDATE host_backends
                SET probe_session_id=(SELECT agent_session_id FROM fleet_hosts WHERE id=host_backends.host_id),
                    probe_boot_id=(SELECT boot_id FROM fleet_hosts WHERE id=host_backends.host_id),
                    probe_incarnation=(SELECT authority_incarnation FROM fleet_hosts WHERE id=host_backends.host_id)
              WHERE host_id=?",
        )
        .bind(HOST)
        .execute(&database.pool)
        .await?;
        sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
            .bind(submission.operation_id.to_string())
            .execute(&database.pool)
            .await?;
        sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
            .bind(submission.operation_id.to_string())
            .execute(&database.pool)
            .await?;
    }
    database.close().await
}

#[tokio::test]
async fn interactive_session_freshness_helper_and_target_grant_are_authoritative() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE pool_execution_profiles SET requires_interactive=1 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_target_grants SET allow_interactive=1 WHERE host_id=? AND target_id=?",
    )
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_sessions
         (id,host_id,session_key,os_user_id,helper_state,authorized,max_jobs,last_seen_at)
         VALUES (?,?,?,?, 'ready',1,1,?)",
    )
    .bind(SESSION)
    .bind(HOST)
    .bind("freshness-session")
    .bind("fixture-user")
    .bind(now_millis() - 45_000)
    .execute(&database.pool)
    .await?;
    sqlx::query("INSERT INTO host_session_target_grants(session_id,target_id) VALUES (?,?)")
        .bind(SESSION)
        .bind(TARGET)
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    let exact = service
        .submit_intent(
            &principal(),
            request("exact-session", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(exact.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(exact.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE host_sessions SET last_seen_at=? WHERE id=?")
        .bind(now_millis() + 1_000)
        .bind(SESSION)
        .execute(&database.pool)
        .await?;
    let future = service
        .submit_intent(
            &principal(),
            request("future-session", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(future.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(future.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE host_sessions SET last_seen_at=?,helper_state='unavailable' WHERE id=?")
        .bind(now_millis())
        .bind(SESSION)
        .execute(&database.pool)
        .await?;
    let unavailable = service
        .submit_intent(
            &principal(),
            request("helper-unavailable", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(unavailable.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE workload_intents SET status='cancelled' WHERE id=?")
        .bind(unavailable.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE host_sessions SET helper_state='ready' WHERE id=?")
        .bind(SESSION)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "UPDATE host_session_target_grants SET revoked_at=? WHERE session_id=? AND target_id=?",
    )
    .bind(now_millis())
    .bind(SESSION)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let revoked = service
        .submit_intent(
            &principal(),
            request("session-grant-revoked", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        service
            .load_operation(&principal(), revoked.operation_id)
            .await?
            .status,
        "queued"
    );
    database.close().await
}

#[tokio::test]
async fn native_and_socket_feature_grants_are_independent() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    sqlx::query(
        "UPDATE pool_execution_profiles
            SET runtime_kind='native_process',execution_os='macos',architecture='arm64',
                requires_docker_socket=0,requires_interactive=0
          WHERE id=?",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    let native_denied = service
        .submit_intent(
            &principal(),
            request("native-grant-denied", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query("UPDATE host_target_grants SET allow_native=1 WHERE host_id=? AND target_id=?")
        .bind(HOST)
        .bind(TARGET)
        .execute(&database.pool)
        .await?;
    assert_eq!(
        service
            .load_operation(&principal(), native_denied.operation_id)
            .await?
            .status,
        "queued"
    );
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    assert_eq!(
        service
            .load_operation(&principal(), native_denied.operation_id)
            .await?
            .status,
        "running"
    );
    let native_allowed = service
        .submit_intent(
            &principal(),
            request("native-grant-allowed", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    assert_eq!(
        service
            .load_operation(&principal(), native_allowed.operation_id)
            .await?
            .status,
        "running"
    );

    sqlx::query(
        "UPDATE pool_execution_profiles
            SET runtime_kind='docker',execution_os='linux',architecture='arm64',
                requires_docker_socket=1
          WHERE id=?",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_target_grants SET allow_docker_socket=0 WHERE host_id=? AND target_id=?",
    )
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    let socket_denied = service
        .submit_intent(
            &principal(),
            request("socket-grant-denied", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query(
        "UPDATE host_target_grants SET allow_docker_socket=1 WHERE host_id=? AND target_id=?",
    )
    .bind(HOST)
    .bind(TARGET)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=4000,memory_mib=4096,disk_bytes=100000000000
          WHERE id='12000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE id='11000000-0000-4000-8000-000000000001'",
    )
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    assert_eq!(
        service
            .load_operation(&principal(), socket_denied.operation_id)
            .await?
            .status,
        "running"
    );
    let socket_allowed = service
        .submit_intent(
            &principal(),
            request("socket-grant-allowed", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    assert_eq!(
        service
            .load_operation(&principal(), socket_allowed.operation_id)
            .await?
            .status,
        "running"
    );
    database.close().await
}

#[tokio::test]
async fn host_allowlist_and_tags_are_both_required() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE pool_execution_profiles SET all_authorized_hosts=0 WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    sqlx::query(r#"UPDATE fleet_hosts SET tags_json='{"region":"za"}' WHERE id=?"#)
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    sqlx::query("INSERT INTO profile_host_selectors (profile_id,kind,host_id) VALUES (?,'host',?)")
        .bind(PROFILE)
        .bind(HOST)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "INSERT INTO profile_host_selectors (profile_id,kind,tag_key,tag_value)
         VALUES (?,'tag','region','us')",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    let service = FleetService::new(database.pool.clone());
    service
        .submit_intent(
            &principal(),
            request("selector-negative", "52000000-0000-4000-8000-000000000015")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query(
        "UPDATE profile_host_selectors SET tag_value='za' WHERE profile_id=? AND kind='tag'",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    database.close().await
}

#[tokio::test]
async fn charged_failed_placement_releases_only_after_proof() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE runner_pools SET max_count=5 WHERE id='legacy-pool'")
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    let first = service
        .submit_intent(
            &principal(),
            request("proof-first", "52000000-0000-4000-8000-000000000016")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    sqlx::query("UPDATE workload_placements SET state='failed' WHERE intent_id=?")
        .bind(first.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    service
        .submit_intent(
            &principal(),
            request("proof-second", "52000000-0000-4000-8000-000000000017")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query(
        "UPDATE workload_placements
            SET local_absence_proven_at=?,delayed_starts_excluded_at=?
          WHERE intent_id=?",
    )
    .bind(now_millis())
    .bind(now_millis())
    .bind(first.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query(
        "UPDATE capacity_allocations
            SET state='released',released_at=?
          WHERE placement_id=(SELECT id FROM workload_placements WHERE intent_id=?)
            AND domain_id='11000000-0000-4000-8000-000000000001'",
    )
    .bind(now_millis())
    .bind(first.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    sqlx::query(
        "UPDATE workload_placements SET upstream_cleanup_state='pending' WHERE intent_id=?",
    )
    .bind(first.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE capacity_allocations
            SET state='released',released_at=?
          WHERE placement_id=(SELECT id FROM workload_placements WHERE intent_id=?)
            AND domain_id='12000000-0000-4000-8000-000000000001'",
    )
    .bind(now_millis())
    .bind(first.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    database.close().await
}

#[tokio::test]
async fn injected_socket_and_session_flags_are_rejected_as_profile_configuration() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    sqlx::query(
        "UPDATE pool_execution_profiles
            SET config_json='{\"requires_docker_socket\":true}'
          WHERE id=?",
    )
    .bind(PROFILE)
    .execute(&database.pool)
    .await?;
    let socket = service
        .submit_intent(
            &principal(),
            request("config-socket", "52000000-0000-4000-8000-000000000019")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT reason_code FROM workload_intents WHERE id=?")
            .bind(socket.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        "invalid_profile"
    );

    sqlx::query("UPDATE pool_execution_profiles SET config_json=? WHERE id=?")
        .bind(format!(
            r#"{{"requires_interactive":true,"session_id":"{SESSION}"}}"#
        ))
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    let interactive = service
        .submit_intent(
            &principal(),
            request("config-session", "52000000-0000-4000-8000-000000000020")?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT session_id FROM workload_placements WHERE intent_id=?",
        )
        .bind(interactive.operation_id.to_string())
        .fetch_optional(&database.pool)
        .await?,
        None
    );
    database.close().await
}

#[tokio::test]
async fn malformed_capabilities_and_probe_identity_leave_demand_queued() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query(
        "UPDATE host_backends SET capabilities_json='{\"not_a_capability\":false}' WHERE host_id=?",
    )
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request(
                "capability-negative",
                "52000000-0000-4000-8000-000000000021",
            )?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM fleet_operations WHERE id=?"
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        Some("capability_unverified".to_owned())
    );
    database.close().await
}

#[tokio::test]
async fn controller_fix_policy_requires_live_run_and_deduplicates() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let mut request = request("fix-terminal", "52000000-0000-4000-8000-000000000022")?;
    request.kind = WorkloadKind::FixSandbox;
    request.agent_run_id = Some("legacy-fix".to_owned());
    assert!(matches!(
        service
            .submit_intent(&Principal::FixAgent, request.clone())
            .await,
        Err(FleetError::Forbidden)
    ));
    sqlx::query("UPDATE agent_runs SET status='running' WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    request.idempotency_key = "fix-live".to_owned();
    let live = service
        .submit_intent(&Principal::FixAgent, request.clone())
        .await?;
    assert!(matches!(
        service
            .submit_intent(
                &Principal::Autoscaler,
                SubmitIntent {
                    idempotency_key: "fix-duplicate".to_owned(),
                    ..request
                },
            )
            .await,
        Err(FleetError::IdempotencyConflict)
    ));
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT kind FROM host_commands WHERE operation_id=?",)
            .bind(live.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        "prepare_environment"
    );
    assert_prepare_contract(
        &database,
        &live.operation_id.to_string(),
        PreparedWorkload::FixSandbox,
        PROFILE,
        TARGET,
    )
    .await?;
    let fix_snapshot_before: String = sqlx::query_scalar(
        "SELECT config_snapshot_json FROM workload_placements WHERE intent_id=?",
    )
    .bind(live.operation_id.to_string())
    .fetch_one(&database.pool)
    .await?;
    let fix_payload_before: serde_json::Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT payload_json FROM host_commands WHERE operation_id=?",
        )
        .bind(live.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
    )?;
    sqlx::query("UPDATE pool_execution_profiles SET image='runner:mutated-fix' WHERE id=?")
        .bind(PROFILE)
        .execute(&database.pool)
        .await?;
    let fix_payload: serde_json::Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>(
            "SELECT payload_json FROM host_commands WHERE operation_id=?",
        )
        .bind(live.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
    )?;
    assert_eq!(fix_payload, fix_payload_before);
    assert_eq!(fix_payload["operation"]["image"], "runner:fixture");
    let fix_snapshot_after: String = sqlx::query_scalar(
        "SELECT config_snapshot_json FROM workload_placements WHERE intent_id=?",
    )
    .bind(live.operation_id.to_string())
    .fetch_one(&database.pool)
    .await?;
    assert_eq!(fix_snapshot_after, fix_snapshot_before);
    database.close().await
}

#[tokio::test]
async fn distinct_fix_workloads_concurrently_deduplicate_by_live_run() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE agent_runs SET status='running',cancel_requested=0 WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    let service_a = FleetService::new(database.pool.clone());
    let pool_b = database.open_pool().await?;
    let service_b = FleetService::new(pool_b.clone());
    let workload_a = Uuid::new_v4().to_string();
    let workload_b = Uuid::new_v4().to_string();
    let mut request_a = request_for(
        "fix-concurrent-a",
        &workload_a,
        "legacy-pool",
        PROFILE,
        TARGET,
    )?;
    request_a.kind = WorkloadKind::FixSandbox;
    request_a.agent_run_id = Some("legacy-fix".to_owned());
    let mut request_b = request_for(
        "fix-concurrent-b",
        &workload_b,
        "legacy-pool",
        PROFILE,
        TARGET,
    )?;
    request_b.kind = WorkloadKind::FixSandbox;
    request_b.agent_run_id = Some("legacy-fix".to_owned());
    let (first, second) = tokio::join!(
        service_a.submit_intent(&Principal::FixAgent, request_a),
        service_b.submit_intent(&Principal::FixAgent, request_b)
    );
    assert_eq!(u8::from(first.is_ok()) + u8::from(second.is_ok()), 1);
    assert!(
        first
            .as_ref()
            .err()
            .or(second.as_ref().err())
            .is_some_and(|error| matches!(error, FleetError::IdempotencyConflict))
    );
    let (admit_a, admit_b) = tokio::join!(
        service_a.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?),
        service_b.run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
    );
    assert_eq!(admit_a?.admitted + admit_b?.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM workload_intents WHERE kind='fix_sandbox' AND agent_run_id='legacy-fix'",
        )
        .fetch_one(&database.pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_operations WHERE principal_kind='fix_agent'",
        )
        .fetch_one(&database.pool)
        .await?,
        1
    );
    pool_b.close().await;
    database.close().await
}

#[tokio::test]
async fn fix_cancel_and_run_repository_ownership_are_current_policy_gates() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    sqlx::query("UPDATE agent_runs SET status='running',cancel_requested=1 WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    let mut cancelled = request("fix-cancel-requested", &Uuid::new_v4().to_string())?;
    cancelled.kind = WorkloadKind::FixSandbox;
    cancelled.agent_run_id = Some("legacy-fix".to_owned());
    assert!(matches!(
        service.submit_intent(&Principal::FixAgent, cancelled).await,
        Err(FleetError::Forbidden)
    ));

    // Both actors have every pool/target grant. Only requested_by differs;
    // changing it back then proves the same live request is otherwise valid.
    sqlx::query("UPDATE agent_runs SET cancel_requested=0 WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    sqlx::query("INSERT INTO users (id,github_id,login,access_token,last_login_at,created_at,updated_at,role) VALUES ('permitted-other-owner',4568,'permitted-other-owner','sealed-fixture',1,1,1,'member')")
        .execute(&database.pool).await?;
    sqlx::query("INSERT INTO user_installations(user_id,installation_id,permission,created_at) VALUES ('permitted-other-owner',1,'admin',1),('permitted-other-owner',2,'admin',1)")
        .execute(&database.pool).await?;
    sqlx::query("INSERT INTO bitbucket_user_grants(connection_id,user_id,permission,revision) VALUES ('legacy-bb','permitted-other-owner','admin',1)")
        .execute(&database.pool).await?;
    sqlx::query("INSERT INTO host_user_grants(host_id,user_id,permission,revision) VALUES (?,'permitted-other-owner','operator',1)")
        .bind(HOST).execute(&database.pool).await?;
    let other_owner = Principal::AuthenticatedUser {
        user_id: "permitted-other-owner".to_owned(),
    };
    let mut owned = request("fix-owner-only", &Uuid::new_v4().to_string())?;
    owned.kind = WorkloadKind::FixSandbox;
    owned.agent_run_id = Some("legacy-fix".to_owned());
    assert!(matches!(
        service.submit_intent(&other_owner, owned.clone()).await,
        Err(FleetError::Forbidden)
    ));
    sqlx::query("UPDATE agent_runs SET requested_by='permitted-other-owner' WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    assert_eq!(
        service.submit_intent(&other_owner, owned).await?.state,
        SubmissionState::Queued
    );

    // The foreign repository has its own job/run, avoiding the live-run-per-job
    // uniqueness constraint so the target identity is the gate being exercised.
    sqlx::query("INSERT INTO workflow_runs (id,repository_id,workflow_id,workflow_name,run_number,run_attempt,event,status,head_branch,head_sha,actor_login,html_url,github_created_at,github_updated_at,created_at,updated_at) VALUES (21,11,5,'Foreign workflow',1,1,'push','completed','main','fixture-sha','fixture-user','https://example.invalid/foreign-run',1,1,1,1)")
        .execute(&database.pool).await?;
    sqlx::query("INSERT INTO workflow_jobs (id,run_id,name,status,conclusion,html_url,created_at,updated_at) VALUES (31,21,'Foreign job','completed','failure','https://example.invalid/foreign-job',1,1)")
        .execute(&database.pool).await?;

    sqlx::query(
        "INSERT INTO agent_runs
         (id,job_id,run_id,repository_id,trigger,status,requested_by,created_at,updated_at)
         VALUES ('foreign-repository-fix',31,21,11,'manual','running','legacy-admin',1,1)",
    )
    .execute(&database.pool)
    .await?;
    let mut foreign = request("fix-foreign-repository", &Uuid::new_v4().to_string())?;
    foreign.kind = WorkloadKind::FixSandbox;
    foreign.agent_run_id = Some("foreign-repository-fix".to_owned());
    assert!(matches!(
        service.submit_intent(&principal(), foreign).await,
        Err(FleetError::Forbidden)
    ));
    database.close().await
}

#[tokio::test]
async fn absent_revision_is_distinct_and_zero_revision_is_invalid() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let mut request = request("revision-distinct", "52000000-0000-4000-8000-000000000023")?;
    request.expected_profile_revision = None;
    service.submit_intent(&principal(), request.clone()).await?;
    request.expected_profile_revision = Some(0);
    assert!(matches!(
        service.submit_intent(&principal(), request).await,
        Err(FleetError::InvalidRequest(_))
    ));
    database.close().await
}

#[tokio::test]
async fn request_boundaries_reject_invalid_keys_and_noninitial_generations() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let oversized = "x".repeat(129);
    for key in ["", "contains\nnewline", oversized.as_str()] {
        assert!(matches!(
            service
                .submit_intent(
                    &principal(),
                    SubmitIntent {
                        idempotency_key: key.to_owned(),
                        workload_id: Uuid::new_v4().to_string().parse()?,
                        ..request("valid-boundary", &Uuid::new_v4().to_string())?
                    },
                )
                .await,
            Err(FleetError::InvalidRequest(_))
        ));
    }
    let mut generation = request("generation-two", &Uuid::new_v4().to_string())?;
    generation.generation = Generation::new(2)?;
    assert!(matches!(
        service.submit_intent(&principal(), generation).await,
        Err(FleetError::InvalidRequest(_))
    ));
    let workload_id = Uuid::new_v4().to_string();
    let first = service
        .submit_intent(&principal(), request("workload-first", &workload_id)?)
        .await?;
    assert!(matches!(
        service
            .submit_intent(&principal(), request("workload-second", &workload_id)?)
            .await,
        Err(FleetError::IdempotencyConflict)
    ));
    assert_eq!(
        service
            .submit_intent(&principal(), request("workload-first", &workload_id)?)
            .await?
            .operation_id,
        first.operation_id
    );
    database.close().await
}

#[tokio::test]
async fn revoked_head_is_durable_and_does_not_rollback_controller_work() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    service
        .submit_intent(
            &principal(),
            request("revoked-head", "52000000-0000-4000-8000-000000000024")?,
        )
        .await?;
    let valid = service
        .submit_intent(
            &Principal::Reconciler,
            request("controller-work", "52000000-0000-4000-8000-000000000025")?,
        )
        .await?;
    sqlx::query("DELETE FROM user_installations WHERE user_id='legacy-admin'")
        .execute(&database.pool)
        .await?;
    let report = service
        .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
        .await?;
    assert_eq!(report.admitted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM fleet_operations WHERE id=?"
        )
        .bind(valid.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        None::<String>
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM workload_placements WHERE intent_id=?",)
            .bind(valid.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        1
    );
    database.close().await
}

#[tokio::test]
async fn target_membership_is_reauthorized_during_admission() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request("target-recheck", "52000000-0000-4000-8000-000000000026")?,
        )
        .await?;
    sqlx::query(
        "DELETE FROM runner_pool_repositories WHERE pool_id='legacy-pool' AND repository_id=10",
    )
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT reason_code FROM workload_intents WHERE id=?")
            .bind(submission.operation_id.to_string())
            .fetch_one(&database.pool)
            .await?,
        "forbidden"
    );
    database.close().await
}

#[tokio::test]
async fn secondary_installation_repository_association_is_currently_authorized() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let secondary = "30000000-0000-4000-8000-000000000003";
    sqlx::query(
        "INSERT INTO fleet_ci_targets
         (id,kind,canonical_key,installation_id,repository_id)
         VALUES (?,'github_repository','github:2:11',2,11)",
    )
    .bind(secondary)
    .execute(&database.pool)
    .await?;
    sqlx::query("INSERT INTO profile_ci_targets(profile_id,target_id) VALUES (?,?)")
        .bind(PROFILE)
        .bind(secondary)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "INSERT INTO host_target_grants (host_id,target_id,allow_schedule,revision)
         VALUES (?, ?, 1, 1)",
    )
    .bind(HOST)
    .bind(secondary)
    .execute(&database.pool)
    .await?;
    let mut secondary_request = request("secondary-installation", &Uuid::new_v4().to_string())?;
    secondary_request.target_id = secondary.parse()?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(&principal(), secondary_request.clone())
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );

    sqlx::query(
        "DELETE FROM runner_pool_repositories
          WHERE pool_id='legacy-pool' AND repository_id=11",
    )
    .execute(&database.pool)
    .await?;
    assert!(matches!(
        service
            .submit_intent(&principal(), secondary_request.clone())
            .await,
        Err(FleetError::Forbidden)
    ));
    assert!(matches!(
        service
            .load_operation(&principal(), submission.operation_id)
            .await,
        Err(FleetError::Forbidden)
    ));
    sqlx::query(
        "INSERT INTO runner_pool_repositories(pool_id,repository_id,created_at)
         VALUES ('legacy-pool',11,1)",
    )
    .execute(&database.pool)
    .await?;
    let mut pending_request = secondary_request.clone();
    pending_request.idempotency_key = "secondary-before-admission-revocation".to_owned();
    pending_request.workload_id = Uuid::new_v4().to_string().parse()?;
    let pending = service
        .submit_intent(&principal(), pending_request.clone())
        .await?;
    sqlx::query(
        "DELETE FROM runner_pool_repositories
          WHERE pool_id='legacy-pool' AND repository_id=11",
    )
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM fleet_operations WHERE id=?",
        )
        .bind(pending.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        Some("forbidden".to_owned())
    );
    assert!(matches!(
        service
            .load_operation(&principal(), pending.operation_id)
            .await,
        Err(FleetError::Forbidden)
    ));
    assert!(matches!(
        service.submit_intent(&principal(), pending_request).await,
        Err(FleetError::Forbidden)
    ));
    database.close().await
}

#[tokio::test]
async fn bitbucket_connection_admin_and_pool_association_are_rechecked() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let bitbucket_profile = "40000000-0000-4000-8000-000000000002";
    let bitbucket_target = "30000000-0000-4000-8000-000000000002";
    sqlx::query(
        "INSERT INTO pool_execution_profiles
         (id,pool_id,name,enabled,runtime_kind,execution_os,architecture,mode,image,
          cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
         VALUES (?,'legacy-pool','Bitbucket persistent',1,'tart_vm','macos','arm64',
                 'persistent','runner:fixture',1000,1024,1073741824,1,1)",
    )
    .bind(bitbucket_profile)
    .execute(&database.pool)
    .await?;
    sqlx::query("INSERT INTO profile_ci_targets(profile_id,target_id) VALUES (?,?)")
        .bind(bitbucket_profile)
        .bind(bitbucket_target)
        .execute(&database.pool)
        .await?;
    let mut bitbucket_request = request("bitbucket-admin", &Uuid::new_v4().to_string())?;
    bitbucket_request.profile_id = bitbucket_profile.parse()?;
    bitbucket_request.target_id = bitbucket_target.parse()?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(&principal(), bitbucket_request.clone())
        .await?;
    assert_eq!(submission.state, SubmissionState::Queued);
    sqlx::query("DELETE FROM runner_pool_bitbucket_connections WHERE pool_id=?")
        .bind("legacy-pool")
        .execute(&database.pool)
        .await?;
    assert!(matches!(
        service
            .submit_intent(&principal(), bitbucket_request.clone())
            .await,
        Err(FleetError::Forbidden)
    ));
    assert!(matches!(
        service
            .load_operation(&principal(), submission.operation_id)
            .await,
        Err(FleetError::Forbidden)
    ));
    database.close().await
}

#[tokio::test]
async fn bitbucket_read_admin_and_revocation_are_distinct_policy_states() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let bitbucket_profile = "40000000-0000-4000-8000-000000000002";
    let bitbucket_target = "30000000-0000-4000-8000-000000000002";
    sqlx::query(
        "INSERT INTO pool_execution_profiles
         (id,pool_id,name,enabled,runtime_kind,execution_os,architecture,mode,image,
          cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
         VALUES (?,'legacy-pool','Bitbucket permission matrix',1,'tart_vm','macos','arm64',
                 'persistent','runner:fixture',1000,1024,1073741824,1,1)",
    )
    .bind(bitbucket_profile)
    .execute(&database.pool)
    .await?;
    sqlx::query("INSERT INTO profile_ci_targets(profile_id,target_id) VALUES (?,?)")
        .bind(bitbucket_profile)
        .bind(bitbucket_target)
        .execute(&database.pool)
        .await?;
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,access_token,last_login_at,created_at,updated_at,role)
         VALUES ('bitbucket-reader',4567,'bitbucket-reader','sealed-fixture',1,1,1,'user')",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO user_installations(user_id,installation_id,permission,created_at)
         VALUES ('bitbucket-reader',1,'admin',1),('bitbucket-reader',2,'admin',1)",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO host_user_grants(host_id,user_id,permission,revision)
         VALUES (?, 'bitbucket-reader','operator',1)",
    )
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO bitbucket_user_grants(connection_id,user_id,permission,revision)
         VALUES ('legacy-bb','bitbucket-reader','read',1)",
    )
    .execute(&database.pool)
    .await?;
    let mut bitbucket_request = request_for(
        "bitbucket-read",
        &Uuid::new_v4().to_string(),
        "legacy-pool",
        bitbucket_profile,
        bitbucket_target,
    )?;
    bitbucket_request.kind = WorkloadKind::CiRunner;
    let reader = Principal::AuthenticatedUser {
        user_id: "bitbucket-reader".to_owned(),
    };
    let service = FleetService::new(database.pool.clone());
    assert!(matches!(
        service
            .submit_intent(&reader, bitbucket_request.clone())
            .await,
        Err(FleetError::Forbidden)
    ));
    sqlx::query(
        "UPDATE bitbucket_user_grants SET permission='admin' WHERE connection_id='legacy-bb' AND user_id='bitbucket-reader'",
    )
    .execute(&database.pool)
    .await?;
    let submission = service
        .submit_intent(&reader, bitbucket_request.clone())
        .await?;
    assert_eq!(submission.state, SubmissionState::Queued);
    sqlx::query(
        "UPDATE bitbucket_user_grants SET revoked_at=?
          WHERE connection_id='legacy-bb' AND user_id='bitbucket-reader'",
    )
    .bind(now_millis())
    .execute(&database.pool)
    .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM fleet_operations WHERE id=?",
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        Some("forbidden".to_owned())
    );
    assert!(matches!(
        service
            .load_operation(&reader, submission.operation_id)
            .await,
        Err(FleetError::Forbidden)
    ));
    assert!(matches!(
        service.submit_intent(&reader, bitbucket_request).await,
        Err(FleetError::Forbidden)
    ));
    database.close().await
}

#[tokio::test]
async fn terminal_and_cancelled_fix_history_remains_readable_but_new_work_is_rejected() -> Result<()>
{
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE agent_runs SET status='running' WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    let mut terminal_request = request("fix-history-terminal", &Uuid::new_v4().to_string())?;
    terminal_request.kind = WorkloadKind::FixSandbox;
    terminal_request.agent_run_id = Some("legacy-fix".to_owned());
    let terminal = service
        .submit_intent(&Principal::FixAgent, terminal_request.clone())
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    sqlx::query("UPDATE agent_runs SET status='succeeded' WHERE id='legacy-fix'")
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE fleet_operations SET status='succeeded' WHERE id=?")
        .bind(terminal.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    assert_eq!(
        service
            .load_operation(&Principal::FixAgent, terminal.operation_id)
            .await?
            .status,
        "succeeded"
    );
    assert_eq!(
        service
            .submit_intent(&Principal::FixAgent, terminal_request.clone())
            .await?
            .state,
        SubmissionState::Replay
    );
    assert!(matches!(
        service
            .submit_intent(
                &Principal::FixAgent,
                SubmitIntent {
                    idempotency_key: "fix-history-terminal-new".to_owned(),
                    workload_id: Uuid::new_v4().to_string().parse()?,
                    ..terminal_request.clone()
                },
            )
            .await,
        Err(FleetError::Forbidden)
    ));
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,access_token,last_login_at,created_at,updated_at,role)
         VALUES ('other-fix-user',999,'other-fix-user','sealed-fixture',1,1,1,'admin')",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "INSERT INTO user_installations(user_id,installation_id,permission,created_at)
         VALUES ('other-fix-user',1,'admin',1)",
    )
    .execute(&database.pool)
    .await?;
    assert!(matches!(
        service
            .submit_intent(
                &Principal::AuthenticatedUser {
                    user_id: "other-fix-user".to_owned(),
                },
                SubmitIntent {
                    idempotency_key: "fix-history-unrelated".to_owned(),
                    workload_id: Uuid::new_v4().to_string().parse()?,
                    ..terminal_request.clone()
                },
            )
            .await,
        Err(FleetError::Forbidden)
    ));

    sqlx::query(
        "INSERT INTO agent_runs
         (id,job_id,run_id,repository_id,trigger,status,requested_by,created_at,updated_at)
         VALUES ('cancelled-fix',30,20,10,'manual','running','legacy-admin',1,1)",
    )
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE host_resource_domains
            SET cpu_millis=100000,memory_mib=100000,disk_bytes=1000000000000
          WHERE host_id=?",
    )
    .bind(HOST)
    .execute(&database.pool)
    .await?;
    let mut cancelled_request = terminal_request;
    cancelled_request.idempotency_key = "fix-history-cancelled".to_owned();
    cancelled_request.workload_id = Uuid::new_v4().to_string().parse()?;
    cancelled_request.agent_run_id = Some("cancelled-fix".to_owned());
    let cancelled = service
        .submit_intent(&Principal::FixAgent, cancelled_request.clone())
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    sqlx::query("UPDATE agent_runs SET status='cancelled' WHERE id='cancelled-fix'")
        .execute(&database.pool)
        .await?;
    sqlx::query("UPDATE fleet_operations SET status='cancelled' WHERE id=?")
        .bind(cancelled.operation_id.to_string())
        .execute(&database.pool)
        .await?;
    assert_eq!(
        service
            .load_operation(&Principal::FixAgent, cancelled.operation_id)
            .await?
            .status,
        "cancelled"
    );
    assert_eq!(
        service
            .submit_intent(&Principal::FixAgent, cancelled_request.clone())
            .await?
            .state,
        SubmissionState::Replay
    );
    assert!(matches!(
        service
            .submit_intent(
                &Principal::FixAgent,
                SubmitIntent {
                    idempotency_key: "fix-history-cancelled-new".to_owned(),
                    workload_id: Uuid::new_v4().to_string().parse()?,
                    ..cancelled_request
                },
            )
            .await,
        Err(FleetError::Forbidden)
    ));
    database.close().await
}

#[tokio::test]
async fn stale_admission_selection_is_a_noop_after_admit_or_cancel() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request("stale-admit", &Uuid::new_v4().to_string())?,
        )
        .await?;
    let mut read_tx = database.pool.begin().await?;
    let stale = crate::fleet::store::intent_for_operation(
        &mut read_tx,
        &submission.operation_id.to_string(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("missing intent"))?;
    read_tx.commit().await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    let mut stale_tx = database.pool.begin_with("BEGIN IMMEDIATE").await?;
    assert_eq!(
        admit_operation(&mut stale_tx, &Principal::Reconciler, &stale.id,).await?,
        AdmissionOutcome::Noop
    );
    stale_tx.commit().await?;
    let status = service
        .load_operation(&principal(), submission.operation_id)
        .await?;
    assert_eq!(status.status, "running");
    assert_eq!(status.reason_code, None);

    let cancelled = service
        .submit_intent(
            &principal(),
            request("stale-cancel", &Uuid::new_v4().to_string())?,
        )
        .await?;
    sqlx::query(
        "UPDATE fleet_operations SET status='cancelled',reason_code='manual_cancel' WHERE id=?",
    )
    .bind(cancelled.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    sqlx::query(
        "UPDATE workload_intents SET status='cancelled',reason_code='manual_cancel' WHERE id=?",
    )
    .bind(cancelled.operation_id.to_string())
    .execute(&database.pool)
    .await?;
    let mut cancelled_tx = database.pool.begin_with("BEGIN IMMEDIATE").await?;
    assert_eq!(
        admit_operation(
            &mut cancelled_tx,
            &Principal::Reconciler,
            &cancelled.operation_id.to_string(),
        )
        .await?,
        AdmissionOutcome::Noop
    );
    cancelled_tx.commit().await?;
    let cancelled_status = service
        .load_operation(&principal(), cancelled.operation_id)
        .await?;
    assert_eq!(cancelled_status.status, "cancelled");
    assert_eq!(
        cancelled_status.reason_code.as_deref(),
        Some("manual_cancel")
    );
    database.close().await
}

#[tokio::test]
async fn successful_retry_clears_previous_queued_reason_from_both_rows() -> Result<()> {
    let database = Database::ready().await?;
    configure_default(&database).await?;
    sqlx::query("UPDATE fleet_control_plane SET readiness='paused'")
        .execute(&database.pool)
        .await?;
    let service = FleetService::new(database.pool.clone());
    let submission = service
        .submit_intent(
            &principal(),
            request("retry-reason", &Uuid::new_v4().to_string())?,
        )
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        0
    );
    let reason = service
        .load_operation(&principal(), submission.operation_id)
        .await?;
    assert_eq!(
        reason.reason_code.as_deref(),
        Some("control_plane_not_ready")
    );
    sqlx::query("UPDATE fleet_control_plane SET readiness='active'")
        .execute(&database.pool)
        .await?;
    assert_eq!(
        service
            .run_scheduling_slice(&Principal::Reconciler, ScanLimit::new(10)?)
            .await?
            .admitted,
        1
    );
    let success = service
        .load_operation(&principal(), submission.operation_id)
        .await?;
    assert_eq!(success.status, "running");
    assert_eq!(success.reason_code, None);
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reason_code FROM workload_intents WHERE id=?",
        )
        .bind(submission.operation_id.to_string())
        .fetch_one(&database.pool)
        .await?,
        None
    );
    database.close().await
}
