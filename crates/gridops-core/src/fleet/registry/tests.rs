//! Transactional registry tests using a file-backed SQLite/WAL fixture.
//!
//! The fixture stays crate-private so test clocks and the test-only vault
//! constructor cannot become production enrollment APIs.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::super::{
    authorization::Principal,
    domain::{Architecture, ExecutionOs},
    ids::{Generation, Revision},
};
use super::enrollment::DEFAULT_ENROLLMENT_TTL_MILLIS;
use super::*;
use crate::{Vault, connect_database_path};

const ADMIN: &str = "registry-admin";
const TARGET: &str = "30000000-0000-4000-8000-000000000001";

#[path = "fault_tests.rs"]
mod fault_tests;
#[path = "invariants_tests.rs"]
mod invariants_tests;

fn inventory_digest() -> String {
    URL_SAFE_NO_PAD.encode([7_u8; 32])
}

#[derive(Debug)]
struct TestClock {
    now: AtomicI64,
    reads: AtomicI64,
}

impl TestClock {
    fn new(now: i64) -> Self {
        Self {
            now: AtomicI64::new(now),
            reads: AtomicI64::new(0),
        }
    }

    fn set(&self, now: i64) {
        self.now.store(now, Ordering::SeqCst);
    }

    fn read_count(&self) -> i64 {
        self.reads.load(Ordering::SeqCst)
    }
}

impl RegistryClock for TestClock {
    fn now_millis(&self) -> i64 {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.now.load(Ordering::SeqCst)
    }
}

struct Fixture {
    directory: std::path::PathBuf,
    pool: SqlitePool,
    clock: Arc<TestClock>,
    service: RegistryService,
}

impl Fixture {
    async fn new(now: i64) -> Result<Self> {
        let directory = std::env::temp_dir().join(format!("gridops-registry-{}", Uuid::new_v4()));
        let pool = connect_database_path(&directory.join("registry.sqlite")).await?;
        sqlx::query(
            "INSERT INTO users
             (id,github_id,login,role,access_token,last_login_at,created_at,updated_at)
             VALUES (?,?,?,'admin','',?,?,?)",
        )
        .bind(ADMIN)
        .bind(99_i64)
        .bind("registry-admin")
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await?;
        let clock = Arc::new(TestClock::new(now));
        let service = RegistryService::with_clock(
            pool.clone(),
            Vault::test_with_secret(b"registry-test-secret"),
            clock.clone(),
        );
        Ok(Self {
            directory,
            pool,
            clock,
            service,
        })
    }

    async fn close(self) -> Result<()> {
        self.pool.close().await;
        std::fs::remove_dir_all(self.directory)?;
        Ok(())
    }

    fn principal() -> Principal {
        Principal::AuthenticatedUser {
            user_id: ADMIN.to_owned(),
        }
    }

    async fn install_target(&self) -> Result<()> {
        let now = self.clock.now_millis();
        sqlx::query(
            "INSERT INTO installations
             (id,account_id,account_login,account_type,target_type,repository_selection,created_at,updated_at)
             VALUES (1,1,'registry','Organization','Organization','selected',?,?)",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO repositories
             (id,installation_id,owner,name,full_name,private,default_branch,html_url,last_synced_at,created_at,updated_at)
             VALUES (10,1,'registry','repo','registry/repo',0,'main','https://example.test/repo',?,?,?)",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO user_installations
             (user_id,installation_id,permission,created_at) VALUES (?,1,'admin',?)",
        )
        .bind(ADMIN)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO fleet_ci_targets
             (id,kind,canonical_key,installation_id,repository_id)
             VALUES (?,'github_repository','registry:1:10',1,10)",
        )
        .bind(TARGET)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn issue_empty(&self, key: &str) -> Result<IssuedEnrollment> {
        let EnrollmentIssue::Created(issue) = self
            .service
            .issue_enrollment(
                &Self::principal(),
                IssueEnrollment {
                    idempotency_key: key.to_owned(),
                    scope: EnrollmentScope::new([])?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await?
        else {
            anyhow::bail!("expected new enrollment")
        };
        Ok(issue)
    }

    async fn consume(
        &self,
        issue: &IssuedEnrollment,
    ) -> std::result::Result<EnrollmentReceipt, RegistryError> {
        self.consume_registration(
            issue,
            HostRegistration::new("registry-host", ExecutionOs::Linux, Architecture::X64, None)?,
        )
        .await
    }

    async fn consume_registration(
        &self,
        issue: &IssuedEnrollment,
        registration: HostRegistration,
    ) -> std::result::Result<EnrollmentReceipt, RegistryError> {
        self.service
            .consume_enrollment(issue.code.expose(), registration)
            .await
    }

    async fn consume_recovery(
        &self,
        issue: &IssuedRecoveryEnrollment,
    ) -> std::result::Result<EnrollmentReceipt, RegistryError> {
        self.service
            .consume_enrollment(
                issue.code.expose(),
                HostRegistration::new(
                    "recovered-host",
                    ExecutionOs::Linux,
                    Architecture::X64,
                    None,
                )?,
            )
            .await
    }

    async fn audit_event_count(&self) -> Result<(i64, i64)> {
        Ok((
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
                .fetch_one(&self.pool)
                .await?,
            sqlx::query_scalar("SELECT COUNT(*) FROM fleet_events")
                .fetch_one(&self.pool)
                .await?,
        ))
    }

    async fn set_current_inventory(
        &self,
        host_id: HostId,
        revision: i64,
        digest: &str,
        complete: bool,
    ) -> Result<()> {
        let now = self.clock.now_millis();
        let session_digest = if OneTimeSecret::parse(digest).is_ok() {
            digest.to_owned()
        } else {
            inventory_digest()
        };
        let (credential_id, credential_generation): (String, i64) = sqlx::query_as(
            "SELECT id,generation FROM host_credentials
               WHERE host_id=? AND revoked_at IS NULL ORDER BY generation DESC LIMIT 1",
        )
        .bind(host_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts
                SET agent_session_id='session-1',boot_id='boot-1',authority_incarnation='inc-1'
              WHERE id=?",
        )
        .bind(host_id.to_string())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts
                SET inventory_complete=?,inventory_revision=?,inventory_digest=?,
                    inventory_epoch=1,inventory_session_id='session-1',inventory_boot_id='boot-1',
                    last_inventory_at=?
              WHERE id=?",
        )
        .bind(i64::from(complete))
        .bind(revision)
        .bind(digest)
        .bind(now)
        .bind(host_id.to_string())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO fleet_observation_sessions
             (session_id,host_id,host_epoch,boot_id,control_plane_incarnation,
              negotiated_protocol,initial_credential_id,initial_credential_generation,
              current_credential_id,current_credential_generation,initial_request_id,
              initial_body_digest,state,created_at,last_seen_at,lease_expires_at,
              handshake_received_at,handshake_lease_expires_at,handshake_state,
              current_inventory_sequence,current_inventory_digest,current_inventory_received_at)
             SELECT 'session-1',id,epoch,'boot-1','inc-1',1,?, ?, ?, ?,
                    'request-1',?, 'observing', ?, ?, ?, ?, ?, 'observing', ?, ?, ?
               FROM fleet_hosts WHERE id=?
             ON CONFLICT(host_id,session_id) DO UPDATE SET
               host_epoch=excluded.host_epoch,boot_id=excluded.boot_id,
               control_plane_incarnation=excluded.control_plane_incarnation,
               current_credential_id=excluded.current_credential_id,
               current_credential_generation=excluded.current_credential_generation,
               state=excluded.state,ended_at=NULL,last_seen_at=excluded.last_seen_at,
               lease_expires_at=excluded.lease_expires_at,
               current_inventory_sequence=excluded.current_inventory_sequence,
               current_inventory_digest=excluded.current_inventory_digest,
               current_inventory_received_at=excluded.current_inventory_received_at",
        )
        .bind(&credential_id)
        .bind(credential_generation)
        .bind(&credential_id)
        .bind(credential_generation)
        .bind(&session_digest)
        .bind(now)
        .bind(now)
        .bind(now + 60_000)
        .bind(now)
        .bind(now + 60_000)
        .bind(1_i64)
        .bind(&session_digest)
        .bind(now)
        .bind(host_id.to_string())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO fleet_inventory_snapshots
             (host_id,authority_session_id,host_epoch,control_plane_incarnation,inventory_sequence,
              digest,snapshot_json,received_at,receipt_lease_expires_at,receipt_state,
              inventory_revision,domain_mappings_json,backend_mappings_json,interactive_mappings_json)
             VALUES (?, 'session-1', 1, 'inc-1', 1, ?, '{}', ?, ?, 'reconciling', ?, '[]', '[]', '[]')",
        )
        .bind(host_id.to_string())
        .bind(&session_digest)
        .bind(now)
        .bind(now + 60_000)
        .bind(revision)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn seed_control_plane(&self) -> Result<()> {
        let now = self.clock.now_millis();
        sqlx::query(
            "INSERT INTO fleet_control_plane
             (singleton,incarnation,readiness,schema_version,protocol_min,protocol_max,updated_at)
             VALUES (1,'inc-1','paused',1,1,1,?)
             ON CONFLICT(singleton) DO UPDATE SET incarnation=excluded.incarnation,
               readiness=excluded.readiness,updated_at=excluded.updated_at",
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn seed_history(&self, host_id: HostId) -> Result<()> {
        let now = self.clock.now_millis();
        let host_id_text = host_id.to_string();
        let domain_id: String = sqlx::query_scalar(
            "SELECT id FROM host_resource_domains
              WHERE host_id=? AND parent_domain_id IS NULL",
        )
        .bind(&host_id_text)
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO runner_pools
             (id,installation_id,repository_id,name,scope,mode,image,created_at,updated_at)
             VALUES ('registry-pool',1,10,'registry-pool','repository','persistent','registry:history',?,?)",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO pool_execution_profiles
             (id,pool_id,name,enabled,runtime_kind,execution_os,architecture,mode,image,
              cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
             VALUES ('registry-profile','registry-pool','registry-profile',1,'docker','linux','x64','persistent',
                     'registry:history',100,128,0,?,?)",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO profile_ci_targets (profile_id,target_id)
             VALUES ('registry-profile',?)",
        )
        .bind(TARGET)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO fleet_operations
             (id,kind,host_id,requested_by_user_id,principal_kind,principal_scope,idempotency_key,
              request_hash,status,created_at,updated_at)
             VALUES ('registry-workload-op','submit_workload',?,'registry-admin','user','user:registry-admin',
                     'history-workload','history-workload','succeeded',?,?)",
        )
        .bind(&host_id_text)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO workload_intents
             (id,workload_id,generation,kind,pool_id,profile_id,target_id,expected_profile_revision,
              status,created_at,updated_at)
             VALUES ('registry-workload-op','registry-workload',1,'ci_runner','registry-pool',
                     'registry-profile',?,1,'admitted',?,?)",
        )
        .bind(TARGET)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO host_backends
             (id,host_id,domain_id,runtime_kind,execution_os,architecture,readiness,enabled,
              capabilities_json,config_json,created_at,updated_at)
             VALUES ('registry-backend',? ,?,'docker','linux','x64','ready',1,'{}','{}',?,?)",
        )
        .bind(&host_id_text)
        .bind(&domain_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO workload_placements
             (id,intent_id,workload_id,generation,host_id,backend_id,host_epoch,profile_revision,
              target_id,config_snapshot_json,cpu_millis,memory_mib,disk_bytes,state,created_at,updated_at)
             VALUES ('registry-placement','registry-workload-op','registry-workload',1,?,'registry-backend',1,1,?,
                     '{}',100,128,0,'running',?,?)",
        )
        .bind(&host_id_text)
        .bind(TARGET)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO capacity_allocations
             (id,placement_id,host_id,backend_id,host_epoch,domain_id,policy_revision,
              cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
             VALUES ('registry-allocation','registry-placement',?,'registry-backend',1,?,1,100,128,0,?,?)",
        )
        .bind(&host_id_text)
        .bind(&domain_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO fleet_operations
             (id,kind,host_id,requested_by_user_id,principal_kind,principal_scope,idempotency_key,
              request_hash,status,created_at,updated_at)
             VALUES ('registry-command-op','edit_host',?,'registry-admin','user','user:registry-admin',
                     'history-command','history-command','succeeded',?,?)",
        )
        .bind(&host_id_text)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO host_commands
             (id,operation_id,scope,host_id,host_epoch,kind,protocol_version,request_hash,payload_json,
              expected_revision,issued_at,deadline_at,idempotency_key)
             VALUES ('registry-command','registry-command-op','host',?,1,'inspect',1,'history-command',
                     '{\"sentinel\":\"history-command\"}',1,?,?,'history-command')",
        )
        .bind(&host_id_text)
        .bind(now)
        .bind(now + 1_000)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO host_samples
             (host_id,domain_id,host_epoch,observed_at,received_at,coverage)
             VALUES (?,?,1,?,?, 'complete')",
        )
        .bind(&host_id_text)
        .bind(&domain_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[tokio::test]
async fn rotation_exchange_ack_and_expiry_replacement_are_durable() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("rotate").await?;
    let enrolled = fixture.consume(&issue).await?;
    let first = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "rotate-1".to_owned(),
            },
        )
        .await?;
    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let next_secret = URL_SAFE_NO_PAD.encode([7_u8; 32]);
    let exchanged = fixture
        .service
        .exchange_rotation(&old, first.operation_id, &next_secret)
        .await?;
    let replay = fixture
        .service
        .exchange_rotation(&old, first.operation_id, &next_secret)
        .await?;
    assert_eq!(exchanged, replay);
    let changed = URL_SAFE_NO_PAD.encode([8_u8; 32]);
    assert!(matches!(
        fixture
            .service
            .exchange_rotation(&old, first.operation_id, &changed)
            .await,
        Err(RegistryError::RotationConflict)
    ));
    fixture.clock.set(1_000 + ROTATION_TTL_MILLIS + 1);
    let next = fixture
        .service
        .authenticate(enrolled.host_id, &next_secret)
        .await?;
    let acknowledged = fixture
        .service
        .acknowledge_rotation(&next, first.operation_id)
        .await?;
    assert_eq!(
        acknowledged,
        fixture
            .service
            .acknowledge_rotation(&next, first.operation_id)
            .await?
    );
    assert!(matches!(
        fixture
            .service
            .authenticate(enrolled.host_id, enrolled.credential.expose())
            .await,
        Err(RegistryError::InvalidCredential)
    ));
    let replacement = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(2)?,
                idempotency_key: "rotate-2".to_owned(),
            },
        )
        .await?;
    assert_eq!(replacement.next_generation.get(), 3);
    fixture.close().await
}

#[tokio::test]
async fn issuer_scope_and_retired_recovery_are_rechecked() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let scoped = EnrollmentScope::new([TARGET.parse()?])?;
    let EnrollmentIssue::Created(issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "scoped".to_owned(),
                scope: scoped,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected new enrollment")
    };
    sqlx::query("UPDATE users SET role='user' WHERE id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .consume_enrollment(
                issue.code.expose(),
                HostRegistration::new("scoped", ExecutionOs::Linux, Architecture::X64, None)?,
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    sqlx::query("UPDATE users SET role='admin' WHERE id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    let enrolled = fixture.consume(&issue).await?;
    sqlx::query("UPDATE fleet_hosts SET lifecycle_state='retired' WHERE id=?")
        .bind(enrolled.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "retired-recovery".to_owned(),
                },
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    fixture.close().await
}

#[tokio::test]
async fn consume_reads_expiry_after_begin_immediate_and_serializes_consumers() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("lock-expiry").await?;
    let lock = fixture.pool.begin_with("BEGIN IMMEDIATE").await?;
    let service = fixture.service.clone();
    let code = issue.code.expose().to_owned();
    let task = tokio::spawn(async move {
        service
            .consume_enrollment(
                &code,
                HostRegistration::new("locked", ExecutionOs::Linux, Architecture::X64, None)?,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture.clock.set(1_000 + DEFAULT_ENROLLMENT_TTL_MILLIS);
    lock.commit().await?;
    assert!(matches!(task.await?, Err(RegistryError::EnrollmentGone)));
    fixture.close().await
}

#[tokio::test]
async fn independent_wal_pools_consume_one_code_once() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("concurrent").await?;
    let second_pool = connect_database_path(&fixture.directory.join("registry.sqlite")).await?;
    let second = RegistryService::with_clock(
        second_pool.clone(),
        Vault::test_with_secret(b"registry-test-secret"),
        fixture.clock.clone(),
    );
    let first_service = fixture.service.clone();
    let code = issue.code.expose().to_owned();
    let first = tokio::spawn(async move {
        first_service
            .consume_enrollment(
                &code,
                HostRegistration::new("one", ExecutionOs::Linux, Architecture::X64, None)?,
            )
            .await
    });
    let code = issue.code.expose().to_owned();
    let second_task = tokio::spawn(async move {
        second
            .consume_enrollment(
                &code,
                HostRegistration::new("two", ExecutionOs::Linux, Architecture::X64, None)?,
            )
            .await
    });
    let first = first.await?;
    let second = second_task.await?;
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fleet_hosts")
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    second_pool.close().await;
    fixture.close().await
}

#[tokio::test]
async fn approval_requires_issued_scope_and_current_inventory_evidence() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let EnrollmentIssue::Created(issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "approval-scope".to_owned(),
                scope: EnrollmentScope::new([TARGET.parse()?])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected new enrollment")
    };
    let enrolled = fixture.consume(&issue).await?;
    let digest = inventory_digest();
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, true)
        .await?;
    fixture.seed_control_plane().await?;
    let base = ApproveHost {
        host_id: enrolled.host_id,
        expected_revision: Revision::new(1)?,
        expected_inventory_revision: 7,
        expected_inventory_digest: digest.clone(),
        scope: EnrollmentScope::new(["30000000-0000-4000-8000-000000000099".parse()?])?,
    };
    assert!(matches!(
        fixture
            .service
            .approve_host(&Fixture::principal(), base)
            .await,
        Err(RegistryError::EnrollmentScopeMismatch)
    ));
    fixture
        .service
        .approve_host(
            &Fixture::principal(),
            ApproveHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                expected_inventory_revision: 7,
                expected_inventory_digest: digest.clone(),
                scope: EnrollmentScope::new([TARGET.parse()?])?,
            },
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT enrollment_state FROM fleet_hosts WHERE id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        "approved"
    );
    fixture.close().await
}

#[tokio::test]
async fn target_permission_revocation_and_installation_suspension_are_rechecked() -> Result<()> {
    // Review 65-68: issuance and consumption must recheck target authority,
    // including revoked installation permission and installation suspension.
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let target = TARGET.parse()?;
    let scoped = || EnrollmentScope::new([target]);

    sqlx::query("UPDATE user_installations SET permission='read' WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .issue_enrollment(
                &Fixture::principal(),
                IssueEnrollment {
                    idempotency_key: "permission-issue-denied".to_owned(),
                    scope: scoped()?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    sqlx::query("UPDATE user_installations SET permission='admin' WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;

    let EnrollmentIssue::Created(permission_issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "permission-consume-denied".to_owned(),
                scope: scoped()?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected permission-scoped enrollment")
    };
    sqlx::query("UPDATE user_installations SET permission='read' WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture.consume(&permission_issue).await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    sqlx::query("UPDATE user_installations SET permission='admin' WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;

    let EnrollmentIssue::Created(suspended_issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "suspension-consume-denied".to_owned(),
                scope: scoped()?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected suspended-installation enrollment")
    };
    sqlx::query("UPDATE installations SET suspended_at=? WHERE id=1")
        .bind(fixture.clock.now_millis())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture.consume(&suspended_issue).await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    sqlx::query("UPDATE installations SET suspended_at=NULL WHERE id=1")
        .execute(&fixture.pool)
        .await?;
    let enrolled = fixture.consume(&suspended_issue).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_target_grants WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        0
    );
    fixture.close().await
}

#[tokio::test]
async fn approval_rejects_inventory_matrix_and_target_authority_loss() -> Result<()> {
    // Review 65-68: wrong revision, digest, incomplete evidence, revoked
    // target permission, suspended installation, and terminal approval are
    // each rejected before grants or lifecycle state are changed.
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let EnrollmentIssue::Created(issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "approval-matrix".to_owned(),
                scope: EnrollmentScope::new([TARGET.parse()?])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected approval enrollment")
    };
    let enrolled = fixture.consume(&issue).await?;
    let digest = inventory_digest();
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, true)
        .await?;
    fixture.seed_control_plane().await?;
    let expected_revision = Revision::new(1)?;
    let approval = |inventory_revision: u64, digest: &str, scope: EnrollmentScope| ApproveHost {
        host_id: enrolled.host_id,
        expected_revision,
        expected_inventory_revision: inventory_revision,
        expected_inventory_digest: digest.to_owned(),
        scope,
    };
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                approval(6, &digest, EnrollmentScope::new([TARGET.parse()?])?),
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                approval(7, "wrong-digest", EnrollmentScope::new([TARGET.parse()?])?),
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, false)
        .await?;
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                approval(7, &digest, EnrollmentScope::new([TARGET.parse()?])?),
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, true)
        .await?;

    sqlx::query("UPDATE user_installations SET permission='read' WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                approval(7, &digest, EnrollmentScope::new([TARGET.parse()?])?),
            )
            .await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    sqlx::query("UPDATE user_installations SET permission='admin' WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    sqlx::query("UPDATE installations SET suspended_at=? WHERE id=1")
        .bind(fixture.clock.now_millis())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                approval(7, &digest, EnrollmentScope::new([TARGET.parse()?])?),
            )
            .await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    sqlx::query("UPDATE installations SET suspended_at=NULL WHERE id=1")
        .execute(&fixture.pool)
        .await?;
    fixture
        .service
        .approve_host(
            &Fixture::principal(),
            approval(7, &digest, EnrollmentScope::new([TARGET.parse()?])?),
        )
        .await?;
    sqlx::query("UPDATE fleet_hosts SET lifecycle_state='retired' WHERE id=?")
        .bind(enrolled.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                approval(7, &digest, EnrollmentScope::new([TARGET.parse()?])?),
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    fixture.close().await
}

#[tokio::test]
async fn enrollment_expiry_revoke_consume_and_recovery_boundaries_are_stable() -> Result<()> {
    // Review 65-69: exact expiry, revoked/consumed code finality, response-loss
    // metadata, and recovery issuance replay expose no second secret/host.
    let fixture = Fixture::new(1_000).await?;
    let EnrollmentIssue::Created(expiring) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "exact-expiry".to_owned(),
                scope: EnrollmentScope::new([])?,
                intended_host_id: None,
                ttl_millis: Some(DEFAULT_ENROLLMENT_TTL_MILLIS),
            },
        )
        .await?
    else {
        anyhow::bail!("expected expiring enrollment")
    };
    fixture.clock.set(1_000 + DEFAULT_ENROLLMENT_TTL_MILLIS);
    assert!(matches!(
        fixture.consume(&expiring).await,
        Err(RegistryError::EnrollmentGone)
    ));

    fixture.clock.set(1_000);
    let revoked = fixture.issue_empty("revoked-code").await?;
    fixture
        .service
        .revoke_enrollment(&Fixture::principal(), revoked.id)
        .await?;
    assert!(matches!(
        fixture.consume(&revoked).await,
        Err(RegistryError::EnrollmentGone)
    ));

    let consumed = fixture.issue_empty("consumed-code").await?;
    let receipt = fixture.consume(&consumed).await?;
    let status = fixture
        .service
        .enrollment_status(&Fixture::principal(), consumed.id)
        .await?;
    assert_eq!(status.consumed_host_id, Some(receipt.host_id));
    assert!(status.revoked_at.is_none());
    assert!(matches!(
        fixture.consume(&consumed).await,
        Err(RegistryError::EnrollmentGone)
    ));

    let replay = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "consumed-code".to_owned(),
                scope: EnrollmentScope::new([])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?;
    assert!(matches!(replay, EnrollmentIssue::Replay(_)));
    fixture.close().await
}

#[tokio::test]
async fn recovery_response_loss_preserves_history_scope_and_fences_old_epoch() -> Result<()> {
    // Review 65-69: recovery has no immediate credential effect, then keeps
    // host identity, grants, allocations, commands, and history bytes while
    // advancing epoch, revoking old credentials, and clearing authority.
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let initial = fixture.issue_empty("recovery-history-initial").await?;
    let enrolled = fixture.consume(&initial).await?;
    sqlx::query(
        "INSERT INTO host_target_grants
         (host_id,target_id,allow_schedule,allow_native,allow_interactive,allow_docker_socket,revision,granted_by)
         VALUES (?, ?, 1, 0, 0, 0, 4, ?)",
    )
    .bind(enrolled.host_id.to_string())
    .bind(TARGET)
    .bind(ADMIN)
    .execute(&fixture.pool)
    .await?;
    fixture.seed_history(enrolled.host_id).await?;
    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let before = (
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM capacity_allocations WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_commands WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_samples WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
    );
    let RecoveryIssue::Created(recovery) = fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                idempotency_key: "recovery-history".to_owned(),
            },
        )
        .await?
    else {
        anyhow::bail!("expected recovery issue")
    };
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_credentials WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "recovery-history".to_owned(),
                },
            )
            .await?,
        RecoveryIssue::Replay(_)
    ));
    let recovered = fixture.consume_recovery(&recovery).await?;
    assert_eq!(recovered.host_id, enrolled.host_id);
    assert_eq!(recovered.credential_generation, 2);
    assert!(matches!(
        fixture
            .service
            .authenticate(enrolled.host_id, enrolled.credential.expose())
            .await,
        Err(RegistryError::InvalidCredential)
    ));
    assert!(matches!(
        fixture.consume_recovery(&recovery).await,
        Err(RegistryError::EnrollmentGone)
    ));
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT epoch,revision,current_credential_generation FROM fleet_hosts WHERE id=?",
        )
        .bind(enrolled.host_id.to_string())
        .fetch_one(&fixture.pool)
        .await?,
        (2, 2, 2)
    );
    assert_eq!(
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<i64>)>(
            "SELECT agent_session_id,boot_id,authority_expires_at FROM fleet_hosts WHERE id=?",
        )
        .bind(enrolled.host_id.to_string())
        .fetch_one(&fixture.pool)
        .await?,
        (None, None, None)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_target_grants WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    assert_eq!(
        (
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM capacity_allocations WHERE host_id=?",
            )
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_commands WHERE host_id=?")
                .bind(enrolled.host_id.to_string())
                .fetch_one(&fixture.pool)
                .await?,
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_samples WHERE host_id=?")
                .bind(enrolled.host_id.to_string())
                .fetch_one(&fixture.pool)
                .await?,
        ),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT payload_json FROM host_commands WHERE id='registry-command'"
        )
        .fetch_one(&fixture.pool)
        .await?,
        r#"{"sentinel":"history-command"}"#
    );
    assert_eq!(old.host_id(), enrolled.host_id);
    fixture.close().await
}

#[tokio::test]
async fn stale_recovery_proof_and_retired_terminal_states_are_rejected() -> Result<()> {
    // Review 65-70: stale recovery epoch/revision and old authenticated proof
    // cannot mutate a recovered host; retired approval, recovery, rotation,
    // and revocation preserve terminal lifecycle finality.
    let fixture = Fixture::new(1_000).await?;
    let initial = fixture.issue_empty("stale-recovery-initial").await?;
    let enrolled = fixture.consume(&initial).await?;
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(2)?,
                    idempotency_key: "stale-recovery-revision".to_owned(),
                },
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    sqlx::query("UPDATE fleet_hosts SET revision=revision+1 WHERE id=?")
        .bind(enrolled.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "stale-recovery-code".to_owned(),
                },
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    sqlx::query("UPDATE fleet_hosts SET revision=1 WHERE id=?")
        .bind(enrolled.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
    let RecoveryIssue::Created(recovery) = fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                idempotency_key: "stale-recovery-code".to_owned(),
            },
        )
        .await?
    else {
        anyhow::bail!("expected recovery issue")
    };
    let recovered = fixture.consume_recovery(&recovery).await?;
    assert!(matches!(
        fixture
            .service
            .exchange_rotation(&old, Uuid::new_v4(), &URL_SAFE_NO_PAD.encode([9_u8; 32]))
            .await,
        Err(RegistryError::InvalidCredential)
    ));
    assert_eq!(recovered.host_id, enrolled.host_id);

    let retired = fixture.issue_empty("retired-terminal").await?;
    let retired_receipt = fixture.consume(&retired).await?;
    sqlx::query("UPDATE fleet_hosts SET lifecycle_state='retired' WHERE id=?")
        .bind(retired_receipt.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: retired_receipt.host_id,
                    expected_generation: Generation::new(1)?,
                    idempotency_key: "retired-rotation".to_owned(),
                },
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: retired_receipt.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "retired-recovery".to_owned(),
                },
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    fixture
        .service
        .revoke_credential(
            &Fixture::principal(),
            retired_receipt.host_id,
            Generation::new(1)?,
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM fleet_hosts WHERE id=?")
            .bind(retired_receipt.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        "retired"
    );
    fixture.close().await
}

#[tokio::test]
async fn issuance_consumption_and_recovery_audit_failures_roll_back() -> Result<()> {
    // Review 65-67 and 70-71: audit/event insertion is part of each write
    // transaction; injected event failure leaves no partial registry state.
    let fixture = Fixture::new(1_000).await?;
    sqlx::query(
        "CREATE TRIGGER fail_registry_issue_event
         BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.enrollment.issued'
         BEGIN SELECT RAISE(ABORT,'registry issue event failure'); END",
    )
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .issue_enrollment(
                &Fixture::principal(),
                IssueEnrollment {
                    idempotency_key: "issue-rollback".to_owned(),
                    scope: EnrollmentScope::new([])?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(fixture.audit_event_count().await?, (0, 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_enrollments")
            .fetch_one(&fixture.pool)
            .await?,
        0
    );
    sqlx::query("DROP TRIGGER fail_registry_issue_event")
        .execute(&fixture.pool)
        .await?;
    let issue = fixture.issue_empty("issue-rollback").await?;
    let baseline = fixture.audit_event_count().await?;

    sqlx::query(
        "CREATE TRIGGER fail_registry_consume_event
         BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.enrollment.consumed'
         BEGIN SELECT RAISE(ABORT,'registry consume event failure'); END",
    )
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture.consume(&issue).await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(fixture.audit_event_count().await?, baseline);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fleet_hosts")
            .fetch_one(&fixture.pool)
            .await?,
        0
    );
    assert!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT consumed_at FROM host_enrollments WHERE id=?",
        )
        .bind(issue.id.to_string())
        .fetch_one(&fixture.pool)
        .await?
        .is_none()
    );
    sqlx::query("DROP TRIGGER fail_registry_consume_event")
        .execute(&fixture.pool)
        .await?;
    let enrolled = fixture.consume(&issue).await?;

    sqlx::query(
        "CREATE TRIGGER fail_registry_recovery_event
         BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.host.recovery_issued'
         BEGIN SELECT RAISE(ABORT,'registry recovery event failure'); END",
    )
    .execute(&fixture.pool)
    .await?;
    let before_recovery = fixture.audit_event_count().await?;
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "recovery-rollback".to_owned(),
                },
            )
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(fixture.audit_event_count().await?, before_recovery);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_enrollments WHERE mode='recovery'")
            .fetch_one(&fixture.pool)
            .await?,
        0
    );
    sqlx::query("DROP TRIGGER fail_registry_recovery_event")
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "recovery-rollback".to_owned(),
                },
            )
            .await?,
        RecoveryIssue::Created(_)
    ));
    fixture.close().await
}

#[tokio::test]
async fn rotation_exchange_ack_and_revoke_audit_failures_roll_back() -> Result<()> {
    // Review 65-70: exchange, acknowledgement, and credential revocation
    // each roll back all state when their audit/event pair cannot commit.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("rotation-rollback").await?;
    let enrolled = fixture.consume(&issue).await?;
    let request = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "rotation-rollback".to_owned(),
            },
        )
        .await?;
    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let next_secret = URL_SAFE_NO_PAD.encode([42_u8; 32]);
    let before_exchange = fixture.audit_event_count().await?;
    sqlx::query(
        "CREATE TRIGGER fail_registry_exchange_event
         BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.credential.rotation_exchanged'
         BEGIN SELECT RAISE(ABORT,'registry exchange event failure'); END",
    )
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .exchange_rotation(&old, request.operation_id, &next_secret)
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(fixture.audit_event_count().await?, before_exchange);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_credentials WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT phase FROM host_credential_rotations WHERE id=?")
            .bind(request.operation_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        "requested"
    );
    sqlx::query("DROP TRIGGER fail_registry_exchange_event")
        .execute(&fixture.pool)
        .await?;
    let exchanged = fixture
        .service
        .exchange_rotation(&old, request.operation_id, &next_secret)
        .await?;
    let next = fixture
        .service
        .authenticate(enrolled.host_id, &next_secret)
        .await?;
    let before_ack = fixture.audit_event_count().await?;
    sqlx::query(
        "CREATE TRIGGER fail_registry_ack_event
         BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.credential.rotation_acknowledged'
         BEGIN SELECT RAISE(ABORT,'registry ack event failure'); END",
    )
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .acknowledge_rotation(&next, request.operation_id)
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(fixture.audit_event_count().await?, before_ack);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT phase FROM host_credential_rotations WHERE id=?")
            .bind(request.operation_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        "exchanged"
    );
    assert!(
        fixture
            .service
            .authenticate(enrolled.host_id, &next_secret)
            .await
            .is_ok()
    );
    sqlx::query("DROP TRIGGER fail_registry_ack_event")
        .execute(&fixture.pool)
        .await?;
    let acknowledged = fixture
        .service
        .acknowledge_rotation(&next, request.operation_id)
        .await?;
    assert_eq!(acknowledged.operation_id, request.operation_id);
    let before_revoke = fixture.audit_event_count().await?;
    sqlx::query(
        "CREATE TRIGGER fail_registry_revoke_event
         BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.credential.revoked'
         BEGIN SELECT RAISE(ABORT,'registry revoke event failure'); END",
    )
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .revoke_credential(&Fixture::principal(), enrolled.host_id, Generation::new(2)?)
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(fixture.audit_event_count().await?, before_revoke);
    assert!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT revoked_at FROM host_credentials WHERE host_id=? AND generation=2",
        )
        .bind(enrolled.host_id.to_string())
        .fetch_one(&fixture.pool)
        .await?
        .is_none()
    );
    sqlx::query("DROP TRIGGER fail_registry_revoke_event")
        .execute(&fixture.pool)
        .await?;
    fixture
        .service
        .revoke_credential(&Fixture::principal(), enrolled.host_id, Generation::new(2)?)
        .await?;
    assert_eq!(exchanged.generation, Generation::new(2)?);
    fixture.close().await
}

#[tokio::test]
async fn rotation_overlap_boundaries_late_ack_and_expired_replacement_are_exact() -> Result<()> {
    // Review 65-70: old authentication is valid immediately before overlap,
    // invalid at the fixed deadline, while the new generation remains valid;
    // ACK remains valid after request TTL and expired unexchanged reservations
    // are revoked before a monotonic replacement is allocated.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("rotation-boundary").await?;
    let enrolled = fixture.consume(&issue).await?;
    let request = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "rotation-boundary".to_owned(),
            },
        )
        .await?;
    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let next_secret = URL_SAFE_NO_PAD.encode([11_u8; 32]);
    fixture.clock.set(request.expires_at - 1);
    let exchanged = fixture
        .service
        .exchange_rotation(&old, request.operation_id, &next_secret)
        .await?;
    assert_eq!(
        exchanged.overlap_expires_at,
        request.expires_at - 1 + ROTATION_OVERLAP_MILLIS
    );
    assert!(
        fixture
            .service
            .authenticate(enrolled.host_id, enrolled.credential.expose())
            .await
            .is_ok()
    );
    fixture.clock.set(exchanged.overlap_expires_at);
    assert!(matches!(
        fixture
            .service
            .authenticate(enrolled.host_id, enrolled.credential.expose())
            .await,
        Err(RegistryError::InvalidCredential)
    ));
    assert!(
        fixture
            .service
            .authenticate(enrolled.host_id, &next_secret)
            .await
            .is_ok()
    );
    fixture.clock.set(request.expires_at + 1);
    let next = fixture
        .service
        .authenticate(enrolled.host_id, &next_secret)
        .await?;
    let acknowledged = fixture
        .service
        .acknowledge_rotation(&next, request.operation_id)
        .await?;
    assert_eq!(
        fixture
            .service
            .acknowledge_rotation(&next, request.operation_id)
            .await?,
        acknowledged
    );

    let second_request = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(2)?,
                idempotency_key: "rotation-expiry".to_owned(),
            },
        )
        .await?;
    fixture.clock.set(second_request.expires_at);
    assert_eq!(
        fixture
            .service
            .rotation_status(&Fixture::principal(), second_request.operation_id)
            .await?
            .state,
        RotationOperationState::Expired
    );
    let replacement = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(2)?,
                idempotency_key: "rotation-expiry-replacement".to_owned(),
            },
        )
        .await?;
    assert_eq!(replacement.next_generation.get(), 4);
    assert_eq!(
        fixture
            .service
            .rotation_status(&Fixture::principal(), second_request.operation_id)
            .await?
            .state,
        RotationOperationState::Revoked
    );
    fixture.close().await
}

#[tokio::test]
async fn competing_rotation_requests_and_exchanges_serialize_across_wal_pools() -> Result<()> {
    // Review 65-70: independently opened WAL pools permit one active request
    // and one first exchange; the loser receives a stable typed conflict.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("rotation-concurrency").await?;
    let enrolled = fixture.consume(&issue).await?;
    let second_pool = connect_database_path(&fixture.directory.join("registry.sqlite")).await?;
    let second_service = RegistryService::with_clock(
        second_pool.clone(),
        Vault::test_with_secret(b"registry-test-secret"),
        fixture.clock.clone(),
    );
    let first_service = fixture.service.clone();
    let first = tokio::spawn(async move {
        first_service
            .request_rotation(
                &Principal::AuthenticatedUser {
                    user_id: ADMIN.to_owned(),
                },
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)
                        .map_err(|_| RegistryError::InvalidRequest("invalid test generation"))?,
                    idempotency_key: "rotation-concurrent-a".to_owned(),
                },
            )
            .await
    });
    let second = tokio::spawn(async move {
        second_service
            .request_rotation(
                &Principal::AuthenticatedUser {
                    user_id: ADMIN.to_owned(),
                },
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)
                        .map_err(|_| RegistryError::InvalidRequest("invalid test generation"))?,
                    idempotency_key: "rotation-concurrent-b".to_owned(),
                },
            )
            .await
    });
    let first = first.await?;
    let second = second.await?;
    let ((Ok(operation), Err(loser)) | (Err(loser), Ok(operation))) = (first, second) else {
        anyhow::bail!("exactly one competing rotation request must win");
    };
    assert!(matches!(loser, RegistryError::RotationConflict));
    let old_first = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let second_auth_service = RegistryService::with_clock(
        connect_database_path(&fixture.directory.join("registry.sqlite")).await?,
        Vault::test_with_secret(b"registry-test-secret"),
        fixture.clock.clone(),
    );
    let old_second = second_auth_service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let first_service = fixture.service.clone();
    let operation_id = operation.operation_id;
    let first_exchange = tokio::spawn(async move {
        first_service
            .exchange_rotation(
                &old_first,
                operation_id,
                &URL_SAFE_NO_PAD.encode([12_u8; 32]),
            )
            .await
    });
    let second_service = RegistryService::with_clock(
        connect_database_path(&fixture.directory.join("registry.sqlite")).await?,
        Vault::test_with_secret(b"registry-test-secret"),
        fixture.clock.clone(),
    );
    let second_exchange = tokio::spawn(async move {
        second_service
            .exchange_rotation(
                &old_second,
                operation_id,
                &URL_SAFE_NO_PAD.encode([13_u8; 32]),
            )
            .await
    });
    let first_exchange = first_exchange.await?;
    let second_exchange = second_exchange.await?;
    let ((Ok(_exchange), Err(loser)) | (Err(loser), Ok(_exchange))) =
        (first_exchange, second_exchange)
    else {
        anyhow::bail!("exactly one competing rotation exchange must win");
    };
    assert!(matches!(loser, RegistryError::RotationConflict));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_credentials WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        2
    );
    second_pool.close().await;
    fixture.close().await
}

#[tokio::test]
async fn rotation_auth_expiry_is_checked_after_lock_wait() -> Result<()> {
    // Review 65-66: an authenticated proof that expires while waiting on the
    // immediate write lock cannot exchange after the lock is released.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("rotation-lock-expiry").await?;
    let enrolled = fixture.consume(&issue).await?;
    sqlx::query(
        "UPDATE host_credentials SET overlap_expires_at=1_100 WHERE host_id=? AND generation=1",
    )
    .bind(enrolled.host_id.to_string())
    .execute(&fixture.pool)
    .await?;
    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let request = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "rotation-lock-expiry".to_owned(),
            },
        )
        .await?;
    let lock = fixture.pool.begin_with("BEGIN IMMEDIATE").await?;
    let service = fixture.service.clone();
    let next_secret = URL_SAFE_NO_PAD.encode([14_u8; 32]);
    let started = Arc::new(AtomicBool::new(false));
    let started_by_task = started.clone();
    let reads_before_wait = fixture.clock.read_count();
    let task = tokio::spawn(async move {
        started_by_task.store(true, Ordering::SeqCst);
        tokio::task::yield_now().await;
        service
            .exchange_rotation(&old, request.operation_id, &next_secret)
            .await
    });
    while !started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    tokio::task::yield_now().await;
    assert_eq!(fixture.clock.read_count(), reads_before_wait);
    fixture.clock.set(1_100);
    lock.commit().await?;
    while fixture.clock.read_count() == reads_before_wait {
        tokio::task::yield_now().await;
    }
    assert!(matches!(task.await?, Err(RegistryError::InvalidCredential)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_credentials WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    fixture.close().await
}

#[tokio::test]
async fn rotation_rejects_wrong_host_and_revoked_authenticated_proofs() -> Result<()> {
    // Review 65-70: an authenticated proof remains bound to host, credential,
    // generation, and epoch; wrong-host and revoked proofs cannot exchange.
    let fixture = Fixture::new(1_000).await?;
    let first = fixture.issue_empty("proof-host-a").await?;
    let host_a = fixture.consume(&first).await?;
    let second = fixture.issue_empty("proof-host-b").await?;
    let host_b = fixture.consume(&second).await?;
    let request = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: host_a.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "proof-host-a".to_owned(),
            },
        )
        .await?;
    let wrong_host = fixture
        .service
        .authenticate(host_b.host_id, host_b.credential.expose())
        .await?;
    assert!(matches!(
        fixture
            .service
            .exchange_rotation(
                &wrong_host,
                request.operation_id,
                &URL_SAFE_NO_PAD.encode([15_u8; 32]),
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    let old = fixture
        .service
        .authenticate(host_a.host_id, host_a.credential.expose())
        .await?;
    fixture
        .service
        .revoke_credential(&Fixture::principal(), host_a.host_id, Generation::new(1)?)
        .await?;
    assert!(matches!(
        fixture
            .service
            .exchange_rotation(
                &old,
                request.operation_id,
                &URL_SAFE_NO_PAD.encode([16_u8; 32]),
            )
            .await,
        Err(RegistryError::InvalidCredential)
    ));
    assert_eq!(
        fixture
            .service
            .rotation_status(&Fixture::principal(), request.operation_id)
            .await?
            .state,
        RotationOperationState::Revoked
    );
    fixture.close().await
}

#[tokio::test]
async fn registry_secret_verifier_and_metadata_outputs_are_redacted() -> Result<()> {
    // Review 65-71: serialized errors, Debug output, audit metadata, and
    // event details never contain plaintext codes, credentials, or verifiers.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("redaction").await?;
    let enrollment_secret = issue.code.expose().to_owned();
    let enrollment_verifier = issue.code.verifier().as_str().to_owned();
    assert_eq!(format!("{:?}", issue.code), "OneTimeSecret(REDACTED)");
    assert!(!format!("{:?}", issue.code).contains(&enrollment_secret));
    assert!(!format!("{:?}", issue.code).contains(&enrollment_verifier));
    let receipt = fixture.consume(&issue).await?;
    let credential_secret = receipt.credential.expose().to_owned();
    let credential_verifier = receipt.credential.verifier().as_str().to_owned();
    assert_eq!(
        format!("{:?}", receipt.credential),
        "OneTimeSecret(REDACTED)"
    );
    let authenticated = fixture
        .service
        .authenticate(receipt.host_id, &credential_secret)
        .await?;
    assert!(!format!("{authenticated:?}").contains(&credential_secret));
    assert!(!format!("{authenticated:?}").contains(&credential_verifier));
    let request = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: receipt.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "redaction-rotation".to_owned(),
            },
        )
        .await?;
    let rotation_secret = URL_SAFE_NO_PAD.encode([17_u8; 32]);
    let rotation_verifier = OneTimeSecret::parse(&rotation_secret)?
        .verifier()
        .as_str()
        .to_owned();
    fixture
        .service
        .exchange_rotation(&authenticated, request.operation_id, &rotation_secret)
        .await?;
    let serialized = sqlx::query_as::<_, (String, String)>(
        "SELECT a.metadata,e.detail_json
           FROM audit_events a JOIN fleet_events e ON e.kind=a.action
          WHERE a.action IN ('fleet.enrollment.issued','fleet.enrollment.consumed',
                             'fleet.credential.rotation_exchanged')",
    )
    .fetch_all(&fixture.pool)
    .await?;
    for (metadata, detail) in serialized {
        assert!(!metadata.contains(&enrollment_secret));
        assert!(!metadata.contains(&enrollment_verifier));
        assert!(!metadata.contains(&credential_secret));
        assert!(!metadata.contains(&credential_verifier));
        assert!(!metadata.contains(&rotation_secret));
        assert!(!metadata.contains(&rotation_verifier));
        assert!(!detail.contains(&enrollment_secret));
        assert!(!detail.contains(&enrollment_verifier));
        assert!(!detail.contains(&credential_secret));
        assert!(!detail.contains(&credential_verifier));
        assert!(!detail.contains(&rotation_secret));
        assert!(!detail.contains(&rotation_verifier));
    }
    let Err(invalid) = fixture
        .service
        .authenticate(receipt.host_id, &"not-a-secret".repeat(4))
        .await
    else {
        anyhow::bail!("malformed credential must be rejected");
    };
    assert!(!format!("{invalid:?}").contains(&credential_secret));
    assert!(!format!("{invalid:?}").contains(&credential_verifier));
    fixture.close().await
}

#[tokio::test]
async fn review2_pending_recovery_retains_original_scope_and_rechecks_authority() -> Result<()> {
    // Review-2 line 7: pending recovery must retain the originating consumed
    // enrollment scope even though the pending host has no target grants.
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let EnrollmentIssue::Created(issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "review2-pending-scope".to_owned(),
                scope: EnrollmentScope::new([TARGET.parse()?])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected scoped issue")
    };
    let enrolled = fixture.consume(&issue).await?;
    sqlx::query("DELETE FROM user_installations WHERE user_id=?")
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "review2-pending-recovery".to_owned(),
                },
            )
            .await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    sqlx::query(
        "INSERT INTO user_installations (user_id,installation_id,permission,created_at)
         VALUES (?,1,'admin',?)",
    )
    .bind(ADMIN)
    .bind(fixture.clock.now_millis())
    .execute(&fixture.pool)
    .await?;
    let RecoveryIssue::Created(recovery) = fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                idempotency_key: "review2-pending-recovery".to_owned(),
            },
        )
        .await?
    else {
        anyhow::bail!("expected retained-scope recovery")
    };
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT scope_json FROM host_enrollments WHERE id=?")
            .bind(recovery.id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        format!(r#"{{"target_ids":["{TARGET}"]}}"#)
    );
    fixture.close().await
}

#[tokio::test]
async fn review2_approval_requires_fresh_bounded_inventory_and_control_plane() -> Result<()> {
    // Review-2 line 9: current authority/session/boot/control-plane identity,
    // canonical digest, and a strictly fresh received timestamp are required.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("review2-stale-inventory").await?;
    let enrolled = fixture.consume(&issue).await?;
    fixture
        .set_current_inventory(enrolled.host_id, 1, "unbounded-digest", true)
        .await?;
    fixture.clock.set(46_000);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fleet_control_plane")
            .fetch_one(&fixture.pool)
            .await?,
        0
    );
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                ApproveHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    expected_inventory_revision: 1,
                    expected_inventory_digest: "unbounded-digest".to_owned(),
                    scope: EnrollmentScope::new([])?,
                },
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    fixture.close().await
}

#[tokio::test]
async fn review2_approval_cannot_remove_scope_without_affected_target_authority() -> Result<()> {
    // Review-2 line 11: approval must authorize both requested and removed
    // target scope; an empty re-approval cannot silently revoke another admin's
    // target grant.
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let EnrollmentIssue::Created(issue) = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "review2-remove-scope".to_owned(),
                scope: EnrollmentScope::new([TARGET.parse()?])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected scoped issue")
    };
    let enrolled = fixture.consume(&issue).await?;
    let digest = inventory_digest();
    fixture
        .set_current_inventory(enrolled.host_id, 1, &digest, true)
        .await?;
    fixture.seed_control_plane().await?;
    sqlx::query("INSERT INTO host_target_grants (host_id,target_id,granted_by) VALUES (?,?,?)")
        .bind(enrolled.host_id.to_string())
        .bind(TARGET)
        .bind(ADMIN)
        .execute(&fixture.pool)
        .await?;
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,role,access_token,last_login_at,created_at,updated_at)
         VALUES ('review2-other-admin',100,'review2-other-admin','admin','',1000,1000,1000)",
    )
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Principal::AuthenticatedUser {
                    user_id: "review2-other-admin".to_owned(),
                },
                ApproveHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    expected_inventory_revision: 1,
                    expected_inventory_digest: digest,
                    scope: EnrollmentScope::new([])?,
                },
            )
            .await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM host_target_grants WHERE host_id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    fixture.close().await
}

#[tokio::test]
async fn review2_authenticated_proof_epoch_is_rechecked() -> Result<()> {
    // Review-2 line 13: a private proof's captured epoch must match the
    // coherent current host and credential rows.
    let fixture = Fixture::new(1_000).await?;
    let issue = fixture.issue_empty("review2-proof-epoch").await?;
    let enrolled = fixture.consume(&issue).await?;
    let proof = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let recovery = fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                idempotency_key: "review2-proof-epoch-recovery".to_owned(),
            },
        )
        .await?;
    let RecoveryIssue::Created(recovery) = recovery else {
        anyhow::bail!("expected new recovery")
    };
    fixture.consume_recovery(&recovery).await?;
    let mut tx = fixture.service.begin().await?;
    assert!(matches!(
        ensure_authenticated_generation(&mut tx, &proof, 1_000).await,
        Err(RegistryError::InvalidCredential)
    ));
    tx.rollback().await?;
    fixture.close().await
}

#[tokio::test]
async fn review2_target_identity_is_checked_for_github_org_and_bitbucket_uuid() -> Result<()> {
    // Review-2 line 15: GitHub organizations bind installation account
    // identity, while Bitbucket workspace UUIDs use canonical brace/case
    // normalization with the current connection grant.
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let organization_target = "40000000-0000-4000-8000-000000000001";
    sqlx::query(
        "INSERT INTO fleet_ci_targets
         (id,kind,canonical_key,installation_id,organization_id)
         VALUES (?,'github_organization','registry:org:1',1,2)",
    )
    .bind(organization_target)
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .issue_enrollment(
                &Fixture::principal(),
                IssueEnrollment {
                    idempotency_key: "review2-org-mismatch".to_owned(),
                    scope: EnrollmentScope::new([organization_target.parse()?])?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await,
        Err(RegistryError::TargetScopeForbidden)
    ));
    let organization_match_target = "40000000-0000-4000-8000-000000000003";
    sqlx::query(
        "INSERT INTO fleet_ci_targets
         (id,kind,canonical_key,installation_id,organization_id)
         VALUES (?,'github_organization','registry:org:2',1,1)",
    )
    .bind(organization_match_target)
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .issue_enrollment(
                &Fixture::principal(),
                IssueEnrollment {
                    idempotency_key: "review2-org-match".to_owned(),
                    scope: EnrollmentScope::new([organization_match_target.parse()?])?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await?,
        EnrollmentIssue::Created(_)
    ));

    let workspace_uuid = "11111111-1111-4111-8111-111111111111";
    let bitbucket_target = "40000000-0000-4000-8000-000000000002";
    sqlx::query(
        "INSERT INTO bitbucket_connections
         (id,name,workspace,workspace_uuid,access_token_key,created_by,created_at,updated_at)
         VALUES ('review2-bb','review2-bb','review2-workspace','{11111111-1111-4111-8111-111111111111}',
                 'review2-bb-key',?,1,1)",
    )
    .bind(ADMIN)
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "INSERT INTO bitbucket_user_grants (connection_id,user_id,permission)
         VALUES ('review2-bb',?,'admin')",
    )
    .bind(ADMIN)
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "INSERT INTO fleet_ci_targets
         (id,kind,canonical_key,bitbucket_connection_id,workspace_uuid)
         VALUES (?,'bitbucket_workspace','registry:bb:workspace','review2-bb',?)",
    )
    .bind(bitbucket_target)
    .bind(workspace_uuid)
    .execute(&fixture.pool)
    .await?;
    assert!(matches!(
        fixture
            .service
            .issue_enrollment(
                &Fixture::principal(),
                IssueEnrollment {
                    idempotency_key: "review2-bb-canonical".to_owned(),
                    scope: EnrollmentScope::new([bitbucket_target.parse()?])?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await?,
        EnrollmentIssue::Created(_)
    ));
    fixture.close().await
}
