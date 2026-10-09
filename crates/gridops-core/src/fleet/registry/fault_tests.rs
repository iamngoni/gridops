//! Checks transaction rollback and immutable history on a real WAL database.
//! Snapshot assertions hash complete rows so failed tests cannot print verifiers.

use super::*;
use sha2::{Digest as _, Sha256};
use sqlx::{Column as _, Row as _, TypeInfo as _, ValueRef as _};
use std::{collections::BTreeMap, future::Future as _, task::Poll};

type RowSnapshot = BTreeMap<&'static str, (usize, String)>;

async fn snapshot(
    pool: &SqlitePool,
    tables: &[(&'static str, &'static str)],
) -> Result<RowSnapshot> {
    let mut snapshot = BTreeMap::new();
    for &(name, statement) in tables {
        let rows = sqlx::query(statement).fetch_all(pool).await?;
        let mut hash = Sha256::new();
        for row in &rows {
            let mut values = Vec::new();
            for column in row.columns() {
                let index = column.ordinal();
                let raw = row.try_get_raw(index)?;
                let value = if raw.is_null() {
                    serde_json::Value::Null
                } else {
                    match raw.type_info().name() {
                        "INTEGER" => json!(row.try_get::<i64, _>(index)?),
                        "REAL" => json!(row.try_get::<f64, _>(index)?),
                        "TEXT" => json!(row.try_get::<String, _>(index)?),
                        "BLOB" => json!(row.try_get::<Vec<u8>, _>(index)?),
                        _ => anyhow::bail!("unsupported test snapshot storage type"),
                    }
                };
                values.push((column.name(), value));
            }
            hash.update(serde_json::to_vec(&values)?);
            hash.update([0]);
        }
        snapshot.insert(name, (rows.len(), URL_SAFE_NO_PAD.encode(hash.finalize())));
    }
    Ok(snapshot)
}

const REGISTRY_ROWS: &[(&str, &str)] = &[
    ("hosts", "SELECT * FROM fleet_hosts ORDER BY id"),
    ("domains", "SELECT * FROM host_resource_domains ORDER BY id"),
    ("credentials", "SELECT * FROM host_credentials ORDER BY id"),
    ("enrollments", "SELECT * FROM host_enrollments ORDER BY id"),
    (
        "rotations",
        "SELECT * FROM host_credential_rotations ORDER BY id",
    ),
    (
        "grants",
        "SELECT * FROM host_target_grants ORDER BY host_id,target_id",
    ),
    ("audit", "SELECT * FROM audit_events ORDER BY id"),
    ("events", "SELECT * FROM fleet_events ORDER BY cursor"),
];

const HISTORY_ROWS: &[(&str, &str)] = &[
    (
        "placements",
        "SELECT * FROM workload_placements ORDER BY id",
    ),
    (
        "allocations",
        "SELECT * FROM capacity_allocations ORDER BY id",
    ),
    ("commands", "SELECT * FROM host_commands ORDER BY id"),
    ("samples", "SELECT * FROM host_samples ORDER BY id"),
    (
        "grants",
        "SELECT * FROM host_target_grants ORDER BY host_id,target_id",
    ),
];

#[tokio::test]
async fn recovery_consumption_rechecks_counters_changed_after_code_issuance() -> Result<()> {
    for replace_identity in [false, true] {
        let f = Fixture::new(1_000).await?;
        f.install_target().await?;
        let enrolled = f
            .consume(&f.issue_empty("stale-after-issue").await?)
            .await?;
        f.seed_history(enrolled.host_id).await?;
        let RecoveryIssue::Created(recovery) = f
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(1)?,
                    idempotency_key: "stale-after-issue-recovery".into(),
                },
            )
            .await?
        else {
            anyhow::bail!("expected a fresh recovery code");
        };
        // Model an independently committed policy/identity change AFTER the code
        // was issued; rejection must not consume it or revoke current credentials.
        if replace_identity {
            let RecoveryIssue::Created(newer) = f
                .service
                .recover_host(
                    &Fixture::principal(),
                    RecoverHost {
                        host_id: enrolled.host_id,
                        expected_revision: Revision::new(1)?,
                        idempotency_key: "competing-recovery".into(),
                    },
                )
                .await?
            else {
                anyhow::bail!("expected a second recovery code");
            };
            f.consume_recovery(&newer).await?;
        } else {
            sqlx::query("UPDATE fleet_hosts SET revision=revision+1 WHERE id=?")
                .bind(enrolled.host_id.to_string())
                .execute(&f.pool)
                .await?;
        }
        let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
        let history = snapshot(&f.pool, HISTORY_ROWS).await?;
        assert!(matches!(
            f.consume_recovery(&recovery).await,
            Err(RegistryError::EnrollmentScopeMismatch)
        ));
        assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
        assert_eq!(snapshot(&f.pool, HISTORY_ROWS).await?, history);
        let consumed: Option<i64> =
            sqlx::query_scalar("SELECT consumed_at FROM host_enrollments WHERE id=?")
                .bind(recovery.id.to_string())
                .fetch_one(&f.pool)
                .await?;
        assert_eq!(consumed, None);
        f.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn original_rotation_requester_demotion_blocks_metadata_replay_without_mutation() -> Result<()>
{
    let f = Fixture::new(1_000).await?;
    let enrolled = f
        .consume(&f.issue_empty("demoted-requester").await?)
        .await?;
    let request = RequestRotation {
        host_id: enrolled.host_id,
        expected_generation: Generation::new(1)?,
        idempotency_key: "original-requester".into(),
    };
    f.service
        .request_rotation(&Fixture::principal(), request.clone())
        .await?;
    sqlx::query("UPDATE users SET role='member' WHERE id=?")
        .bind(ADMIN)
        .execute(&f.pool)
        .await?;
    let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
    assert!(matches!(
        f.service
            .request_rotation(&Fixture::principal(), request)
            .await,
        Err(RegistryError::Forbidden)
    ));
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
    f.close().await
}

#[tokio::test]
async fn failed_credential_insert_rolls_back_created_host_root_and_consumption() -> Result<()> {
    let f = Fixture::new(1_000).await?;
    let issue = f.issue_empty("credential-insert-fault").await?;
    let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
    sqlx::query("CREATE TRIGGER fail_credential_insert BEFORE INSERT ON host_credentials BEGIN SELECT RAISE(ABORT,'synthetic insert fault'); END")
        .execute(&f.pool).await?;
    assert!(matches!(
        f.consume(&issue).await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
    sqlx::query("DROP TRIGGER fail_credential_insert")
        .execute(&f.pool)
        .await?;
    f.consume(&issue).await?;
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM fleet_hosts),(SELECT COUNT(*) FROM host_resource_domains),(SELECT COUNT(*) FROM host_credentials)")
        .fetch_one(&f.pool).await?;
    assert_eq!(counts, (1, 1, 1));
    assert_eq!(f.audit_event_count().await?, (2, 2));
    assert!(matches!(
        f.consume(&issue).await,
        Err(RegistryError::EnrollmentGone)
    ));
    f.close().await
}

#[tokio::test]
async fn enrollment_revocation_and_rotation_request_are_atomic_with_audit() -> Result<()> {
    let f = Fixture::new(1_000).await?;
    let issue = f.issue_empty("revoke-fault").await?;
    let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
    sqlx::query("CREATE TRIGGER fail_revoke_event BEFORE INSERT ON fleet_events WHEN NEW.kind='fleet.enrollment.revoked' BEGIN SELECT RAISE(ABORT,'synthetic event fault'); END")
        .execute(&f.pool).await?;
    assert!(matches!(
        f.service
            .revoke_enrollment(&Fixture::principal(), issue.id)
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
    sqlx::query("DROP TRIGGER fail_revoke_event")
        .execute(&f.pool)
        .await?;
    f.service
        .revoke_enrollment(&Fixture::principal(), issue.id)
        .await?;
    let after = snapshot(&f.pool, REGISTRY_ROWS).await?;
    f.service
        .revoke_enrollment(&Fixture::principal(), issue.id)
        .await?;
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, after);
    let enrolled = f
        .consume(&f.issue_empty("rotation-request-fault").await?)
        .await?;
    let request = || -> Result<RequestRotation> {
        Ok(RequestRotation {
            host_id: enrolled.host_id,
            expected_generation: Generation::new(1)?,
            idempotency_key: "rotation-request-fault".into(),
        })
    };
    let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
    sqlx::query("CREATE TRIGGER fail_rotation_request_event BEFORE INSERT ON fleet_events WHEN NEW.kind='fleet.credential.rotation_requested' BEGIN SELECT RAISE(ABORT,'synthetic request fault'); END")
        .execute(&f.pool).await?;
    assert!(matches!(
        f.service
            .request_rotation(&Fixture::principal(), request()?)
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
    sqlx::query("DROP TRIGGER fail_rotation_request_event")
        .execute(&f.pool)
        .await?;
    let created = f
        .service
        .request_rotation(&Fixture::principal(), request()?)
        .await?;
    assert_eq!(created.next_generation.get(), 2);
    let after = snapshot(&f.pool, REGISTRY_ROWS).await?;
    assert_eq!(
        f.service
            .request_rotation(&Fixture::principal(), request()?)
            .await?
            .operation_id,
        created.operation_id
    );
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, after);
    f.close().await
}

#[tokio::test]
async fn approval_event_failure_rolls_back_grants_and_host_policy() -> Result<()> {
    let f = Fixture::new(1_000).await?;
    f.install_target().await?;
    let EnrollmentIssue::Created(issue) = f
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "approval-fault".into(),
                scope: EnrollmentScope::new([TARGET.parse()?])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    else {
        anyhow::bail!("expected enrollment");
    };
    let enrolled = f.consume(&issue).await?;
    f.set_current_inventory(enrolled.host_id, 1, &inventory_digest(), true)
        .await?;
    f.seed_control_plane().await?;
    let approval = || -> Result<ApproveHost> {
        Ok(ApproveHost {
            host_id: enrolled.host_id,
            expected_revision: Revision::new(1)?,
            expected_inventory_revision: 1,
            expected_inventory_digest: inventory_digest(),
            scope: EnrollmentScope::new([TARGET.parse()?])?,
        })
    };
    let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
    sqlx::query("CREATE TRIGGER fail_approval_event BEFORE INSERT ON fleet_events WHEN NEW.kind='fleet.host.approved' BEGIN SELECT RAISE(ABORT,'synthetic approval fault'); END")
        .execute(&f.pool).await?;
    assert!(matches!(
        f.service
            .approve_host(&Fixture::principal(), approval()?)
            .await,
        Err(RegistryError::Database(_))
    ));
    assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
    sqlx::query("DROP TRIGGER fail_approval_event")
        .execute(&f.pool)
        .await?;
    let event_count = f.audit_event_count().await?;
    f.service
        .approve_host(&Fixture::principal(), approval()?)
        .await?;
    assert_eq!(
        f.audit_event_count().await?,
        (event_count.0 + 1, event_count.1 + 1)
    );
    f.close().await
}

#[tokio::test]
async fn recovery_host_mutation_failure_preserves_entire_history_and_open_rotation() -> Result<()> {
    for approved in [false, true] {
        let f = Fixture::new(1_000).await?;
        f.install_target().await?;
        let EnrollmentIssue::Created(issue) = f
            .service
            .issue_enrollment(
                &Fixture::principal(),
                IssueEnrollment {
                    idempotency_key: "history-fault".into(),
                    scope: EnrollmentScope::new([TARGET.parse()?])?,
                    intended_host_id: None,
                    ttl_millis: None,
                },
            )
            .await?
        else {
            anyhow::bail!("expected enrollment");
        };
        let enrolled = f.consume(&issue).await?;
        if approved {
            f.set_current_inventory(enrolled.host_id, 1, &inventory_digest(), true)
                .await?;
            f.seed_control_plane().await?;
            f.service
                .approve_host(
                    &Fixture::principal(),
                    ApproveHost {
                        host_id: enrolled.host_id,
                        expected_revision: Revision::new(1)?,
                        expected_inventory_revision: 1,
                        expected_inventory_digest: inventory_digest(),
                        scope: EnrollmentScope::new([TARGET.parse()?])?,
                    },
                )
                .await?;
        }
        f.seed_history(enrolled.host_id).await?;
        let rotation = f
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)?,
                    idempotency_key: "history-open-rotation".into(),
                },
            )
            .await?;
        let history = snapshot(&f.pool, HISTORY_ROWS).await?;
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM fleet_hosts WHERE id=?")
            .bind(enrolled.host_id.to_string())
            .fetch_one(&f.pool)
            .await?;
        let RecoveryIssue::Created(recovery) = f
            .service
            .recover_host(
                &Fixture::principal(),
                RecoverHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(u64::try_from(revision)?)?,
                    idempotency_key: "history-recover".into(),
                },
            )
            .await?
        else {
            anyhow::bail!("expected recovery");
        };
        assert_eq!(snapshot(&f.pool, HISTORY_ROWS).await?, history);
        let before = snapshot(&f.pool, REGISTRY_ROWS).await?;
        sqlx::query("CREATE TRIGGER fail_recovered_host BEFORE UPDATE OF epoch ON fleet_hosts WHEN NEW.epoch != OLD.epoch BEGIN SELECT RAISE(ABORT,'synthetic recovered host fault'); END")
            .execute(&f.pool).await?;
        assert!(matches!(
            f.consume_recovery(&recovery).await,
            Err(RegistryError::Database(_))
        ));
        assert_eq!(snapshot(&f.pool, REGISTRY_ROWS).await?, before);
        assert_eq!(snapshot(&f.pool, HISTORY_ROWS).await?, history);
        sqlx::query("DROP TRIGGER fail_recovered_host")
            .execute(&f.pool)
            .await?;
        let recovered = f.consume_recovery(&recovery).await?;
        assert_eq!(recovered.host_id, enrolled.host_id);
        assert_eq!(snapshot(&f.pool, HISTORY_ROWS).await?, history);
        assert!(
            f.service
                .authenticate(enrolled.host_id, enrolled.credential.expose())
                .await
                .is_err()
        );
        let state = f
            .service
            .rotation_status(&Fixture::principal(), rotation.operation_id)
            .await?;
        assert_eq!(state.state, RotationOperationState::Revoked);
        let charged: (i64, Option<i64>) = sqlx::query_as("SELECT cpu_millis,released_at FROM capacity_allocations WHERE id='registry-allocation'")
            .fetch_one(&f.pool).await?;
        assert_eq!(charged, (100, None));
        f.close().await?;
    }
    Ok(())
}

async fn waiter_pool(f: &Fixture) -> Result<SqlitePool> {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(f.directory.join("registry.sqlite"))
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

#[tokio::test]
async fn wal_consumers_create_one_complete_enrollment_and_one_typed_loser() -> Result<()> {
    let f = Fixture::new(1_000).await?;
    let issue = f.issue_empty("complete-race").await?;
    let pool = waiter_pool(&f).await?;
    let other = RegistryService::with_clock(
        pool.clone(),
        Vault::test_with_secret(b"registry-test-secret"),
        f.clock.clone(),
    );
    let (first, second) = tokio::join!(
        f.service.consume_enrollment(
            issue.code.expose(),
            HostRegistration::new("first", ExecutionOs::Windows, Architecture::X64, None)?
        ),
        other.consume_enrollment(
            issue.code.expose(),
            HostRegistration::new("second", ExecutionOs::Macos, Architecture::Arm64, None)?
        )
    );
    let ((Ok(receipt), Err(RegistryError::EnrollmentGone))
    | (Err(RegistryError::EnrollmentGone), Ok(receipt))) = (first, second)
    else {
        anyhow::bail!("one consumer must succeed and the other must observe consumed enrollment");
    };
    let counts: (i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM fleet_hosts),(SELECT COUNT(*) FROM host_resource_domains),(SELECT COUNT(*) FROM host_credentials),(SELECT COUNT(*) FROM audit_events WHERE action='fleet.enrollment.consumed'),(SELECT COUNT(*) FROM fleet_events WHERE kind='fleet.enrollment.consumed')")
        .fetch_one(&f.pool).await?;
    assert_eq!(counts, (1, 1, 1, 1, 1));
    let owners: (String,String,String) = sqlx::query_as("SELECT h.id,d.host_id,c.host_id FROM fleet_hosts h JOIN host_resource_domains d ON d.host_id=h.id JOIN host_credentials c ON c.host_id=h.id")
        .fetch_one(&f.pool).await?;
    assert_eq!(
        owners,
        (
            receipt.host_id.to_string(),
            receipt.host_id.to_string(),
            receipt.host_id.to_string()
        )
    );
    pool.close().await;
    f.close().await
}

#[tokio::test]
async fn matching_hardware_fingerprints_do_not_inherit_identity_or_trust() -> Result<()> {
    let f = Fixture::new(1_000).await?;
    let first_code = f.issue_empty("duplicate-hardware-one").await?;
    let second_code = f.issue_empty("duplicate-hardware-two").await?;
    let first = f
        .consume_registration(
            &first_code,
            HostRegistration::new(
                "first",
                ExecutionOs::Linux,
                Architecture::X64,
                Some("shared-synthetic-hardware".into()),
            )?,
        )
        .await?;
    let second = f
        .consume_registration(
            &second_code,
            HostRegistration::new(
                "second",
                ExecutionOs::Linux,
                Architecture::X64,
                Some("shared-synthetic-hardware".into()),
            )?,
        )
        .await?;
    assert_ne!(first.host_id, second.host_id);
    assert_ne!(first.credential_id, second.credential_id);
    assert!(matches!(
        f.service
            .authenticate(first.host_id, second.credential.expose())
            .await,
        Err(RegistryError::InvalidCredential)
    ));
    let hosts: Vec<(String,String,String,Option<i64>)> = sqlx::query_as("SELECT enrollment_state,lifecycle_state,integrity_state,authority_expires_at FROM fleet_hosts ORDER BY id")
        .fetch_all(&f.pool).await?;
    assert_eq!(
        hosts,
        vec![("pending".into(), "paused".into(), "unverified".into(), None); 2]
    );
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM host_resource_domains WHERE parent_domain_id IS NULL),(SELECT COUNT(*) FROM host_target_grants),(SELECT SUM(cpu_millis+memory_mib+disk_bytes) FROM host_resource_domains)")
        .fetch_one(&f.pool).await?;
    assert_eq!(counts, (2, 0, 0));
    f.close().await
}

#[tokio::test]
async fn pending_writer_reads_enrollment_and_credential_expiry_only_after_lock() -> Result<()> {
    let f = Fixture::new(1_000).await?;
    let issue = f.issue_empty("actual-lock-expiry").await?;
    let pool = waiter_pool(&f).await?;
    let service = RegistryService::with_clock(
        pool.clone(),
        Vault::test_with_secret(b"registry-test-secret"),
        f.clock.clone(),
    );
    let lock = f.pool.begin_with("BEGIN IMMEDIATE").await?;
    let clock_reads = f.clock.read_count();
    let mut consume = Box::pin(service.consume_enrollment(
        issue.code.expose(),
        HostRegistration::new("blocked-host", ExecutionOs::Linux, Architecture::X64, None)?,
    ));
    // The single connection is already idle and skips health-check I/O. Polling
    // takes it and dispatches BEGIN IMMEDIATE while an independent writer holds
    // the lock; no elapsed sleep or spawned-task flag supplies synchronization.
    let first_poll = std::future::poll_fn(|cx| Poll::Ready(consume.as_mut().poll(cx))).await;
    assert!(first_poll.is_pending());
    assert_eq!(pool.num_idle(), 0);
    assert_eq!(f.clock.read_count(), clock_reads);
    f.clock.set(issue.expires_at);
    lock.commit().await?;
    assert!(matches!(consume.await, Err(RegistryError::EnrollmentGone)));

    let enrolled = f
        .consume(&f.issue_empty("actual-credential-expiry").await?)
        .await?;
    let authenticated = f
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let rotation = f
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "actual-credential-expiry".into(),
            },
        )
        .await?;
    let expires = f.clock.now_millis() + 1_000;
    sqlx::query("UPDATE host_credentials SET overlap_expires_at=? WHERE id=?")
        .bind(expires)
        .bind(enrolled.credential_id.to_string())
        .execute(&f.pool)
        .await?;
    tokio::time::timeout(Duration::from_secs(1), async {
        while pool.num_idle() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let lock = f.pool.begin_with("BEGIN IMMEDIATE").await?;
    let clock_reads = f.clock.read_count();
    let next_secret = URL_SAFE_NO_PAD.encode([73_u8; 32]);
    let mut exchange =
        Box::pin(service.exchange_rotation(&authenticated, rotation.operation_id, &next_secret));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(exchange.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert_eq!(pool.num_idle(), 0);
    assert_eq!(f.clock.read_count(), clock_reads);
    f.clock.set(expires);
    lock.commit().await?;
    assert!(matches!(
        exchange.await,
        Err(RegistryError::InvalidCredential)
    ));
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM host_credential_rotations WHERE id=?")
            .bind(rotation.operation_id.to_string())
            .fetch_one(&f.pool)
            .await?;
    assert_eq!(phase, "requested");
    pool.close().await;
    f.close().await
}
