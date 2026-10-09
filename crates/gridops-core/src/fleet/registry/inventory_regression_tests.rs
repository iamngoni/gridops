// Regression proof for observation ownership, exact receipts, WAL contention,
// and mixed execution backends. Included under the shared private test fixture.

use super::*;

use std::{
    future::Future,
    sync::atomic::{AtomicI64, AtomicUsize, Ordering},
    task::Poll,
    time::Duration,
};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::fleet::{
    Architecture, BackendReadiness, ExecutionOs, RuntimeKind,
    capabilities::BackendCapabilities,
    ids::{AuthoritySessionId, BackendId, HostEpoch, Revision, SessionId, UpstreamText},
    protocol::inventory::{
        DomainKind, HelperState, InventoryReport, InventoryRequest, ObservationSessionState,
        ObservedBackend, ObservedDomain, ObservedInteractiveSession, ObservedSample,
        SampleCoverage,
    },
    protocol::primitives::{BrowserCounter, WireTimestamp},
    registry::{
        ApproveHost, AuthenticatedHost, EnrollmentIssue, InventoryError, OneTimeSecret,
        RecoverHost, RecoveryIssue, RegistryError, RegistryService, RequestRotation,
    },
};

use crate::{Vault, connect_database_path};

type SampleRow = (String, Option<i64>, Option<i64>, Option<i64>, Option<i64>);

async fn independent_pool(fixture: &Fixture) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(fixture.directory.join("inventory.sqlite"))
                .foreign_keys(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(1), async {
        while pool.num_idle() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(pool)
}

fn handshake_for(
    session_id: AuthoritySessionId,
    boot_id: UpstreamText,
    epoch: HostEpoch,
) -> Result<HandshakeRequest> {
    Ok(HandshakeRequest::new(
        request_id()?,
        session_id,
        boot_id,
        ProtocolRange::new(1, 1)?,
        text("inventory-regression-agent")?,
        Some(epoch),
    ))
}

async fn started_fixture() -> Result<(
    Fixture,
    AuthenticatedHost,
    HandshakeRequest,
    InventoryRequest,
)> {
    let fixture = Fixture::new().await?;
    let (authenticated, _) = fixture.host().await?;
    let incarnation = fixture.incarnation().await?;
    let session_id = AuthoritySessionId::try_new(uuid::Uuid::new_v4())?;
    let boot_id = text("inventory-regression-boot")?;
    let handshake = handshake_for(session_id, boot_id.clone(), authenticated.epoch())?;
    fixture
        .service
        .handshake(&authenticated, handshake.clone())
        .await?;
    let mut report = report()?;
    report.control_plane_incarnation = incarnation;
    report.authority_session_id = session_id;
    report.boot_id = boot_id;
    Ok((
        fixture,
        authenticated,
        handshake,
        InventoryRequest::new(request_id()?, report)?,
    ))
}

fn sequence(request: &InventoryRequest, value: u64) -> Result<InventoryRequest> {
    let mut report = request.report.clone();
    report.inventory_sequence = BrowserCounter::new(value)?;
    Ok(InventoryRequest::new(request_id()?, report)?)
}

fn heartbeat_for(
    request: &InventoryRequest,
    value: u64,
    samples: Vec<ObservedSample>,
) -> Result<HeartbeatRequest> {
    Ok(HeartbeatRequest::new(
        request_id()?,
        request.report.host_epoch,
        request.report.control_plane_incarnation,
        request.report.authority_session_id,
        request.report.boot_id.clone(),
        BrowserCounter::new(value)?,
        request.report.observed_at,
        samples,
    )?)
}

async fn enroll_host(
    fixture: &Fixture,
    idempotency_key: &str,
    name: &str,
    host_os: ExecutionOs,
    architecture: Architecture,
) -> Result<(AuthenticatedHost, String)> {
    let principal = Principal::AuthenticatedUser {
        user_id: "inventory-admin".to_owned(),
    };
    let issue = match fixture
        .service
        .issue_enrollment(
            &principal,
            IssueEnrollment {
                idempotency_key: idempotency_key.to_owned(),
                scope: EnrollmentScope::new([])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    {
        EnrollmentIssue::Created(value) => value,
        EnrollmentIssue::Replay(_) => anyhow::bail!("unexpected enrollment replay"),
    };
    let receipt = fixture
        .service
        .consume_enrollment(
            issue.code.expose(),
            HostRegistration::new(name, host_os, architecture, None)?,
        )
        .await?;
    let secret = receipt.credential.expose().to_owned();
    Ok((
        fixture
            .service
            .authenticate(receipt.host_id, &secret)
            .await?,
        secret,
    ))
}

fn backend_variant(
    local_key: &str,
    domain_local_key: &str,
    runtime_kind: RuntimeKind,
    execution_os: ExecutionOs,
    architecture: Architecture,
) -> Result<ObservedBackend> {
    let mut backend = report()?.backends.remove(0);
    let mut capabilities = backend.capabilities.report().clone();
    capabilities.runtime_kind = runtime_kind;
    capabilities.execution_os = execution_os;
    capabilities.architecture = architecture;
    backend.local_key = text(local_key)?;
    backend.domain_local_key = text(domain_local_key)?;
    backend.runtime_kind = runtime_kind;
    backend.execution_os = execution_os;
    backend.architecture = architecture;
    backend.capabilities = BackendCapabilities::try_from(capabilities)?;
    backend.readiness = BackendReadiness::Ready;
    Ok(backend)
}

fn child_domain(
    local_key: &str,
    parent_local_key: &str,
    kind: DomainKind,
) -> Result<ObservedDomain> {
    Ok(ObservedDomain {
        local_key: text(local_key)?,
        known_id: None,
        parent_local_key: Some(text(parent_local_key)?),
        kind,
        name: text(local_key)?,
        runtime_identity: None,
        capacity: ObservedCapacity {
            cpu_millis: None,
            memory_mib: None,
            disk_bytes: None,
        },
    })
}

fn sample_for(domain_local_key: &str, coverage: SampleCoverage) -> Result<ObservedSample> {
    Ok(ObservedSample {
        domain_local_key: text(domain_local_key)?,
        observed_at: WireTimestamp::from_millis(999_999)?,
        coverage,
        cpu_used_millis: None,
        memory_used_mib: None,
        disk_free_bytes: None,
        uptime_seconds: None,
    })
}

#[derive(Debug)]
struct CountingClock {
    now: AtomicI64,
    reads: AtomicUsize,
}

impl CountingClock {
    fn new(now: i64) -> Self {
        Self {
            now: AtomicI64::new(now),
            reads: AtomicUsize::new(0),
        }
    }

    fn set(&self, now: i64) {
        self.now.store(now, Ordering::SeqCst);
    }
}

impl RegistryClock for CountingClock {
    fn now_millis(&self) -> i64 {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.now.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn wal_independent_pools_have_one_session_and_typed_inventory_losers() -> Result<()> {
    let (fixture, authenticated, handshake, inventory) = started_fixture().await?;
    let pool = independent_pool(&fixture).await?;
    let other = RegistryService::with_clock(
        pool.clone(),
        Vault::test_with_secret(b"inventory-test-secret"),
        fixture.clock.clone(),
    );

    let (first, second) = tokio::join!(
        fixture.service.handshake(&authenticated, handshake.clone()),
        other.handshake(&authenticated, handshake.clone()),
    );
    let first = first?;
    let second = second?;
    assert_eq!(first, second);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_observation_sessions WHERE host_id=?",
        )
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?,
        1
    );

    // A higher sequence can commit first; the reordered lower report is a
    // typed loser and cannot create a second receipt or sample batch.
    let higher = sequence(&inventory, 2)?;
    let accepted = other.submit_inventory(&authenticated, higher).await?;
    assert_eq!(accepted.inventory_sequence.get(), 2);
    assert!(matches!(
        fixture
            .service
            .submit_inventory(&authenticated, inventory)
            .await,
        Err(InventoryError::SequenceConflict)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_inventory_snapshots WHERE host_id=?",
        )
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?,
        1
    );
    let competing = handshake_for(
        AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
        text("competing-boot")?,
        authenticated.epoch(),
    )?;
    assert!(matches!(
        other.handshake(&authenticated, competing).await,
        Err(InventoryError::SessionConflict)
    ));
    let (integrity, state): (String, String) =
        sqlx::query_as("SELECT integrity_state,lifecycle_state FROM fleet_hosts WHERE id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!(
        (integrity.as_str(), state.as_str()),
        ("quarantined", "paused")
    );
    pool.close().await;
    fixture.close().await
}

#[tokio::test]
async fn inventory_reconciliation_rolls_back_child_sample_and_audit_failures() -> Result<()> {
    let (fixture, authenticated, _, mut inventory) = started_fixture().await?;
    inventory
        .report
        .domains
        .push(child_domain("child", "root", DomainKind::Container)?);
    inventory
        .report
        .samples
        .push(sample_for("child", SampleCoverage::Partial)?);
    inventory = InventoryRequest::new(inventory.request_id, inventory.report)?;

    sqlx::query(
        "CREATE TRIGGER fail_inventory_sample BEFORE INSERT ON host_samples
         BEGIN SELECT RAISE(ABORT,'synthetic inventory sample fault'); END",
    )
    .execute(&fixture.pool)
    .await?;
    let sample_outcome = fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await;
    assert!(matches!(sample_outcome, Err(InventoryError::Database(_))));
    sqlx::query("DROP TRIGGER fail_inventory_sample")
        .execute(&fixture.pool)
        .await?;

    let zero_after_sample_fault: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM fleet_observed_domains WHERE host_id=?),
           (SELECT COUNT(*) FROM fleet_observed_backends WHERE host_id=?),
           (SELECT COUNT(*) FROM host_samples WHERE host_id=?),
           (SELECT COUNT(*) FROM fleet_inventory_snapshots WHERE host_id=?),
           (SELECT inventory_complete FROM fleet_hosts WHERE id=?)",
    )
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(zero_after_sample_fault, (0, 0, 0, 0, 0));

    sqlx::query(
        "CREATE TRIGGER fail_inventory_audit BEFORE INSERT ON fleet_events
         WHEN NEW.kind='fleet.observation.inventory_received'
         BEGIN SELECT RAISE(ABORT,'synthetic inventory audit fault'); END",
    )
    .execute(&fixture.pool)
    .await?;
    let audit_outcome = fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await;
    assert!(matches!(
        audit_outcome,
        Err(InventoryError::Registry(RegistryError::Database(_)))
    ));
    sqlx::query("DROP TRIGGER fail_inventory_audit")
        .execute(&fixture.pool)
        .await?;
    let zero_after_audit_fault: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM fleet_observed_domains WHERE host_id=?),
           (SELECT COUNT(*) FROM fleet_observed_backends WHERE host_id=?),
           (SELECT COUNT(*) FROM host_samples WHERE host_id=?),
           (SELECT COUNT(*) FROM fleet_inventory_snapshots WHERE host_id=?),
           (SELECT inventory_complete FROM fleet_hosts WHERE id=?)",
    )
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(zero_after_audit_fault, (0, 0, 0, 0, 0));

    fixture
        .service
        .submit_inventory(&authenticated, inventory)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fleet_observed_domains WHERE host_id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?,
        2
    );
    fixture.close().await
}

#[tokio::test]
async fn lock_wait_reads_expiry_after_transaction_lock_and_accepts_exact_expiry() -> Result<()> {
    let (fixture, authenticated, _, _) = started_fixture().await?;
    let pool = independent_pool(&fixture).await?;
    let clock = std::sync::Arc::new(CountingClock::new(1_000));
    let service = RegistryService::with_clock(
        pool.clone(),
        Vault::test_with_secret(b"inventory-test-secret"),
        clock.clone(),
    );
    let lock = fixture.pool.begin_with("BEGIN IMMEDIATE").await?;
    let competing = handshake_for(
        AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
        text("expiry-after-lock")?,
        authenticated.epoch(),
    )?;
    let mut future = Box::pin(service.handshake(&authenticated, competing));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert_eq!(clock.reads.load(Ordering::SeqCst), 0);
    clock.set(91_000);
    lock.commit().await?;
    future.await?;
    let ended: Option<i64> = sqlx::query_scalar(
        "SELECT ended_at FROM fleet_observation_sessions WHERE session_id=?",
    )
    .bind(
        sqlx::query_scalar::<_, String>(
            "SELECT session_id FROM fleet_observation_sessions WHERE host_id=? ORDER BY created_at LIMIT 1",
        )
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?,
    )
    .fetch_one(&fixture.pool)
    .await?;
    assert!(ended.is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_observation_sessions WHERE host_id=? AND ended_at IS NULL",
        )
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?,
        1
    );
    pool.close().await;
    fixture.close().await
}

#[tokio::test]
async fn rotation_before_and_after_ack_advances_the_same_observation_session() -> Result<()> {
    let (fixture, authenticated, handshake, inventory) = started_fixture().await?;
    let principal = Principal::AuthenticatedUser {
        user_id: "inventory-admin".to_owned(),
    };
    let rotation = fixture
        .service
        .request_rotation(
            &principal,
            RequestRotation {
                host_id: authenticated.host_id(),
                expected_generation: authenticated.generation(),
                idempotency_key: "inventory-session-rotation".to_owned(),
            },
        )
        .await?;
    let next_secret = OneTimeSecret::generate();
    fixture
        .service
        .exchange_rotation(&authenticated, rotation.operation_id, next_secret.expose())
        .await?;
    let next = fixture
        .service
        .authenticate(authenticated.host_id(), next_secret.expose())
        .await?;
    let before_ack = fixture.service.handshake(&next, handshake.clone()).await?;
    assert_eq!(before_ack.agent_session_id, handshake.agent_session_id);
    fixture
        .service
        .submit_inventory(&next, inventory.clone())
        .await?;
    fixture
        .service
        .acknowledge_rotation(&next, rotation.operation_id)
        .await?;
    let after_ack = fixture.service.handshake(&next, handshake).await?;
    assert_eq!(after_ack, before_ack);
    fixture.service.submit_inventory(&next, inventory).await?;
    assert!(
        fixture
            .service
            .authenticate(authenticated.host_id(), "invalid")
            .await
            .is_err()
    );
    let current: (i64, String) = sqlx::query_as(
        "SELECT current_credential_generation,state FROM fleet_observation_sessions WHERE host_id=?",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(current, (2, "reconciling".to_owned()));
    fixture.close().await
}

#[tokio::test]
async fn stale_host_tuple_conflicted_session_and_obsolete_incarnation_are_fenced() -> Result<()> {
    let (fixture, authenticated, handshake, inventory) = started_fixture().await?;
    sqlx::query("UPDATE fleet_hosts SET agent_session_id=?,boot_id=? WHERE id=?")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind("replacement-boot")
        .bind(authenticated.host_id().to_string())
        .execute(&fixture.pool)
        .await?;
    let stale_heartbeat = heartbeat_for(&inventory, 1, Vec::new())?;
    assert!(matches!(
        fixture
            .service
            .heartbeat(&authenticated, stale_heartbeat)
            .await,
        Err(InventoryError::IdentityConflict)
    ));

    sqlx::query("UPDATE fleet_hosts SET agent_session_id=?,boot_id=? WHERE id=?")
        .bind(handshake.agent_session_id.to_string())
        .bind(handshake.boot_id.as_str())
        .bind(authenticated.host_id().to_string())
        .execute(&fixture.pool)
        .await?;
    sqlx::query("UPDATE fleet_observation_sessions SET state='conflicted' WHERE host_id=?")
        .bind(authenticated.host_id().to_string())
        .execute(&fixture.pool)
        .await?;
    assert!(matches!(
        fixture
            .service
            .heartbeat(&authenticated, heartbeat_for(&inventory, 2, Vec::new())?)
            .await,
        Err(InventoryError::SessionConflict)
    ));

    let new_incarnation = uuid::Uuid::new_v4().to_string();
    sqlx::query("UPDATE fleet_control_plane SET incarnation=? WHERE singleton=1")
        .bind(&new_incarnation)
        .execute(&fixture.pool)
        .await?;
    let replacement = handshake_for(
        AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
        text("recovered-incarnation")?,
        authenticated.epoch(),
    )?;
    // An obsolete control-plane lineage is recoverable; it is not a competing
    // live session for the new incarnation.
    let response = fixture
        .service
        .handshake(&authenticated, replacement.clone())
        .await?;
    assert_eq!(response.agent_session_id, replacement.agent_session_id);
    let mut replacement_report = report()?;
    replacement_report.control_plane_incarnation = new_incarnation.parse()?;
    replacement_report.authority_session_id = replacement.agent_session_id;
    replacement_report.boot_id = replacement.boot_id;
    replacement_report.inventory_sequence = BrowserCounter::new(1)?;
    fixture
        .service
        .submit_inventory(
            &authenticated,
            InventoryRequest::new(request_id()?, replacement_report)?,
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT epoch FROM fleet_hosts WHERE id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?,
        1
    );
    fixture.close().await
}

#[tokio::test]
async fn explicit_recovery_fences_obsolete_epoch_and_requires_fresh_inventory() -> Result<()> {
    let (fixture, authenticated, _, inventory) = started_fixture().await?;
    fixture
        .service
        .submit_inventory(&authenticated, inventory)
        .await?;
    let before_revision: u64 =
        sqlx::query_scalar::<_, i64>("SELECT revision FROM fleet_hosts WHERE id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?
            .try_into()?;
    let recovery = match fixture
        .service
        .recover_host(
            &Principal::AuthenticatedUser {
                user_id: "inventory-admin".to_owned(),
            },
            RecoverHost {
                host_id: authenticated.host_id(),
                expected_revision: Revision::new(before_revision)?,
                idempotency_key: "inventory-explicit-recovery".to_owned(),
            },
        )
        .await?
    {
        RecoveryIssue::Created(value) => value,
        RecoveryIssue::Replay(_) => anyhow::bail!("unexpected recovery replay"),
    };
    let receipt = fixture
        .service
        .consume_enrollment(
            recovery.code.expose(),
            HostRegistration::new(
                "recovered-inventory-host",
                ExecutionOs::Linux,
                Architecture::X64,
                None,
            )?,
        )
        .await?;
    assert!(
        fixture
            .service
            .authenticate(authenticated.host_id(), "invalid")
            .await
            .is_err()
    );
    let recovered = fixture
        .service
        .authenticate(receipt.host_id, receipt.credential.expose())
        .await?;
    assert_eq!(recovered.epoch().get(), 2);
    let new_session = handshake_for(
        AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
        text("recovery-boot")?,
        recovered.epoch(),
    )?;
    fixture
        .service
        .handshake(&recovered, new_session.clone())
        .await?;
    let mut fresh = report()?;
    fresh.host_epoch = recovered.epoch();
    fresh.control_plane_incarnation = fixture.incarnation().await?;
    fresh.authority_session_id = new_session.agent_session_id;
    fresh.boot_id = new_session.boot_id;
    fixture
        .service
        .submit_inventory(&recovered, InventoryRequest::new(request_id()?, fresh)?)
        .await?;
    let (epoch, complete): (i64, i64) =
        sqlx::query_as("SELECT epoch,inventory_complete FROM fleet_hosts WHERE id=?")
            .bind(recovered.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!((epoch, complete), (2, 1));
    fixture.close().await
}

#[tokio::test]
async fn original_receipts_remain_equal_after_interleaving_and_reopen() -> Result<()> {
    let (fixture, authenticated, handshake, inventory) = started_fixture().await?;
    let first_handshake = fixture
        .service
        .handshake(&authenticated, handshake.clone())
        .await?;
    let first_inventory = fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await?;
    fixture.clock.0.store(2_000, Ordering::SeqCst);
    let heartbeat = heartbeat_for(&inventory, 1, Vec::new())?;
    let first_heartbeat = fixture
        .service
        .heartbeat(&authenticated, heartbeat.clone())
        .await?;
    assert_eq!(
        fixture
            .service
            .submit_inventory(&authenticated, inventory.clone())
            .await?,
        first_inventory
    );
    assert_eq!(
        fixture
            .service
            .handshake(&authenticated, handshake.clone())
            .await?,
        first_handshake
    );

    fixture.clock.0.store(3_000, Ordering::SeqCst);
    let second_inventory = sequence(&inventory, 2)?;
    let second_response = fixture
        .service
        .submit_inventory(&authenticated, second_inventory.clone())
        .await?;
    assert_eq!(
        fixture
            .service
            .heartbeat(&authenticated, heartbeat.clone())
            .await?,
        first_heartbeat
    );

    let directory = fixture.directory.clone();
    let clock = fixture.clock.clone();
    fixture.pool.close().await;
    let reopened_pool = connect_database_path(&directory.join("inventory.sqlite")).await?;
    let reopened = RegistryService::with_clock(
        reopened_pool.clone(),
        Vault::test_with_secret(b"inventory-test-secret"),
        clock,
    );
    assert_eq!(
        reopened
            .submit_inventory(&authenticated, second_inventory)
            .await?,
        second_response
    );
    assert_eq!(
        reopened.heartbeat(&authenticated, heartbeat).await?,
        first_heartbeat
    );
    assert_eq!(
        reopened.handshake(&authenticated, handshake).await?,
        first_handshake
    );
    reopened_pool.close().await;
    std::fs::remove_dir_all(directory)?;
    Ok(())
}

#[tokio::test]
async fn heartbeat_receipt_state_is_stable_across_inventory_transition() -> Result<()> {
    let (fixture, authenticated, _, inventory) = started_fixture().await?;
    let first = fixture
        .service
        .heartbeat(&authenticated, heartbeat_for(&inventory, 1, Vec::new())?)
        .await?;
    assert_eq!(first.state, ObservationSessionState::Observing);
    fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await?;
    let second_request = heartbeat_for(&inventory, 2, Vec::new())?;
    let second = fixture
        .service
        .heartbeat(&authenticated, second_request.clone())
        .await?;
    assert_eq!(second.state, ObservationSessionState::Reconciling);
    assert_eq!(
        fixture
            .service
            .heartbeat(&authenticated, second_request)
            .await?,
        second
    );
    fixture.close().await
}

#[tokio::test]
async fn runtime_identity_and_unbound_known_id_claims_are_rejected_atomically() -> Result<()> {
    let (fixture, authenticated, _, mut inventory) = started_fixture().await?;
    inventory.report.domains[0].runtime_identity = Some(text("root-runtime-a")?);
    inventory.report.backends[0].runtime_identity = Some(text("backend-runtime-a")?);
    fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await?;

    let mut changed = sequence(&inventory, 2)?;
    changed.report.domains[0].runtime_identity = Some(text("root-runtime-b")?);
    changed.report.backends[0].runtime_identity = Some(text("backend-runtime-b")?);
    assert!(matches!(
        fixture
            .service
            .submit_inventory(&authenticated, changed)
            .await,
        Err(InventoryError::MappingConflict)
    ));
    let observed_identity: String =
        sqlx::query_scalar("SELECT runtime_identity FROM fleet_observed_backends WHERE host_id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!(observed_identity, "backend-runtime-a");

    let root_id: String = sqlx::query_scalar(
        "SELECT id FROM host_resource_domains WHERE host_id=? AND parent_domain_id IS NULL",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    let claimed = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO host_backends
         (id,host_id,domain_id,runtime_kind,execution_os,architecture,readiness,enabled,revision,capabilities_json,config_json,created_at,updated_at)
         VALUES (?,?,?,'native_process','linux','x64','unknown',1,7,'{}','{}',1000,1000)",
    )
    .bind(claimed.to_string())
    .bind(authenticated.host_id().to_string())
    .bind(root_id)
    .execute(&fixture.pool)
    .await?;
    let mut claim = sequence(&inventory, 2)?;
    claim.report.backends.push(ObservedBackend {
        local_key: text("claimed")?,
        known_id: Some(BackendId::try_new(claimed)?),
        domain_local_key: text("root")?,
        runtime_kind: RuntimeKind::NativeProcess,
        execution_os: ExecutionOs::Linux,
        architecture: Architecture::X64,
        capabilities: report()?.backends[0].capabilities.clone(),
        readiness: BackendReadiness::Ready,
        reason: None,
        runtime_identity: None,
    });
    assert!(matches!(
        fixture
            .service
            .submit_inventory(&authenticated, claim)
            .await,
        Err(InventoryError::MappingConflict)
    ));
    let protected: (i64, i64, String) =
        sqlx::query_as("SELECT enabled,revision,readiness FROM host_backends WHERE id=?")
            .bind(claimed.to_string())
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!(protected, (1, 7, "unknown".to_owned()));
    fixture.close().await
}

#[tokio::test]
async fn omitted_authorized_helper_becomes_unknown_and_reappears_without_policy_loss() -> Result<()>
{
    let (fixture, authenticated, _, mut inventory) = started_fixture().await?;
    inventory
        .report
        .interactive_sessions
        .push(ObservedInteractiveSession {
            local_key: text("desktop")?,
            known_id: None,
            os_user_id: text("operator")?,
            helper_state: HelperState::Ready,
        });
    fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await?;
    sqlx::query(
        "UPDATE host_sessions SET authorized=1,max_jobs=7 WHERE host_id=? AND session_key='desktop'",
    )
    .bind(authenticated.host_id().to_string())
    .execute(&fixture.pool)
    .await?;
    let mut omitted = sequence(&inventory, 2)?;
    omitted.report.interactive_sessions.clear();
    fixture
        .service
        .submit_inventory(&authenticated, omitted)
        .await?;
    let unknown: (String, i64, i64) = sqlx::query_as(
        "SELECT helper_state,authorized,max_jobs FROM host_sessions WHERE host_id=? AND session_key='desktop'",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(unknown, ("unknown".to_owned(), 1, 7));

    let mut reappeared = sequence(&inventory, 3)?;
    reappeared.report.interactive_sessions.clear();
    reappeared
        .report
        .interactive_sessions
        .push(ObservedInteractiveSession {
            local_key: text("desktop")?,
            known_id: None,
            os_user_id: text("operator")?,
            helper_state: HelperState::Ready,
        });
    fixture
        .service
        .submit_inventory(&authenticated, reappeared)
        .await?;
    let ready: (String, i64, i64) = sqlx::query_as(
        "SELECT helper_state,authorized,max_jobs FROM host_sessions WHERE host_id=? AND session_key='desktop'",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(ready, ("ready".to_owned(), 1, 7));
    fixture.close().await
}

#[tokio::test]
async fn unbound_known_helper_key_cannot_claim_an_authorized_server_session() -> Result<()> {
    let (fixture, authenticated, _, inventory) = started_fixture().await?;
    let session_id = SessionId::try_new(uuid::Uuid::new_v4())?;
    sqlx::query(
        "INSERT INTO host_sessions
         (id,host_id,session_key,os_user_id,helper_state,authorized,max_jobs,revision)
         VALUES (?,?,?,'operator','ready',1,7,4)",
    )
    .bind(session_id.to_string())
    .bind(authenticated.host_id().to_string())
    .bind("desktop")
    .execute(&fixture.pool)
    .await?;
    let mut claim = sequence(&inventory, 2)?;
    claim
        .report
        .interactive_sessions
        .push(ObservedInteractiveSession {
            local_key: text("desktop")?,
            known_id: None,
            os_user_id: text("operator")?,
            helper_state: HelperState::Ready,
        });
    assert!(matches!(
        fixture
            .service
            .submit_inventory(&authenticated, claim)
            .await,
        Err(InventoryError::MappingConflict)
    ));
    let policy: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT helper_state,authorized,max_jobs,revision FROM host_sessions WHERE id=?",
    )
    .bind(session_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(policy, ("ready".to_owned(), 1, 7, 4));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM fleet_observed_interactive_sessions WHERE host_id=?",
        )
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?,
        0
    );
    fixture.close().await
}

#[tokio::test]
async fn mixed_platform_runtime_reports_accept_native_tart_and_linux_docker() -> Result<()> {
    let fixture = Fixture::new().await?;
    for (key, name, host_os, backends) in [
        (
            "mac-platform",
            "mac-platform",
            ExecutionOs::Macos,
            vec![
                backend_variant(
                    "mac-native",
                    "root",
                    RuntimeKind::NativeProcess,
                    ExecutionOs::Macos,
                    Architecture::Arm64,
                )?,
                backend_variant(
                    "mac-tart",
                    "vm",
                    RuntimeKind::TartVm,
                    ExecutionOs::Macos,
                    Architecture::Arm64,
                )?,
                backend_variant(
                    "mac-docker",
                    "container",
                    RuntimeKind::Docker,
                    ExecutionOs::Linux,
                    Architecture::X64,
                )?,
            ],
        ),
        (
            "windows-platform",
            "windows-platform",
            ExecutionOs::Windows,
            vec![
                backend_variant(
                    "windows-native",
                    "root",
                    RuntimeKind::NativeProcess,
                    ExecutionOs::Windows,
                    Architecture::X64,
                )?,
                backend_variant(
                    "windows-docker",
                    "container",
                    RuntimeKind::Docker,
                    ExecutionOs::Linux,
                    Architecture::X64,
                )?,
            ],
        ),
    ] {
        let (authenticated, _) =
            enroll_host(&fixture, key, name, host_os, Architecture::X64).await?;
        let incarnation = fixture.incarnation().await?;
        let session = AuthoritySessionId::try_new(uuid::Uuid::new_v4())?;
        let boot = text(key)?;
        fixture
            .service
            .handshake(
                &authenticated,
                handshake_for(session, boot.clone(), authenticated.epoch())?,
            )
            .await?;
        let mut value = report()?;
        value.control_plane_incarnation = incarnation;
        value.authority_session_id = session;
        value.boot_id = boot;
        value
            .domains
            .push(child_domain("vm", "root", DomainKind::VirtualMachine)?);
        value
            .domains
            .push(child_domain("container", "vm", DomainKind::Container)?);
        value.backends = backends;
        let response = fixture
            .service
            .submit_inventory(&authenticated, InventoryRequest::new(request_id()?, value)?)
            .await?;
        assert_eq!(
            response.backend_ids.len(),
            if host_os == ExecutionOs::Macos { 3 } else { 2 }
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM host_backends WHERE host_id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?;
        assert_eq!(count, i64::try_from(response.backend_ids.len())?);
    }
    fixture.close().await
}

#[tokio::test]
async fn partial_parent_child_samples_keep_nulls_and_do_not_refresh_complete_inventory()
-> Result<()> {
    let (fixture, authenticated, _, mut inventory) = started_fixture().await?;
    inventory
        .report
        .domains
        .push(child_domain("child", "root", DomainKind::VirtualMachine)?);
    inventory.report.samples = vec![
        sample_for("root", SampleCoverage::Partial)?,
        sample_for("child", SampleCoverage::Unknown)?,
    ];
    fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await?;
    let original: (i64, i64, String) = sqlx::query_as(
        "SELECT inventory_revision,last_inventory_at,inventory_digest FROM fleet_hosts WHERE id=?",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    let domain_ids: Vec<(String, String)> = sqlx::query_as(
        "SELECT local_key,domain_id FROM fleet_observed_domains WHERE host_id=? ORDER BY local_key",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_all(&fixture.pool)
    .await?;
    assert_eq!(domain_ids.len(), 2);
    let samples: Vec<SampleRow> = sqlx::query_as(
        "SELECT domain_id,cpu_used_millis,memory_used_mib,disk_free_bytes,uptime_seconds FROM host_samples WHERE host_id=? ORDER BY id",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_all(&fixture.pool)
    .await?;
    assert_eq!(samples.len(), 2);
    assert_ne!(samples[0].0, samples[1].0);
    assert!(samples.iter().all(|sample| sample.1.is_none()
        && sample.2.is_none()
        && sample.3.is_none()
        && sample.4.is_none()));

    fixture.clock.0.store(2_000, Ordering::SeqCst);
    fixture
        .service
        .heartbeat(
            &authenticated,
            heartbeat_for(&inventory, 1, inventory.report.samples.clone())?,
        )
        .await?;
    let after: (i64, i64, String) = sqlx::query_as(
        "SELECT inventory_revision,last_inventory_at,inventory_digest FROM fleet_hosts WHERE id=?",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(after, original);
    fixture.close().await
}

#[tokio::test]
async fn count_depth_cycle_unknown_field_and_integer_limits_are_enforced() -> Result<()> {
    let (fixture, authenticated, _, inventory) = started_fixture().await?;
    let mut bounded = inventory.report.clone();
    bounded.domains.clear();
    bounded.domains.push(report()?.domains[0].clone());
    for index in 1..64 {
        let parent = if index == 1 {
            "root".to_owned()
        } else {
            format!("domain-{}", index - 1)
        };
        bounded.domains.push(child_domain(
            &format!("domain-{index}"),
            &parent,
            DomainKind::Container,
        )?);
    }
    fixture
        .service
        .submit_inventory(
            &authenticated,
            InventoryRequest::new(request_id()?, bounded.clone())?,
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fleet_observed_domains WHERE host_id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?,
        64
    );

    let mut too_many = bounded.clone();
    too_many.domains.push(child_domain(
        "domain-64",
        "domain-63",
        DomainKind::Container,
    )?);
    assert!(
        serde_json::from_value::<InventoryReport>({
            let mut value = serde_json::to_value(&too_many)?;
            value["unexpected"] = true.into();
            value
        })
        .is_err()
    );
    assert!(InventoryRequest::new(request_id()?, too_many).is_err());

    let mut cycle = sequence(&inventory, 2)?.report;
    cycle
        .domains
        .push(child_domain("a", "b", DomainKind::Container)?);
    cycle
        .domains
        .push(child_domain("b", "a", DomainKind::Container)?);
    assert!(matches!(
        fixture
            .service
            .submit_inventory(&authenticated, InventoryRequest::new(request_id()?, cycle)?)
            .await,
        Err(InventoryError::Payload(_))
    ));
    let mut oversized = serde_json::to_value(&inventory.report)?;
    oversized["domains"][0]["capacity"]["cpu_millis"] =
        serde_json::json!(9_007_199_254_740_992_u64);
    assert!(serde_json::from_value::<InventoryReport>(oversized).is_err());
    fixture.close().await
}

#[tokio::test]
async fn direct_sql_cannot_rewrite_observation_history_or_provenance() -> Result<()> {
    let (fixture, authenticated, _, inventory) = started_fixture().await?;
    fixture
        .service
        .submit_inventory(&authenticated, inventory)
        .await?;
    let host_id = authenticated.host_id().to_string();
    let session_id: String =
        sqlx::query_scalar("SELECT session_id FROM fleet_observation_sessions WHERE host_id=?")
            .bind(&host_id)
            .fetch_one(&fixture.pool)
            .await?;
    let receipt_columns: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name,\"notnull\" FROM pragma_table_info('fleet_inventory_snapshots')
          WHERE name IN ('receipt_lease_expires_at','receipt_state','inventory_revision',
                         'domain_mappings_json','backend_mappings_json','interactive_mappings_json')
          ORDER BY name",
    )
    .fetch_all(&fixture.pool)
    .await?;
    assert_eq!(receipt_columns.len(), 6);
    assert!(receipt_columns.iter().all(|(_, not_null)| *not_null == 1));
    assert!(
        sqlx::query("UPDATE fleet_observation_sessions SET boot_id='forged' WHERE session_id=?")
            .bind(&session_id)
            .execute(&fixture.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE fleet_inventory_snapshots SET host_epoch=99,snapshot_json='{}' WHERE host_id=?"
        )
        .bind(&host_id)
        .execute(&fixture.pool)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("UPDATE host_samples SET boot_id='forged' WHERE host_id=?")
            .bind(&host_id)
            .execute(&fixture.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query(
            "UPDATE host_samples
            SET authority_session_id=NULL,boot_id=NULL,control_plane_incarnation=NULL
          WHERE host_id=?",
        )
        .bind(&host_id)
        .execute(&fixture.pool)
        .await
        .is_err()
    );
    fixture.close().await
}

#[tokio::test]
async fn approval_after_reconnect_uses_global_inventory_revision_and_current_sequence() -> Result<()>
{
    let (fixture, authenticated, _handshake, inventory) = started_fixture().await?;
    fixture
        .service
        .submit_inventory(&authenticated, inventory.clone())
        .await?;
    fixture
        .service
        .submit_inventory(&authenticated, sequence(&inventory, 2)?)
        .await?;
    let old_times: (i64, i64) = sqlx::query_as(
        "SELECT last_seen_at,lease_expires_at FROM fleet_observation_sessions WHERE session_id=?",
    )
    .bind(inventory.report.authority_session_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    fixture.clock.0.store(91_001, Ordering::SeqCst);
    let replacement = handshake_for(
        AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
        text("approval-reconnect")?,
        authenticated.epoch(),
    )?;
    fixture
        .service
        .handshake(&authenticated, replacement.clone())
        .await?;
    let closed: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT last_seen_at,lease_expires_at,ended_at FROM fleet_observation_sessions WHERE session_id=?",
    )
    .bind(inventory.report.authority_session_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(closed, (old_times.0, old_times.1, Some(91_001)));
    let mut fresh_report = report()?;
    fresh_report.control_plane_incarnation = fixture.incarnation().await?;
    fresh_report.authority_session_id = replacement.agent_session_id;
    fresh_report.boot_id = replacement.boot_id;
    let fresh_response = fixture
        .service
        .submit_inventory(
            &authenticated,
            InventoryRequest::new(request_id()?, fresh_report)?,
        )
        .await?;
    assert_eq!(fresh_response.host_epoch, authenticated.epoch());
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM fleet_hosts WHERE id=?")
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?;
    let principal = Principal::AuthenticatedUser {
        user_id: "inventory-admin".to_owned(),
    };
    fixture
        .service
        .approve_host(
            &principal,
            ApproveHost {
                host_id: authenticated.host_id(),
                expected_revision: Revision::new(u64::try_from(revision)?)?,
                expected_inventory_revision: fresh_response.inventory_revision.get(),
                expected_inventory_digest: String::from(fresh_response.digest),
                scope: EnrollmentScope::new([])?,
            },
        )
        .await?;
    let state: String = sqlx::query_scalar("SELECT enrollment_state FROM fleet_hosts WHERE id=?")
        .bind(authenticated.host_id().to_string())
        .fetch_one(&fixture.pool)
        .await?;
    assert_eq!(state, "approved");
    fixture.close().await
}

#[tokio::test]
async fn live_work_authority_conflict_preserves_expired_observation_history() -> Result<()> {
    let (fixture, authenticated, handshake, _) = started_fixture().await?;
    sqlx::query("UPDATE fleet_hosts SET authority_expires_at=100000 WHERE id=?")
        .bind(authenticated.host_id().to_string())
        .execute(&fixture.pool)
        .await?;
    fixture.clock.0.store(91_001, Ordering::SeqCst);
    let competing = handshake_for(
        AuthoritySessionId::try_new(uuid::Uuid::new_v4())?,
        text("different-live-work-session")?,
        authenticated.epoch(),
    )?;
    assert!(matches!(
        fixture.service.handshake(&authenticated, competing).await,
        Err(InventoryError::SessionConflict)
    ));
    let history: (i64, i64, Option<i64>, String) = sqlx::query_as(
        "SELECT last_seen_at,lease_expires_at,ended_at,state FROM fleet_observation_sessions WHERE session_id=?")
        .bind(handshake.agent_session_id.to_string()).fetch_one(&fixture.pool).await?;
    assert_eq!(history, (1_000, 91_000, Some(91_001), "conflicted".into()));
    let host: (String, i64, Option<i64>) = sqlx::query_as(
        "SELECT integrity_state,epoch,authority_expires_at FROM fleet_hosts WHERE id=?",
    )
    .bind(authenticated.host_id().to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(host, ("quarantined".into(), 1, None));
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM fleet_observation_sessions WHERE host_id=?")
            .bind(authenticated.host_id().to_string())
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!(count, 1);
    fixture.close().await
}
