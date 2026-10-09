//! Focused acceptance proofs for registry lineage, policy, observation, and replay invariants.
//!
//! These tests deliberately use the private file-backed fixture from `tests.rs` so every
//! transition exercises the real SQLite/WAL service transaction and deterministic clock.

use anyhow::Result;
use sqlx::Row as _;

use super::*;

const ADMIN_2: &str = "registry-admin-2";
const TARGET_2: &str = "30000000-0000-4000-8000-000000000002";
type GrantSnapshot = (String, i64, i64, i64, i64, i64, Option<i64>);

async fn add_second_admin(fixture: &Fixture) -> Result<Principal> {
    let now = fixture.clock.now_millis();
    sqlx::query(
        "INSERT INTO users
         (id,github_id,login,role,access_token,last_login_at,created_at,updated_at)
         VALUES (?,-99,?,'admin','',?,?,?)",
    )
    .bind(ADMIN_2)
    .bind(ADMIN_2)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&fixture.pool)
    .await?;
    Ok(Principal::AuthenticatedUser {
        user_id: ADMIN_2.to_owned(),
    })
}

async fn set_inventory_for_current_epoch(
    fixture: &Fixture,
    host_id: HostId,
    revision: i64,
    digest: &str,
) -> Result<()> {
    let now = fixture.clock.now_millis();
    let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM fleet_hosts WHERE id=?")
        .bind(host_id.to_string())
        .fetch_one(&fixture.pool)
        .await?;
    let (credential_id, credential_generation): (String, i64) = sqlx::query_as(
        "SELECT id,generation FROM host_credentials
           WHERE host_id=? AND revoked_at IS NULL ORDER BY generation DESC LIMIT 1",
    )
    .bind(host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    sqlx::query(
        "UPDATE fleet_hosts
            SET agent_session_id='session-1',boot_id='boot-1',authority_incarnation='inc-1'
         WHERE id=?",
    )
    .bind(host_id.to_string())
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "UPDATE fleet_hosts
            SET inventory_complete=1,inventory_revision=?,inventory_digest=?,
                inventory_epoch=epoch,inventory_session_id=agent_session_id,
                inventory_boot_id=boot_id,last_inventory_at=?
          WHERE id=?",
    )
    .bind(revision)
    .bind(digest)
    .bind(now)
    .bind(host_id.to_string())
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "DELETE FROM fleet_inventory_snapshots WHERE host_id=? AND authority_session_id='session-1'",
    )
    .bind(host_id.to_string())
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "DELETE FROM fleet_observation_sessions WHERE host_id=? AND session_id='session-1'",
    )
    .bind(host_id.to_string())
    .execute(&fixture.pool)
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
           FROM fleet_hosts WHERE id=?",
    )
    .bind(&credential_id)
    .bind(credential_generation)
    .bind(&credential_id)
    .bind(credential_generation)
    .bind(digest)
    .bind(now)
    .bind(now)
    .bind(now + 60_000)
    .bind(now)
    .bind(now + 60_000)
    .bind(1_i64)
    .bind(digest)
    .bind(now)
    .bind(host_id.to_string())
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO fleet_inventory_snapshots
         (host_id,authority_session_id,host_epoch,control_plane_incarnation,inventory_sequence,
          digest,snapshot_json,received_at,receipt_lease_expires_at,receipt_state,
          inventory_revision,domain_mappings_json,backend_mappings_json,interactive_mappings_json)
         VALUES (?, 'session-1', ?, 'inc-1', 1, ?, '{}', ?, ?, 'reconciling', ?, '[]', '[]', '[]')",
    )
    .bind(host_id.to_string())
    .bind(epoch)
    .bind(digest)
    .bind(now)
    .bind(now + 60_000)
    .bind(revision)
    .execute(&fixture.pool)
    .await?;
    Ok(())
}

async fn prepare_approval() -> Result<(Fixture, HostId, ApproveHost)> {
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let issue = fixture.issue_empty("invariant-approval").await?;
    let enrolled = fixture.consume(&issue).await?;
    let digest = inventory_digest();
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, true)
        .await?;
    fixture.seed_control_plane().await?;
    Ok((
        fixture,
        enrolled.host_id,
        ApproveHost {
            host_id: enrolled.host_id,
            expected_revision: Revision::new(1)?,
            expected_inventory_revision: 7,
            expected_inventory_digest: digest,
            scope: EnrollmentScope::new([])?,
        },
    ))
}

async fn assert_approval_rejection(
    fixture: &Fixture,
    request: ApproveHost,
    expected: impl FnOnce(&RegistryError) -> bool,
) -> Result<()> {
    let before: (i64, String, String, i64, i64) = sqlx::query_as(
        "SELECT revision,enrollment_state,lifecycle_state,
                (SELECT COUNT(*) FROM host_target_grants WHERE host_id=?),
                (SELECT COUNT(*) FROM host_target_grants WHERE host_id=? AND revoked_at IS NOT NULL)
           FROM fleet_hosts WHERE id=?",
    )
    .bind(request.host_id.to_string())
    .bind(request.host_id.to_string())
    .bind(request.host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    let events_before = fixture.audit_event_count().await?;
    let error = match fixture
        .service
        .approve_host(&Fixture::principal(), request.clone())
        .await
    {
        Ok(()) => anyhow::bail!("invalid approval evidence was accepted"),
        Err(error) => error,
    };
    assert!(expected(&error), "unexpected approval error: {error:?}");
    let after: (i64, String, String, i64, i64) = sqlx::query_as(
        "SELECT revision,enrollment_state,lifecycle_state,
                (SELECT COUNT(*) FROM host_target_grants WHERE host_id=?),
                (SELECT COUNT(*) FROM host_target_grants WHERE host_id=? AND revoked_at IS NOT NULL)
           FROM fleet_hosts WHERE id=?",
    )
    .bind(request.host_id.to_string())
    .bind(request.host_id.to_string())
    .bind(request.host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(before, after);
    assert_eq!(events_before, fixture.audit_event_count().await?);
    Ok(())
}

#[tokio::test]
async fn causal_lineage_keeps_empty_scope_across_same_clock_recoveries() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let admin_2 = add_second_admin(&fixture).await?;
    let target = TARGET.parse::<CiTargetId>()?;
    let issue = match fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "lineage-normal".to_owned(),
                scope: EnrollmentScope::new([target])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?
    {
        EnrollmentIssue::Created(issue) => issue,
        EnrollmentIssue::Replay(_) => anyhow::bail!("normal enrollment unexpectedly replayed"),
    };
    let enrolled = fixture.consume(&issue).await?;
    let digest = inventory_digest();
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, true)
        .await?;
    fixture.seed_control_plane().await?;
    fixture
        .service
        .approve_host(
            &Fixture::principal(),
            ApproveHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                expected_inventory_revision: 7,
                expected_inventory_digest: digest.clone(),
                scope: EnrollmentScope::new([])?,
            },
        )
        .await?;
    let initial_lineage: (String, i64) = sqlx::query_as(
        "SELECT current_enrollment_id,current_enrollment_epoch FROM fleet_hosts WHERE id=?",
    )
    .bind(enrolled.host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(initial_lineage, (issue.id.to_string(), 1));

    let recovery_1 = match fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(2)?,
                idempotency_key: "lineage-recovery-1".to_owned(),
            },
        )
        .await?
    {
        RecoveryIssue::Created(issue) => issue,
        RecoveryIssue::Replay(_) => anyhow::bail!("first recovery unexpectedly replayed"),
    };
    let first_recovered = fixture.consume_recovery(&recovery_1).await?;
    assert_eq!(first_recovered.host_id, enrolled.host_id);
    let first_lineage: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT current_enrollment_id,current_enrollment_epoch,epoch,revision
           FROM fleet_hosts WHERE id=?",
    )
    .bind(enrolled.host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(
        first_lineage,
        (recovery_1.id.to_string(), 2, 2, 3),
        "same-clock recovery must advance causal lineage exactly once",
    );

    let recovery_2 = match fixture
        .service
        .recover_host(
            &admin_2,
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(3)?,
                idempotency_key: "lineage-recovery-2".to_owned(),
            },
        )
        .await?
    {
        RecoveryIssue::Created(issue) => issue,
        RecoveryIssue::Replay(_) => anyhow::bail!("second recovery unexpectedly replayed"),
    };
    let second_recovered = fixture.consume_recovery(&recovery_2).await?;
    assert_eq!(second_recovered.host_id, enrolled.host_id);
    let second_scope: String =
        sqlx::query_scalar("SELECT scope_json FROM host_enrollments WHERE id=?")
            .bind(recovery_2.id.to_string())
            .fetch_one(&fixture.pool)
            .await?;
    let issuer: String = sqlx::query_scalar("SELECT issued_by FROM host_enrollments WHERE id=?")
        .bind(recovery_2.id.to_string())
        .fetch_one(&fixture.pool)
        .await?;
    assert_eq!(second_scope, r#"{"target_ids":[]}"#);
    assert_eq!(issuer, ADMIN_2);
    let current_lineage: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT current_enrollment_id,current_enrollment_epoch,epoch,revision
           FROM fleet_hosts WHERE id=?",
    )
    .bind(enrolled.host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(current_lineage, (recovery_2.id.to_string(), 3, 3, 4));

    set_inventory_for_current_epoch(&fixture, enrolled.host_id, 9, &digest).await?;
    let expanded = fixture
        .service
        .approve_host(
            &Fixture::principal(),
            ApproveHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(4)?,
                expected_inventory_revision: 9,
                expected_inventory_digest: digest.clone(),
                scope: EnrollmentScope::new([target])?,
            },
        )
        .await;
    assert!(matches!(
        expanded,
        Err(RegistryError::EnrollmentScopeMismatch)
    ));
    fixture
        .service
        .approve_host(
            &Fixture::principal(),
            ApproveHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(4)?,
                expected_inventory_revision: 9,
                expected_inventory_digest: digest,
                scope: EnrollmentScope::new([])?,
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
async fn approval_preserves_policy_grants_and_history_across_recovery() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    sqlx::query(
        "INSERT INTO fleet_ci_targets (id,kind,canonical_key,installation_id,repository_id)
         VALUES (?,'github_repository','registry:1:10:second',1,10)",
    )
    .bind(TARGET_2)
    .execute(&fixture.pool)
    .await?;
    let first = fixture
        .service
        .issue_enrollment(
            &Fixture::principal(),
            IssueEnrollment {
                idempotency_key: "grant-preservation".to_owned(),
                scope: EnrollmentScope::new([TARGET.parse()?, TARGET_2.parse()?])?,
                intended_host_id: None,
                ttl_millis: None,
            },
        )
        .await?;
    let issue = match first {
        EnrollmentIssue::Created(issue) => issue,
        EnrollmentIssue::Replay(_) => anyhow::bail!("grant enrollment unexpectedly replayed"),
    };
    let enrolled = fixture.consume(&issue).await?;
    let digest = inventory_digest();
    fixture
        .set_current_inventory(enrolled.host_id, 7, &digest, true)
        .await?;
    fixture.seed_control_plane().await?;
    let both = EnrollmentScope::new([TARGET.parse()?, TARGET_2.parse()?])?;
    fixture
        .service
        .approve_host(
            &Fixture::principal(),
            ApproveHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(1)?,
                expected_inventory_revision: 7,
                expected_inventory_digest: digest.clone(),
                scope: both,
            },
        )
        .await?;
    sqlx::query(
        "UPDATE host_target_grants
            SET allow_schedule=1,allow_native=1,allow_interactive=1,
                allow_docker_socket=1,revision=7
          WHERE host_id=? AND target_id=?",
    )
    .bind(enrolled.host_id.to_string())
    .bind(TARGET)
    .execute(&fixture.pool)
    .await?;
    sqlx::query(
        "UPDATE host_target_grants
            SET allow_native=1,revision=4
          WHERE host_id=? AND target_id=?",
    )
    .bind(enrolled.host_id.to_string())
    .bind(TARGET_2)
    .execute(&fixture.pool)
    .await?;
    let grants_before: Vec<GrantSnapshot> = sqlx::query(
        "SELECT target_id,allow_schedule,allow_native,allow_interactive,
                allow_docker_socket,revision,revoked_at
           FROM host_target_grants WHERE host_id=? ORDER BY target_id",
    )
    .bind(enrolled.host_id.to_string())
    .fetch_all(&fixture.pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            row.try_get("target_id")?,
            row.try_get("allow_schedule")?,
            row.try_get("allow_native")?,
            row.try_get("allow_interactive")?,
            row.try_get("allow_docker_socket")?,
            row.try_get("revision")?,
            row.try_get("revoked_at")?,
        ))
    })
    .collect::<Result<_>>()?;
    let host_before: (i64, String, String) = sqlx::query_as(
        "SELECT revision,enrollment_state,lifecycle_state FROM fleet_hosts WHERE id=?",
    )
    .bind(enrolled.host_id.to_string())
    .fetch_one(&fixture.pool)
    .await?;
    let audit_before = fixture.audit_event_count().await?;
    assert!(matches!(
        fixture
            .service
            .approve_host(
                &Fixture::principal(),
                ApproveHost {
                    host_id: enrolled.host_id,
                    expected_revision: Revision::new(2)?,
                    expected_inventory_revision: 7,
                    expected_inventory_digest: digest.clone(),
                    scope: EnrollmentScope::new([TARGET.parse()?])?,
                },
            )
            .await,
        Err(RegistryError::StaleRevision)
    ));
    let grants_after_repeat: Vec<GrantSnapshot> = sqlx::query(
        "SELECT target_id,allow_schedule,allow_native,allow_interactive,
                allow_docker_socket,revision,revoked_at
           FROM host_target_grants WHERE host_id=? ORDER BY target_id",
    )
    .bind(enrolled.host_id.to_string())
    .fetch_all(&fixture.pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            row.try_get("target_id")?,
            row.try_get("allow_schedule")?,
            row.try_get("allow_native")?,
            row.try_get("allow_interactive")?,
            row.try_get("allow_docker_socket")?,
            row.try_get("revision")?,
            row.try_get("revoked_at")?,
        ))
    })
    .collect::<Result<_>>()?;
    assert_eq!(grants_before, grants_after_repeat);
    assert_eq!(
        host_before,
        sqlx::query_as(
            "SELECT revision,enrollment_state,lifecycle_state FROM fleet_hosts WHERE id=?",
        )
        .bind(enrolled.host_id.to_string())
        .fetch_one(&fixture.pool)
        .await?
    );
    assert_eq!(audit_before, fixture.audit_event_count().await?);

    let recovery = match fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(2)?,
                idempotency_key: "grant-preservation-recovery".to_owned(),
            },
        )
        .await?
    {
        RecoveryIssue::Created(recovery) => recovery,
        RecoveryIssue::Replay(_) => anyhow::bail!("recovery unexpectedly replayed"),
    };
    fixture.consume_recovery(&recovery).await?;
    set_inventory_for_current_epoch(&fixture, enrolled.host_id, 8, &digest).await?;
    fixture
        .service
        .approve_host(
            &Fixture::principal(),
            ApproveHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(3)?,
                expected_inventory_revision: 8,
                expected_inventory_digest: digest,
                scope: EnrollmentScope::new([TARGET.parse()?])?,
            },
        )
        .await?;
    let retained: (i64, i64, i64, i64, i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT allow_schedule,allow_native,allow_interactive,allow_docker_socket,
                revision,revoked_at IS NULL,revoked_at
           FROM host_target_grants WHERE host_id=? AND target_id=?",
    )
    .bind(enrolled.host_id.to_string())
    .bind(TARGET)
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(retained, (1, 1, 1, 1, 7, 1, None));
    let removed: (i64, i64, i64, i64, i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT allow_schedule,allow_native,allow_interactive,allow_docker_socket,
                revision,revoked_at IS NULL,revoked_at
           FROM host_target_grants WHERE host_id=? AND target_id=?",
    )
    .bind(enrolled.host_id.to_string())
    .bind(TARGET_2)
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(removed.0, 0);
    assert_eq!(removed.1, 1, "revoked history keeps its prior policy bytes");
    assert_eq!(removed.2, 0);
    assert_eq!(removed.3, 0);
    assert_eq!(removed.4, 4);
    assert_eq!(removed.5, 0);
    assert!(
        removed.6.is_some(),
        "removed association must retain revoke history"
    );
    fixture.close().await
}

#[tokio::test]
async fn approval_requires_each_current_observation_predicate_independently() -> Result<()> {
    {
        let (fixture, host_id, request) = prepare_approval().await?;
        assert_eq!(
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT authority_expires_at FROM fleet_hosts WHERE id=?",
            )
            .bind(host_id.to_string())
            .fetch_one(&fixture.pool)
            .await?,
            None,
            "approval proof must not require work authority",
        );
        fixture
            .service
            .approve_host(&Fixture::principal(), request)
            .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "DELETE FROM fleet_inventory_snapshots
             WHERE host_id=? AND authority_session_id='session-1'",
        )
        .bind(request.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        sqlx::query("DELETE FROM fleet_observation_sessions WHERE session_id='session-1'")
            .execute(&fixture.pool)
            .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET ended_at=? WHERE session_id='session-1'",
        )
        .bind(fixture.clock.now_millis())
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET state='conflicted' WHERE session_id='session-1'",
        )
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET lease_expires_at=? WHERE session_id='session-1'",
        )
        .bind(fixture.clock.now_millis())
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=0,agent_session_id='session-wrong'
             WHERE id=?",
        )
        .bind(request.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=1,inventory_revision=7,
                inventory_digest=?,inventory_epoch=1,inventory_session_id='session-wrong',
                inventory_boot_id='boot-1',last_inventory_at=1000 WHERE id=?",
        )
        .bind(&request.expected_inventory_digest)
        .bind(request.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, host_id, request) = prepare_approval().await?;
        let credential = URL_SAFE_NO_PAD.encode([8_u8; 32]);
        sqlx::query(
            "INSERT INTO host_credentials
             (id,host_id,host_epoch,generation,verifier,verifier_format,created_at)
             VALUES ('eeeeeeee-0000-4000-8000-000000000001',?,2,2,?,'sha256',1000)",
        )
        .bind(host_id.to_string())
        .bind(credential)
        .execute(&fixture.pool)
        .await?;
        sqlx::query("UPDATE fleet_hosts SET current_credential_generation=2 WHERE id=?")
            .bind(host_id.to_string())
            .execute(&fixture.pool)
            .await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET ended_at=? WHERE session_id='session-1'",
        )
        .bind(fixture.clock.now_millis())
        .execute(&fixture.pool)
        .await?;
        sqlx::query(
            "INSERT INTO fleet_observation_sessions
             (session_id,host_id,host_epoch,boot_id,control_plane_incarnation,
              negotiated_protocol,initial_credential_id,initial_credential_generation,
              current_credential_id,current_credential_generation,initial_request_id,
              initial_body_digest,state,created_at,last_seen_at,lease_expires_at,
              handshake_received_at,handshake_lease_expires_at,handshake_state,
              current_inventory_sequence,current_inventory_digest,current_inventory_received_at)
             VALUES ('session-epoch-wrong',? ,2,'boot-1','inc-1',1,
                     'eeeeeeee-0000-4000-8000-000000000001',2,
                     'eeeeeeee-0000-4000-8000-000000000001',2,
                     'request-epoch-wrong',?, 'observing',1000,1000,61000,1000,61000,'observing',7,?,1000)",
        )
        .bind(host_id.to_string())
        .bind(inventory_digest())
        .bind(inventory_digest())
        .execute(&fixture.pool)
        .await?;
        sqlx::query("UPDATE fleet_hosts SET current_credential_generation=2 WHERE id=?")
            .bind(host_id.to_string())
            .execute(&fixture.pool)
            .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=0,agent_session_id='session-epoch-wrong'
             WHERE id=?",
        )
        .bind(host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=1,inventory_session_id='session-epoch-wrong',
                inventory_boot_id='boot-1',inventory_epoch=1,inventory_revision=7,
                inventory_digest=?,last_inventory_at=1000 WHERE id=?",
        )
        .bind(inventory_digest())
        .bind(host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query("UPDATE fleet_hosts SET inventory_complete=0,boot_id='boot-wrong' WHERE id=?")
            .bind(request.host_id.to_string())
            .execute(&fixture.pool)
            .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=1,inventory_revision=7,
                inventory_digest=?,inventory_epoch=1,inventory_session_id='session-1',
                inventory_boot_id='boot-wrong',last_inventory_at=1000 WHERE id=?",
        )
        .bind(&request.expected_inventory_digest)
        .bind(request.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=0,authority_incarnation='inc-wrong'
             WHERE id=?",
        )
        .bind(request.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        sqlx::query(
            "UPDATE fleet_hosts SET inventory_complete=1,inventory_revision=7,
                inventory_digest=?,inventory_epoch=1,inventory_session_id='session-1',
                inventory_boot_id='boot-1',last_inventory_at=1000 WHERE id=?",
        )
        .bind(&request.expected_inventory_digest)
        .bind(request.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, host_id, request) = prepare_approval().await?;
        sqlx::query("UPDATE host_credentials SET revoked_at=? WHERE host_id=? AND generation=1")
            .bind(fixture.clock.now_millis())
            .bind(host_id.to_string())
            .execute(&fixture.pool)
            .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        fixture.clock.set(45_999);
        fixture
            .service
            .approve_host(&Fixture::principal(), request)
            .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        fixture.clock.set(46_000);
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query("UPDATE fleet_hosts SET last_inventory_at=? WHERE id=?")
            .bind(1_001_i64)
            .bind(request.host_id.to_string())
            .execute(&fixture.pool)
            .await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET current_inventory_received_at=? WHERE session_id='session-1'",
        )
        .bind(1_001_i64)
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    {
        let (fixture, _, request) = prepare_approval().await?;
        sqlx::query(
            "UPDATE fleet_observation_sessions SET current_inventory_received_at=NULL WHERE session_id='session-1'",
        )
        .execute(&fixture.pool)
        .await?;
        assert_approval_rejection(&fixture, request, |error| {
            matches!(error, RegistryError::StaleRevision)
        })
        .await?;
        fixture.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn rotation_replay_binds_actor_body_and_survives_terminal_states() -> Result<()> {
    let fixture = Fixture::new(1_000).await?;
    fixture.install_target().await?;
    let admin_2 = add_second_admin(&fixture).await?;
    let issue = fixture.issue_empty("rotation-binding-enrollment").await?;
    let enrolled = fixture.consume(&issue).await?;
    let first = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "rotation-binding".to_owned(),
            },
        )
        .await?;
    let counts_after_create = fixture.audit_event_count().await?;
    let replay = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(1)?,
                idempotency_key: "rotation-binding".to_owned(),
            },
        )
        .await?;
    assert_eq!(first, replay);
    assert_eq!(counts_after_create, fixture.audit_event_count().await?);
    assert!(matches!(
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(2)?,
                    idempotency_key: "rotation-binding".to_owned(),
                },
            )
            .await,
        Err(RegistryError::IdempotencyConflict)
    ));
    assert!(matches!(
        fixture
            .service
            .request_rotation(
                &admin_2,
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)?,
                    idempotency_key: "rotation-binding".to_owned(),
                },
            )
            .await,
        Err(RegistryError::IdempotencyConflict)
    ));
    sqlx::query("UPDATE users SET role='member' WHERE id=?")
        .bind(ADMIN_2)
        .execute(&fixture.pool)
        .await?;
    let before_demoted_replay = fixture.audit_event_count().await?;
    assert!(matches!(
        fixture
            .service
            .request_rotation(
                &admin_2,
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)?,
                    idempotency_key: "rotation-binding".to_owned(),
                },
            )
            .await,
        Err(RegistryError::Forbidden)
    ));
    assert_eq!(before_demoted_replay, fixture.audit_event_count().await?);

    let old = fixture
        .service
        .authenticate(enrolled.host_id, enrolled.credential.expose())
        .await?;
    let next_secret = OneTimeSecret::generate();
    fixture
        .service
        .exchange_rotation(&old, first.operation_id, next_secret.expose())
        .await?;
    let counts_after_exchange = fixture.audit_event_count().await?;
    assert_eq!(
        first,
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)?,
                    idempotency_key: "rotation-binding".to_owned(),
                },
            )
            .await?
    );
    assert_eq!(counts_after_exchange, fixture.audit_event_count().await?);
    let next = fixture
        .service
        .authenticate(enrolled.host_id, next_secret.expose())
        .await?;
    fixture
        .service
        .acknowledge_rotation(&next, first.operation_id)
        .await?;
    let counts_after_ack = fixture.audit_event_count().await?;
    assert_eq!(
        first,
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(1)?,
                    idempotency_key: "rotation-binding".to_owned(),
                },
            )
            .await?
    );
    assert_eq!(counts_after_ack, fixture.audit_event_count().await?);

    let expired = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(2)?,
                idempotency_key: "rotation-expired-binding".to_owned(),
            },
        )
        .await?;
    fixture.clock.set(expired.expires_at);
    let before_expired_replay = fixture.audit_event_count().await?;
    assert_eq!(
        expired,
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(2)?,
                    idempotency_key: "rotation-expired-binding".to_owned(),
                },
            )
            .await?
    );
    assert_eq!(before_expired_replay, fixture.audit_event_count().await?);
    let revoked = fixture
        .service
        .request_rotation(
            &Fixture::principal(),
            RequestRotation {
                host_id: enrolled.host_id,
                expected_generation: Generation::new(2)?,
                idempotency_key: "rotation-revoked-binding".to_owned(),
            },
        )
        .await?;
    fixture
        .service
        .revoke_credential(&Fixture::principal(), enrolled.host_id, Generation::new(2)?)
        .await?;
    let before_revoked_replay = fixture.audit_event_count().await?;
    assert_eq!(
        revoked,
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(2)?,
                    idempotency_key: "rotation-revoked-binding".to_owned(),
                },
            )
            .await?
    );
    assert_eq!(before_revoked_replay, fixture.audit_event_count().await?);

    let revision: i64 = sqlx::query_scalar("SELECT revision FROM fleet_hosts WHERE id=?")
        .bind(enrolled.host_id.to_string())
        .fetch_one(&fixture.pool)
        .await?;
    let recovery = match fixture
        .service
        .recover_host(
            &Fixture::principal(),
            RecoverHost {
                host_id: enrolled.host_id,
                expected_revision: Revision::new(u64::try_from(revision)?)?,
                idempotency_key: "rotation-binding-recovery".to_owned(),
            },
        )
        .await?
    {
        RecoveryIssue::Created(recovery) => recovery,
        RecoveryIssue::Replay(_) => anyhow::bail!("rotation recovery unexpectedly replayed"),
    };
    fixture.consume_recovery(&recovery).await?;
    let before_recovered_replay = fixture.audit_event_count().await?;
    assert_eq!(
        revoked,
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(2)?,
                    idempotency_key: "rotation-revoked-binding".to_owned(),
                },
            )
            .await?
    );
    assert_eq!(before_recovered_replay, fixture.audit_event_count().await?);
    sqlx::query("UPDATE fleet_hosts SET lifecycle_state='retired' WHERE id=?")
        .bind(enrolled.host_id.to_string())
        .execute(&fixture.pool)
        .await?;
    let before_retired_replay = fixture.audit_event_count().await?;
    assert_eq!(
        revoked,
        fixture
            .service
            .request_rotation(
                &Fixture::principal(),
                RequestRotation {
                    host_id: enrolled.host_id,
                    expected_generation: Generation::new(2)?,
                    idempotency_key: "rotation-revoked-binding".to_owned(),
                },
            )
            .await?
    );
    assert_eq!(before_retired_replay, fixture.audit_event_count().await?);
    fixture.close().await
}
