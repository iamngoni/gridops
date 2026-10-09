//! Private, transactional placement admission.
//!
//! This module performs only local `SQLite` work. Provider registration, agent
//! calls, and runtime side effects happen after the durable prepare command is
//! committed by a later reconciler.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(test)]
use sqlx::SqlitePool;
use sqlx::{Row as _, Sqlite, Transaction};
use uuid::Uuid;

use super::{
    authorization::Principal,
    capabilities::{
        BackendCapabilities, BackendIdentity, CurrentAuthority, CurrentProbe, ExecutionRequirement,
        SupportedRunner, TargetKind,
    },
    domain::{
        Architecture, BitbucketConnectionId, CiTarget, ExecutionOs, ExternalId, NonNilTargetUuid,
        ResourceAmount, ResourceRequest, RuntimeKind,
    },
    ids::{
        AllocationId, AuthoritySessionId, BackendId, CommandId, ControlPlaneIncarnation,
        FleetIdParseError, Generation, HostEpoch, HostId, OperationId, PlacementId, ProfileId,
        Revision, UpstreamText,
    },
    protocol::{
        agent::{
            AgentCommandEnvelope, AgentOperation, CommandScope, ExecutionConfiguration,
            PrepareEnvironment, PreparedWorkload, RunnerMode,
        },
        primitives::{IdempotencyKey, ImageReference, ProtocolVersion, WireTimestamp},
    },
    service::{self, FleetError, WorkloadKind},
    store,
};

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanLimit(usize);

#[cfg(test)]
impl ScanLimit {
    pub(crate) const fn new(value: usize) -> Result<Self, FleetError> {
        if value == 0 || value > 100 {
            Err(FleetError::InvalidRequest(
                "scan limit must be between 1 and 100",
            ))
        } else {
            Ok(Self(value))
        }
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SliceReport {
    pub(crate) scanned: usize,
    pub(crate) admitted: usize,
    pub(crate) queued: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    host_id: String,
    backend_id: String,
    domain_id: String,
    host_epoch: i64,
    authority_session_id: String,
    boot_id: String,
    control_plane_incarnation: String,
    session_id: Option<String>,
    profile_revision: i64,
    cpu_millis: i64,
    memory_mib: i64,
    disk_bytes: i64,
    runtime_kind: String,
    execution_os: String,
    architecture: String,
    mode: String,
    requires_interactive: bool,
    requires_docker_socket: bool,
    labels_json: String,
    config_json: String,
    image: String,
    capabilities_json: String,
    probe_received_at: Option<i64>,
    probe_epoch: Option<i64>,
    probe_session_id: Option<String>,
    probe_boot_id: Option<String>,
    probe_incarnation: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OperatorConfiguration {
    #[serde(default)]
    network: Option<UpstreamText>,
}

#[derive(Debug, Clone)]
struct DomainRow {
    id: String,
    parent_domain_id: Option<String>,
    cpu_millis: i64,
    memory_mib: i64,
    disk_bytes: i64,
    revision: i64,
}

#[cfg(test)]
pub(crate) async fn run_slice(
    pool: &SqlitePool,
    controller: &Principal,
    limit: ScanLimit,
) -> Result<SliceReport, FleetError> {
    if !matches!(
        controller,
        Principal::Reconciler | Principal::Autoscaler | Principal::FixAgent
    ) {
        return Err(FleetError::Forbidden);
    }
    let rows = sqlx::query(
        "SELECT i.id,i.workload_id,i.generation,i.kind,i.pool_id,i.profile_id,i.target_id,
                i.agent_run_id,i.expected_profile_revision
          FROM workload_intents i
          WHERE i.status = 'queued'
            AND NOT EXISTS (SELECT 1 FROM workload_placements p WHERE p.intent_id=i.id)
          ORDER BY i.created_at,i.id
          LIMIT ?",
    )
    .bind(
        i64::try_from(limit.get())
            .map_err(|_| FleetError::InvalidRequest("scan limit overflow"))?,
    )
    .fetch_all(pool)
    .await?;
    let mut report = SliceReport {
        scanned: rows.len(),
        ..SliceReport::default()
    };
    for row in rows {
        let operation_id: String = row.get("id");
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        match admit_operation(&mut tx, controller, &operation_id).await? {
            AdmissionOutcome::Admitted => {
                tx.commit().await?;
                report.admitted += 1;
            }
            AdmissionOutcome::Queued => {
                tx.commit().await?;
                report.queued += 1;
            }
            AdmissionOutcome::Noop => tx.commit().await?,
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionOutcome {
    Admitted,
    Queued,
    Noop,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Awaiting persisted fairness scheduler integration"
    )
)]
pub(crate) async fn admit_operation(
    tx: &mut Transaction<'_, Sqlite>,
    controller: &Principal,
    operation_id: &str,
) -> Result<AdmissionOutcome, FleetError> {
    if !matches!(
        controller,
        Principal::Reconciler | Principal::Autoscaler | Principal::FixAgent
    ) {
        return Err(FleetError::Forbidden);
    }
    let Some(row) = sqlx::query(
        "SELECT i.id,i.workload_id,i.generation,i.kind,i.pool_id,i.profile_id,i.target_id,
                i.agent_run_id,i.expected_profile_revision,i.status AS intent_status,
                o.status AS operation_status
           FROM workload_intents i JOIN fleet_operations o ON o.id=i.id
          WHERE i.id=?",
    )
    .bind(operation_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::NotFound);
    };
    let intent_status: String = row.get("intent_status");
    let operation_status: String = row.get("operation_status");
    if intent_status != "queued" || operation_status != "queued" {
        return Ok(AdmissionOutcome::Noop);
    }
    let intent = store::IntentRow {
        id: row.get("id"),
        workload_id: row.get("workload_id"),
        generation: row.get("generation"),
        kind: row.get("kind"),
        pool_id: row.get("pool_id"),
        profile_id: row.get("profile_id"),
        target_id: row.get("target_id"),
        agent_run_id: row.get("agent_run_id"),
        expected_profile_revision: row.get("expected_profile_revision"),
    };
    let now = crate::db::now_millis();
    if let Err(error) = service::recheck_intent_authorization(tx, controller, &intent).await {
        if is_durable_candidate_error(&error) {
            persist_rejection(tx, &intent.id, reason_code(&error), now).await?;
            return Ok(AdmissionOutcome::Queued);
        }
        return Err(error);
    }
    match ensure_control_plane_ready(tx).await {
        Ok(()) => {}
        Err(FleetError::Queued(reason)) => {
            persist_rejection(tx, &intent.id, reason, now).await?;
            return Ok(AdmissionOutcome::Queued);
        }
        Err(error) => return Err(error),
    }
    let candidates = candidates(tx, &intent, now).await?;
    let mut last_reason = None;
    for candidate in candidates {
        if let Some(reason) = candidate_compatible(tx, &intent, &candidate).await? {
            last_reason = Some(reason);
            continue;
        }
        if !pool_has_capacity(tx, &intent, now).await? {
            last_reason = Some(if intent.kind == WorkloadKind::FixSandbox.as_str() {
                "physical_resource_exhausted"
            } else {
                "pool_capacity_exhausted"
            });
            continue;
        }
        if let Some(reason) = domains_fit(tx, &candidate).await? {
            last_reason = Some(reason);
            continue;
        }
        insert_admission(tx, &intent, &candidate, now).await?;
        return Ok(AdmissionOutcome::Admitted);
    }
    persist_rejection(
        tx,
        &intent.id,
        last_reason.unwrap_or("no_eligible_host"),
        now,
    )
    .await?;
    Ok(AdmissionOutcome::Queued)
}

fn is_durable_candidate_error(error: &FleetError) -> bool {
    matches!(
        error,
        FleetError::Forbidden
            | FleetError::NotFound
            | FleetError::StaleProfileRevision
            | FleetError::Queued(_)
            | FleetError::IdempotencyConflict
    )
}

fn reason_code(error: &FleetError) -> &'static str {
    match error {
        FleetError::Forbidden => "forbidden",
        FleetError::NotFound => "policy_unavailable",
        FleetError::StaleProfileRevision => "stale_profile_revision",
        FleetError::Queued(reason) => reason,
        FleetError::IdempotencyConflict => "duplicate_workload",
        _ => "no_eligible_host",
    }
}

async fn persist_rejection(
    tx: &mut Transaction<'_, Sqlite>,
    intent_id: &str,
    reason: &str,
    now: i64,
) -> Result<(), FleetError> {
    sqlx::query(
        "UPDATE workload_intents
            SET reason_code=?,updated_at=?
          WHERE id=? AND status='queued'
            AND EXISTS (SELECT 1 FROM fleet_operations
                         WHERE id=workload_intents.id AND status='queued')",
    )
    .bind(reason)
    .bind(now)
    .bind(intent_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE fleet_operations
            SET reason_code=?,updated_at=?
          WHERE id=? AND status='queued'
            AND EXISTS (SELECT 1 FROM workload_intents
                         WHERE id=fleet_operations.id AND status='queued')",
    )
    .bind(reason)
    .bind(now)
    .bind(intent_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn ensure_control_plane_ready(tx: &mut Transaction<'_, Sqlite>) -> Result<(), FleetError> {
    let ready: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM fleet_control_plane WHERE singleton=1 AND readiness='active'",
    )
    .fetch_optional(&mut **tx)
    .await?;
    if ready != Some(1) {
        return Err(FleetError::Queued("control_plane_not_ready"));
    }
    Ok(())
}

async fn candidate_compatible(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    candidate: &Candidate,
) -> Result<Option<&'static str>, FleetError> {
    let target = target_from_db(tx, &intent.target_id).await?;
    let runtime = parse_runtime_kind(&candidate.runtime_kind)?;
    let execution_os = parse_execution_os(&candidate.execution_os)?;
    let architecture = parse_architecture(&candidate.architecture)?;
    let persistent = candidate.mode == "persistent";
    if candidate.mode != "ephemeral" && !persistent {
        return Ok(Some("unsupported_mode"));
    }
    if intent.kind == WorkloadKind::FixSandbox.as_str()
        && (!matches!(target, CiTarget::GithubRepository { .. })
            || runtime != RuntimeKind::Docker
            || execution_os != ExecutionOs::Linux)
    {
        return Ok(Some("unsupported_mode"));
    }
    let valid_labels = serde_json::from_str::<Vec<String>>(&candidate.labels_json)
        .map(|labels| {
            labels
                .into_iter()
                .all(|label| UpstreamText::new(label).is_ok())
        })
        .unwrap_or(false);
    if !valid_labels
        || serde_json::from_str::<OperatorConfiguration>(&candidate.config_json).is_err()
        || candidate.image.is_empty()
        || candidate.image.chars().count() > 300
        || candidate.image.chars().any(char::is_control)
    {
        return Ok(Some("invalid_profile"));
    }
    let Ok(image) = ImageReference::try_from(candidate.image.clone()) else {
        return Ok(Some("invalid_profile"));
    };
    let resources = ResourceRequest::new(
        u32::try_from(candidate.cpu_millis)
            .map_err(|_| FleetError::Invariant("invalid profile CPU"))?,
        u64::try_from(candidate.memory_mib)
            .map_err(|_| FleetError::Invariant("invalid profile memory"))?,
        u64::try_from(candidate.disk_bytes)
            .map_err(|_| FleetError::Invariant("invalid profile disk"))?,
    )
    .map_err(|_| FleetError::Invariant("invalid profile resources"))?;
    let identity = BackendIdentity {
        host_id: parse_id::<HostId>(&candidate.host_id)?,
        backend_id: parse_id::<BackendId>(&candidate.backend_id)?,
        runtime_kind: runtime,
        execution_os,
        architecture,
    };
    let authority = CurrentAuthority {
        host_id: identity.host_id,
        host_epoch: HostEpoch::new(
            u64::try_from(candidate.host_epoch)
                .map_err(|_| FleetError::Invariant("invalid host epoch"))?,
        )
        .map_err(|_| FleetError::Invariant("invalid host epoch"))?,
        authority_session_id: candidate
            .authority_session_id
            .parse::<AuthoritySessionId>()
            .map_err(|_| FleetError::Invariant("invalid authority session ID"))?,
        boot_id: UpstreamText::new(candidate.boot_id.clone())
            .map_err(|_| FleetError::Invariant("invalid boot identity"))?,
        control_plane_incarnation: candidate
            .control_plane_incarnation
            .parse::<ControlPlaneIncarnation>()
            .map_err(|_| FleetError::Invariant("invalid control-plane incarnation"))?,
    };
    let probe = match (
        candidate.probe_received_at,
        candidate.probe_epoch,
        candidate.probe_session_id.as_deref(),
        candidate.probe_boot_id.as_deref(),
        candidate.probe_incarnation.as_deref(),
    ) {
        (Some(received_at), Some(epoch), Some(session), Some(boot), Some(incarnation)) => {
            Some(CurrentProbe {
                host_id: identity.host_id,
                backend_id: identity.backend_id,
                received_at: WireTimestamp::from_millis(received_at)
                    .map_err(|_| FleetError::Invariant("invalid probe receipt timestamp"))?,
                host_epoch: HostEpoch::new(
                    u64::try_from(epoch)
                        .map_err(|_| FleetError::Invariant("invalid probe epoch"))?,
                )
                .map_err(|_| FleetError::Invariant("invalid probe epoch"))?,
                authority_session_id: session
                    .parse::<AuthoritySessionId>()
                    .map_err(|_| FleetError::Invariant("invalid probe session ID"))?,
                boot_id: UpstreamText::new(boot.to_owned())
                    .map_err(|_| FleetError::Invariant("invalid probe boot identity"))?,
                control_plane_incarnation: incarnation
                    .parse::<ControlPlaneIncarnation>()
                    .map_err(|_| FleetError::Invariant("invalid probe incarnation"))?,
            })
        }
        _ => None,
    };
    let requirement = ExecutionRequirement {
        runtime_kind: runtime,
        execution_os,
        architecture,
        runner: SupportedRunner {
            target_kind: TargetKind::from(&target),
            mode: if persistent {
                RunnerMode::Persistent
            } else {
                RunnerMode::Ephemeral
            },
        },
        environment: image,
        resources,
        requires_interactive: candidate.requires_interactive,
        requires_docker_socket: candidate.requires_docker_socket,
    };
    let Ok(capabilities) = BackendCapabilities::parse(candidate.capabilities_json.as_bytes())
    else {
        return Ok(Some("capability_unverified"));
    };
    match capabilities.evaluate(
        &identity,
        &requirement,
        probe.as_ref(),
        &authority,
        WireTimestamp::from_millis(crate::db::now_millis())
            .map_err(|_| FleetError::Invariant("invalid admission timestamp"))?,
    ) {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(error.reason_code())),
    }
}

async fn candidates(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    now: i64,
) -> Result<Vec<Candidate>, FleetError> {
    let rows = sqlx::query(
        "SELECT h.id AS host_id,b.id AS backend_id,b.domain_id,h.epoch,
                h.agent_session_id,h.boot_id,h.authority_incarnation,h.authority_expires_at,
                p.revision AS profile_revision,p.cpu_millis,p.memory_mib,p.disk_bytes,
                p.runtime_kind,p.execution_os,p.architecture,p.mode,p.requires_interactive,
                p.requires_docker_socket,p.labels_json,p.config_json,p.image,
                b.capabilities_json,b.last_probed_at,b.probe_epoch,b.probe_session_id,
                b.probe_boot_id,b.probe_incarnation
           FROM workload_intents i
           JOIN fleet_operations o ON o.id=i.id
           JOIN runner_pools rp ON rp.id=i.pool_id
           JOIN pool_execution_profiles p ON p.id=i.profile_id AND p.pool_id=i.pool_id
           JOIN host_backends b ON b.runtime_kind=p.runtime_kind
                                AND b.execution_os=p.execution_os
                                AND b.architecture=p.architecture
           JOIN fleet_hosts h ON h.id=b.host_id
           JOIN fleet_control_plane cp ON cp.singleton=1
                                      AND cp.readiness='active'
                                      AND cp.incarnation=h.authority_incarnation
           JOIN host_target_grants g ON g.host_id=h.id AND g.target_id=i.target_id
          WHERE i.id=? AND i.status='queued' AND o.status='queued'
            AND NOT EXISTS (SELECT 1 FROM workload_placements existing WHERE existing.intent_id=i.id)
            AND rp.paused=0 AND rp.state='active'
            AND p.enabled=1 AND p.revision=i.expected_profile_revision
            AND (i.kind != 'fix_sandbox' OR (
              p.runtime_kind='docker' AND p.execution_os='linux'
              AND EXISTS (
                SELECT 1 FROM fleet_ci_targets t JOIN agent_runs ar
                  ON ar.repository_id=t.repository_id
                 WHERE t.id=i.target_id AND t.kind='github_repository'
                   AND ar.id=i.agent_run_id AND ar.status IN ('queued','running')
                   AND ar.cancel_requested=0
                   AND (o.principal_kind != 'user' OR ar.requested_by=o.requested_by_user_id)
              )
            ))
            AND (i.kind != 'fix_sandbox' OR NOT EXISTS (
              SELECT 1 FROM workload_intents duplicate
               WHERE duplicate.agent_run_id=i.agent_run_id AND duplicate.id != i.id
                 AND duplicate.status IN ('queued','admitted')
            ))
            AND EXISTS (SELECT 1 FROM profile_ci_targets pct
                         WHERE pct.profile_id=i.profile_id AND pct.target_id=i.target_id
                           AND pct.enabled=1 AND pct.revoked_at IS NULL)
            AND b.enabled=1 AND b.readiness='ready'
            AND g.allow_schedule=1 AND g.revoked_at IS NULL
            AND (p.requires_interactive=0 OR g.allow_interactive=1)
            AND (o.principal_kind != 'user' OR EXISTS (
              SELECT 1 FROM host_user_grants hug
               WHERE hug.host_id=h.id AND hug.user_id=o.requested_by_user_id
                 AND hug.permission IN ('operator','admin') AND hug.revoked_at IS NULL
            ))
            AND h.enrollment_state='approved' AND h.lifecycle_state='active'
            AND h.integrity_state='verified' AND h.pressure_state='normal'
            AND h.last_heartbeat_at IS NOT NULL AND h.last_heartbeat_at > ?
            AND h.last_heartbeat_at <= ?
            AND h.last_inventory_at IS NOT NULL AND h.last_inventory_at > ?
            AND h.last_inventory_at <= ?
            AND h.authority_expires_at IS NOT NULL AND h.authority_expires_at > ?
            AND h.authority_expires_at <= ?
            AND h.inventory_complete=1
            AND h.authority_incarnation IS NOT NULL
            AND h.agent_session_id IS NOT NULL AND h.boot_id IS NOT NULL
            AND h.inventory_epoch=h.epoch
            AND h.inventory_session_id=h.agent_session_id
            AND h.inventory_boot_id=h.boot_id
            AND NOT EXISTS (
              SELECT 1 FROM fleet_ci_targets t JOIN installations ins
                ON ins.id=t.installation_id
               WHERE t.id=i.target_id AND ins.suspended_at IS NOT NULL
            )
            AND NOT EXISTS (
              SELECT 1 FROM runner_pool_installations rpi
               JOIN installations pins ON pins.id=rpi.installation_id
              WHERE rpi.pool_id=i.pool_id AND pins.suspended_at IS NOT NULL
            )
            AND (
              o.principal_kind != 'user' OR NOT EXISTS (
                SELECT 1 FROM (
                  SELECT installation_id FROM runner_pool_installations WHERE pool_id=i.pool_id
                  UNION
                  SELECT installation_id FROM runner_pools WHERE id=i.pool_id
                ) required
                WHERE NOT EXISTS (
                  SELECT 1 FROM user_installations ui JOIN users u ON u.id=ui.user_id
                   WHERE ui.user_id=o.requested_by_user_id
                     AND ui.installation_id=required.installation_id
                     AND (ui.permission='admin' OR u.role='admin')
                )
              )
            )
            AND (
              o.principal_kind != 'user' OR NOT EXISTS (
                SELECT 1 FROM runner_pool_bitbucket_connections rpc
                WHERE rpc.pool_id=i.pool_id AND NOT EXISTS (
                  SELECT 1 FROM bitbucket_user_grants bug JOIN users u ON u.id=bug.user_id
                   WHERE bug.user_id=o.requested_by_user_id
                     AND bug.connection_id=rpc.connection_id
                     AND bug.revoked_at IS NULL
                     AND (bug.permission='admin' OR u.role='admin')
                )
              )
            )
            AND (
              o.principal_kind != 'user' OR EXISTS (
                SELECT 1 FROM fleet_ci_targets t
                 WHERE t.id=i.target_id AND (
                   (t.installation_id IS NOT NULL AND EXISTS (
                     SELECT 1 FROM user_installations ui JOIN users u ON u.id=ui.user_id
                      WHERE ui.user_id=o.requested_by_user_id
                        AND ui.installation_id=t.installation_id
                        AND (ui.permission='admin' OR u.role='admin')
                   )) OR
                   (t.bitbucket_connection_id IS NOT NULL AND EXISTS (
                     SELECT 1 FROM bitbucket_user_grants bug JOIN users u ON u.id=bug.user_id
                      WHERE bug.user_id=o.requested_by_user_id
                        AND bug.connection_id=t.bitbucket_connection_id
                        AND bug.revoked_at IS NULL
                        AND (bug.permission='admin' OR u.role='admin')
                   ))
                 )
              )
            )
            AND (p.runtime_kind != 'native_process' OR g.allow_native=1)
            AND (p.requires_docker_socket=0 OR g.allow_docker_socket=1)
          ORDER BY h.id,b.id",
    )
    .bind(&intent.id)
    .bind(now.saturating_sub(45_000))
    .bind(now)
    .bind(now.saturating_sub(45_000))
    .bind(now)
    .bind(now)
    .bind(now.saturating_add(90_000))
    .fetch_all(&mut **tx)
    .await?;
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let candidate = Candidate {
            host_id: row.get("host_id"),
            backend_id: row.get("backend_id"),
            domain_id: row.get("domain_id"),
            host_epoch: row.get("epoch"),
            authority_session_id: row.get("agent_session_id"),
            boot_id: row.get("boot_id"),
            control_plane_incarnation: row.get("authority_incarnation"),
            session_id: None,
            profile_revision: row.get("profile_revision"),
            cpu_millis: row.get("cpu_millis"),
            memory_mib: row.get("memory_mib"),
            disk_bytes: row.get("disk_bytes"),
            runtime_kind: row.get("runtime_kind"),
            execution_os: row.get("execution_os"),
            architecture: row.get("architecture"),
            mode: row.get("mode"),
            requires_interactive: row.get::<i64, _>("requires_interactive") != 0,
            requires_docker_socket: row.get::<i64, _>("requires_docker_socket") != 0,
            labels_json: row.get("labels_json"),
            config_json: row.get("config_json"),
            image: row.get("image"),
            capabilities_json: row.get("capabilities_json"),
            probe_received_at: row.get("last_probed_at"),
            probe_epoch: row.get("probe_epoch"),
            probe_session_id: row.get("probe_session_id"),
            probe_boot_id: row.get("probe_boot_id"),
            probe_incarnation: row.get("probe_incarnation"),
        };
        let candidate = if candidate.requires_interactive {
            let Some(session_id) = available_session(tx, intent, &candidate.host_id, now).await?
            else {
                continue;
            };
            Candidate {
                session_id: Some(session_id),
                ..candidate
            }
        } else {
            candidate
        };
        if !host_selector_matches(tx, intent, &candidate.host_id).await? {
            continue;
        }
        result.push(candidate);
    }
    Ok(result)
}

async fn available_session(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    host_id: &str,
    now: i64,
) -> Result<Option<String>, FleetError> {
    let row = sqlx::query(
        "SELECT s.id
           FROM host_sessions s
           JOIN host_session_target_grants st ON st.session_id=s.id AND st.target_id=?
          WHERE s.host_id=? AND s.helper_state='ready' AND s.authorized=1
            AND st.revoked_at IS NULL
            AND s.last_seen_at IS NOT NULL AND s.last_seen_at > ? AND s.last_seen_at <= ?
            AND (SELECT COUNT(*) FROM workload_placements p
                  WHERE p.session_id=s.id
                    AND NOT (
                      p.local_absence_proven_at IS NOT NULL
                      AND p.delayed_starts_excluded_at IS NOT NULL
                      AND NOT EXISTS (
                        SELECT 1 FROM capacity_allocations a
                         WHERE a.placement_id=p.id AND a.state != 'released'
                      )
                    )) < s.max_jobs
          ORDER BY s.id LIMIT 1",
    )
    .bind(&intent.target_id)
    .bind(host_id)
    .bind(now.saturating_sub(45_000))
    .bind(now)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| row.get("id")))
}

async fn host_selector_matches(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    host_id: &str,
) -> Result<bool, FleetError> {
    let host_selectors: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM profile_host_selectors WHERE profile_id=? AND kind='host'",
    )
    .bind(&intent.profile_id)
    .fetch_one(&mut **tx)
    .await?;
    let tag_selectors: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM profile_host_selectors WHERE profile_id=? AND kind='tag'",
    )
    .bind(&intent.profile_id)
    .fetch_one(&mut **tx)
    .await?;
    if host_selectors == 0 && tag_selectors == 0 {
        let all_hosts: i64 = sqlx::query_scalar(
            "SELECT all_authorized_hosts FROM pool_execution_profiles WHERE id=? AND pool_id=?",
        )
        .bind(&intent.profile_id)
        .bind(&intent.pool_id)
        .fetch_one(&mut **tx)
        .await?;
        return Ok(all_hosts != 0);
    }
    if host_selectors != 0 {
        let host_match: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM profile_host_selectors
              WHERE profile_id=? AND kind='host' AND host_id=?",
        )
        .bind(&intent.profile_id)
        .bind(host_id)
        .fetch_one(&mut **tx)
        .await?;
        if host_match == 0 {
            return Ok(false);
        }
    }
    let tags = sqlx::query(
        "SELECT tag_key,tag_value FROM profile_host_selectors
          WHERE profile_id=? AND kind='tag'",
    )
    .bind(&intent.profile_id)
    .fetch_all(&mut **tx)
    .await?;
    let Some(host_tags): Option<String> =
        sqlx::query_scalar("SELECT tags_json FROM fleet_hosts WHERE id=?")
            .bind(host_id)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Ok(false);
    };
    let host_tags: serde_json::Value = serde_json::from_str(&host_tags)
        .map_err(|_| FleetError::Invariant("host tags are not valid JSON"))?;
    let matching_tags = tags.iter().filter(|row| {
        let key: String = row.get("tag_key");
        let value: String = row.get("tag_value");
        host_tags.get(&key).and_then(serde_json::Value::as_str) == Some(value.as_str())
    });
    Ok(matching_tags.count() == usize::try_from(tag_selectors).unwrap_or(usize::MAX))
}

async fn pool_has_capacity(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    _now: i64,
) -> Result<bool, FleetError> {
    if intent.kind != WorkloadKind::CiRunner.as_str() {
        return Ok(true);
    }
    let Some((max_count,)): Option<(i64,)> =
        sqlx::query_as("SELECT max_count FROM runner_pools WHERE id=?")
            .bind(&intent.pool_id)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Err(FleetError::NotFound);
    };
    let legacy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runners r WHERE r.pool_id=? AND r.deleted_at IS NULL
          AND NOT EXISTS (SELECT 1 FROM workload_placements p WHERE p.runner_id=r.id)",
    )
    .bind(&intent.pool_id)
    .fetch_one(&mut **tx)
    .await?;
    let managed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workload_placements p
          JOIN workload_intents i ON i.id=p.intent_id
         WHERE i.pool_id=? AND i.kind='ci_runner'
           AND NOT (
             p.local_absence_proven_at IS NOT NULL
             AND p.delayed_starts_excluded_at IS NOT NULL
             AND NOT EXISTS (
               SELECT 1 FROM capacity_allocations a
                WHERE a.placement_id=p.id AND a.state != 'released'
             )
           )",
    )
    .bind(&intent.pool_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(legacy
        .checked_add(managed)
        .ok_or(FleetError::Invariant("pool count overflow"))?
        < max_count)
}

async fn domains_fit(
    tx: &mut Transaction<'_, Sqlite>,
    candidate: &Candidate,
) -> Result<Option<&'static str>, FleetError> {
    let path = domain_path(tx, &candidate.domain_id, &candidate.host_id).await?;
    let request = ResourceAmount::new(
        u64::try_from(candidate.cpu_millis).map_err(|_| FleetError::Invariant("negative CPU"))?,
        u64::try_from(candidate.memory_mib)
            .map_err(|_| FleetError::Invariant("negative memory"))?,
        u64::try_from(candidate.disk_bytes).map_err(|_| FleetError::Invariant("negative disk"))?,
    )
    .map_err(|_| FleetError::Invariant("profile resource exceeds integer bounds"))?;
    for domain in path {
        let used = sqlx::query(
            "SELECT cpu_millis,memory_mib,disk_bytes FROM capacity_allocations
              WHERE domain_id=? AND host_id=? AND state != 'released'",
        )
        .bind(&domain.id)
        .bind(&candidate.host_id)
        .fetch_all(&mut **tx)
        .await?;
        let mut aggregate = ResourceAmount::ZERO;
        for row in used {
            let amount = ResourceAmount::new(
                u64::try_from(row.get::<i64, _>("cpu_millis"))
                    .map_err(|_| FleetError::Invariant("negative allocation"))?,
                u64::try_from(row.get::<i64, _>("memory_mib"))
                    .map_err(|_| FleetError::Invariant("negative allocation"))?,
                u64::try_from(row.get::<i64, _>("disk_bytes"))
                    .map_err(|_| FleetError::Invariant("negative allocation"))?,
            )
            .map_err(|_| FleetError::Invariant("allocation exceeds integer bounds"))?;
            aggregate = aggregate
                .checked_add(amount)
                .map_err(|_| FleetError::Invariant("allocation aggregate overflow"))?;
        }
        let capacity = ResourceAmount::new(
            u64::try_from(domain.cpu_millis)
                .map_err(|_| FleetError::Invariant("negative budget"))?,
            u64::try_from(domain.memory_mib)
                .map_err(|_| FleetError::Invariant("negative budget"))?,
            u64::try_from(domain.disk_bytes)
                .map_err(|_| FleetError::Invariant("negative budget"))?,
        )
        .map_err(|_| FleetError::Invariant("budget exceeds integer bounds"))?;
        let Ok(remaining) = capacity.checked_sub(aggregate) else {
            return Ok(Some(if domain.parent_domain_id.is_none() {
                "physical_resource_exhausted"
            } else {
                "child_resource_exhausted"
            }));
        };
        if !remaining.fits_request(request) {
            return Ok(Some(if domain.parent_domain_id.is_none() {
                "physical_resource_exhausted"
            } else {
                "child_resource_exhausted"
            }));
        }
    }
    Ok(None)
}

async fn domain_path(
    tx: &mut Transaction<'_, Sqlite>,
    leaf: &str,
    host_id: &str,
) -> Result<Vec<DomainRow>, FleetError> {
    let mut result = Vec::new();
    let mut current = Some(leaf.to_owned());
    let mut seen = HashSet::new();
    while let Some(id) = current.take() {
        if !seen.insert(id.clone())
            || result.len() >= super::protocol::primitives::MAX_RESOURCE_DOMAIN_DEPTH
        {
            return Err(FleetError::Invariant(
                "resource domain topology is cyclic or too deep",
            ));
        }
        let Some(row) = sqlx::query(
            "SELECT id,parent_domain_id,host_id,cpu_millis,memory_mib,disk_bytes,revision
               FROM host_resource_domains WHERE id=? AND host_id=?",
        )
        .bind(&id)
        .bind(host_id)
        .fetch_optional(&mut **tx)
        .await?
        else {
            return Err(FleetError::Invariant(
                "resource domain is missing or cross-host",
            ));
        };
        let parent: Option<String> = row.get("parent_domain_id");
        result.push(DomainRow {
            id: row.get("id"),
            parent_domain_id: parent.clone(),
            cpu_millis: row.get("cpu_millis"),
            memory_mib: row.get("memory_mib"),
            disk_bytes: row.get("disk_bytes"),
            revision: row.get("revision"),
        });
        current = parent;
    }
    Ok(result)
}

async fn insert_admission(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    candidate: &Candidate,
    now: i64,
) -> Result<(), FleetError> {
    let placement_id = new_id::<PlacementId>()?;
    let command_id = Uuid::new_v4();
    let generation = intent.generation;
    let epoch = candidate.host_epoch;
    let configuration = effective_configuration(candidate)?;
    let snapshot = json!({
        "runtime_kind": candidate.runtime_kind,
        "execution_os": candidate.execution_os,
        "architecture": candidate.architecture,
        "mode": candidate.mode,
        "requires_interactive": candidate.requires_interactive,
        "requires_docker_socket": candidate.requires_docker_socket,
        "labels": serde_json::from_str::<serde_json::Value>(&candidate.labels_json)
            .map_err(|_| FleetError::Invariant("invalid profile labels"))?,
        "config": serde_json::to_value(&configuration)
            .map_err(|_| FleetError::Invariant("invalid effective configuration"))?,
        "image": candidate.image,
        "cpu_millis": candidate.cpu_millis,
        "memory_mib": candidate.memory_mib,
        "disk_bytes": candidate.disk_bytes,
        "profile_revision": candidate.profile_revision,
    });
    sqlx::query(
        "INSERT INTO workload_placements
         (id,intent_id,workload_id,generation,host_id,backend_id,host_epoch,session_id,
          profile_revision,target_id,config_snapshot_json,cpu_millis,memory_mib,disk_bytes,
          state,agent_run_id,created_at,updated_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?, 'preparing',?,?,?)",
    )
    .bind(placement_id.to_string())
    .bind(&intent.id)
    .bind(&intent.workload_id)
    .bind(generation)
    .bind(&candidate.host_id)
    .bind(&candidate.backend_id)
    .bind(epoch)
    .bind(candidate.session_id.as_deref())
    .bind(candidate.profile_revision)
    .bind(&intent.target_id)
    .bind(snapshot.to_string())
    .bind(candidate.cpu_millis)
    .bind(candidate.memory_mib)
    .bind(candidate.disk_bytes)
    .bind(intent.agent_run_id.as_deref())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let path = domain_path(tx, &candidate.domain_id, &candidate.host_id).await?;
    let mut allocation_ids = Vec::with_capacity(path.len());
    for domain in path {
        let allocation_id = new_id::<AllocationId>()?;
        allocation_ids.push(allocation_id.to_string());
        sqlx::query(
            "INSERT INTO capacity_allocations
             (id,placement_id,host_id,backend_id,host_epoch,domain_id,policy_revision,
              cpu_millis,memory_mib,disk_bytes,created_at,updated_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(allocation_id.to_string())
        .bind(placement_id.to_string())
        .bind(&candidate.host_id)
        .bind(&candidate.backend_id)
        .bind(epoch)
        .bind(domain.id)
        .bind(domain.revision)
        .bind(candidate.cpu_millis)
        .bind(candidate.memory_mib)
        .bind(candidate.disk_bytes)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }

    let operation_id = &intent.id;
    let deadline = now
        .checked_add(1_200_000)
        .ok_or(FleetError::Invariant("prepare deadline overflow"))?;
    let envelope = build_prepare_envelope(
        tx,
        intent,
        candidate,
        placement_id,
        command_id,
        generation,
        &allocation_ids,
        now,
        deadline,
    )
    .await?;
    let envelope = envelope
        .seal_hash()
        .map_err(|_| FleetError::Invariant("invalid prepare command envelope"))?;
    let command_hash = envelope.request_hash.clone();
    let command_payload_text = serde_json::to_string(&envelope)
        .map_err(|_| FleetError::Invariant("prepare command serialization failed"))?;
    sqlx::query(
        "INSERT INTO host_commands
         (id,operation_id,scope,placement_id,host_id,backend_id,host_epoch,generation,kind,
          protocol_version,request_hash,payload_json,expected_revision,issued_at,deadline_at,idempotency_key)
         VALUES (?,?,?,?,?,?,?,?,'prepare_environment',1,?,?,?,?,?,?)",
    )
    .bind(command_id.to_string())
    .bind(operation_id)
    .bind("placement")
    .bind(placement_id.to_string())
    .bind(&candidate.host_id)
    .bind(&candidate.backend_id)
    .bind(epoch)
    .bind(generation)
    .bind(&command_hash)
    .bind(command_payload_text)
    .bind(candidate.profile_revision)
    .bind(now)
    .bind(deadline)
    .bind(format!("prepare:{placement_id}"))
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "UPDATE workload_intents SET status='admitted',reason_code=NULL,updated_at=? WHERE id=?",
    )
    .bind(now)
    .bind(&intent.id)
    .execute(&mut **tx)
    .await?;
    // Admission only queues the prepare command; delivery determines completion.
    sqlx::query(
        "UPDATE fleet_operations
            SET status='running',reason_code=NULL,updated_at=?
          WHERE id=? AND status='queued'",
    )
    .bind(now)
    .bind(operation_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO fleet_events (id,host_id,placement_id,target_id,kind,detail_json,created_at)
         VALUES (?,?,?,?,?,?,?)",
    )
    .bind(command_id.to_string())
    .bind(&candidate.host_id)
    .bind(placement_id.to_string())
    .bind(&intent.target_id)
    .bind("placement_admitted")
    .bind(json!({"state":"preparing", "runtime_kind": candidate.runtime_kind}).to_string())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn build_prepare_envelope(
    tx: &mut Transaction<'_, Sqlite>,
    intent: &store::IntentRow,
    candidate: &Candidate,
    placement_id: PlacementId,
    command_id: Uuid,
    generation: i64,
    allocation_ids: &[String],
    issued_at: i64,
    deadline_at: i64,
) -> Result<AgentCommandEnvelope, FleetError> {
    let target = target_from_db(tx, &intent.target_id).await?;
    let profile_id = parse_id::<ProfileId>(&intent.profile_id)?;
    let host_id = parse_id::<HostId>(&candidate.host_id)?;
    let backend_id = parse_id::<BackendId>(&candidate.backend_id)?;
    let host_epoch = HostEpoch::new(
        u64::try_from(candidate.host_epoch)
            .map_err(|_| FleetError::Invariant("invalid host epoch"))?,
    )
    .map_err(|_| FleetError::Invariant("invalid host epoch"))?;
    let generation = Generation::new(
        u64::try_from(generation).map_err(|_| FleetError::Invariant("invalid generation"))?,
    )
    .map_err(|_| FleetError::Invariant("invalid generation"))?;
    let profile_revision = Revision::new(
        u64::try_from(candidate.profile_revision)
            .map_err(|_| FleetError::Invariant("invalid profile revision"))?,
    )
    .map_err(|_| FleetError::Invariant("invalid profile revision"))?;
    let command_id = CommandId::try_new(command_id)
        .map_err(|_| FleetError::Invariant("generated nil command ID"))?;
    let operation_id = parse_id::<OperationId>(&intent.id)?;
    let authority_session_id = candidate
        .authority_session_id
        .parse::<AuthoritySessionId>()
        .map_err(|_| FleetError::Invariant("invalid authority session ID"))?;
    let incarnation = candidate
        .control_plane_incarnation
        .parse::<ControlPlaneIncarnation>()
        .map_err(|_| FleetError::Invariant("invalid control-plane incarnation"))?;
    let runtime_kind = parse_runtime_kind(&candidate.runtime_kind)?;
    let execution_os = parse_execution_os(&candidate.execution_os)?;
    let architecture = parse_architecture(&candidate.architecture)?;
    let mode = match candidate.mode.as_str() {
        "ephemeral" => RunnerMode::Ephemeral,
        "persistent" => RunnerMode::Persistent,
        _ => return Err(FleetError::Invariant("invalid profile mode")),
    };
    let image = ImageReference::try_from(candidate.image.clone())
        .map_err(|_| FleetError::Invariant("invalid profile image"))?;
    let labels = serde_json::from_str::<Vec<String>>(&candidate.labels_json)
        .map_err(|_| FleetError::Invariant("invalid profile labels"))?
        .into_iter()
        .map(|label| {
            UpstreamText::new(label).map_err(|_| FleetError::Invariant("invalid profile label"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let configuration = effective_configuration(candidate)?;
    let resource_request = ResourceRequest::new(
        u32::try_from(candidate.cpu_millis)
            .map_err(|_| FleetError::Invariant("invalid profile CPU"))?,
        u64::try_from(candidate.memory_mib)
            .map_err(|_| FleetError::Invariant("invalid profile memory"))?,
        u64::try_from(candidate.disk_bytes)
            .map_err(|_| FleetError::Invariant("invalid profile disk"))?,
    )
    .map_err(|_| FleetError::Invariant("invalid profile resources"))?;
    let allocation_ids = allocation_ids
        .iter()
        .map(|id| parse_id::<AllocationId>(id))
        .collect::<Result<Vec<_>, _>>()?;
    let operation = AgentOperation::PrepareEnvironment(PrepareEnvironment {
        execution_profile_id: profile_id,
        profile_revision,
        workload: match intent.kind.as_str() {
            "ci_runner" => PreparedWorkload::CiRunner,
            "fix_sandbox" => PreparedWorkload::FixSandbox,
            _ => return Err(FleetError::Invariant("unknown workload kind")),
        },
        target,
        runtime_kind,
        execution_os,
        architecture,
        mode,
        image,
        labels,
        configuration,
        resource_request,
        allocation_ids,
    });
    Ok(AgentCommandEnvelope {
        protocol_version: ProtocolVersion::new(1)
            .map_err(|_| FleetError::Invariant("invalid protocol version"))?,
        command_id,
        operation_id,
        idempotency_key: IdempotencyKey::parse(format!("prepare:{placement_id}"))
            .map_err(|_| FleetError::Invariant("invalid command idempotency key"))?,
        host_id,
        host_epoch,
        control_plane_incarnation: incarnation,
        authority_session_id,
        scope: CommandScope::Placement {
            placement_id,
            backend_id,
            generation,
        },
        expected_revision: profile_revision,
        issued_at: WireTimestamp::from_millis(issued_at)
            .map_err(|_| FleetError::Invariant("invalid command issue timestamp"))?,
        deadline_at: WireTimestamp::from_millis(deadline_at)
            .map_err(|_| FleetError::Invariant("invalid command deadline"))?,
        request_hash: String::new(),
        operation,
    })
}

fn effective_configuration(candidate: &Candidate) -> Result<ExecutionConfiguration, FleetError> {
    let profile: OperatorConfiguration = serde_json::from_str(&candidate.config_json)
        .map_err(|_| FleetError::Invariant("invalid profile configuration"))?;
    let session_id = if candidate.requires_interactive {
        Some(
            candidate
                .session_id
                .as_deref()
                .ok_or(FleetError::Invariant("interactive session disappeared"))?
                .parse()
                .map_err(|_| FleetError::Invariant("invalid interactive session ID"))?,
        )
    } else {
        None
    };
    Ok(ExecutionConfiguration {
        network: profile.network,
        session_id,
        requires_interactive: candidate.requires_interactive,
        requires_docker_socket: candidate.requires_docker_socket,
    })
}

async fn target_from_db(
    tx: &mut Transaction<'_, Sqlite>,
    target_id: &str,
) -> Result<CiTarget, FleetError> {
    let Some(row) = sqlx::query(
        "SELECT kind,installation_id,repository_id,organization_id,bitbucket_connection_id,
                workspace_uuid,repository_uuid
           FROM fleet_ci_targets WHERE id=?",
    )
    .bind(target_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::NotFound);
    };
    let kind: String = row.get("kind");
    match kind.as_str() {
        "github_repository" => Ok(CiTarget::GithubRepository {
            installation_id: ExternalId::try_from(
                u64::try_from(row.get::<i64, _>("installation_id"))
                    .map_err(|_| FleetError::Invariant("invalid target installation ID"))?,
            )
            .map_err(|_| FleetError::Invariant("invalid target installation ID"))?,
            repository_id: ExternalId::try_from(
                u64::try_from(row.get::<i64, _>("repository_id"))
                    .map_err(|_| FleetError::Invariant("invalid target repository ID"))?,
            )
            .map_err(|_| FleetError::Invariant("invalid target repository ID"))?,
        }),
        "github_organization" => Ok(CiTarget::GithubOrganization {
            installation_id: ExternalId::try_from(
                u64::try_from(row.get::<i64, _>("installation_id"))
                    .map_err(|_| FleetError::Invariant("invalid target installation ID"))?,
            )
            .map_err(|_| FleetError::Invariant("invalid target installation ID"))?,
            organization_id: ExternalId::try_from(
                u64::try_from(row.get::<i64, _>("organization_id"))
                    .map_err(|_| FleetError::Invariant("invalid target organization ID"))?,
            )
            .map_err(|_| FleetError::Invariant("invalid target organization ID"))?,
        }),
        "bitbucket_workspace" | "bitbucket_repository" => {
            let connection_id =
                BitbucketConnectionId::new(row.get::<String, _>("bitbucket_connection_id"))
                    .map_err(|_| FleetError::Invariant("invalid Bitbucket connection ID"))?;
            let workspace_uuid = uuid::Uuid::parse_str(
                row.get::<String, _>("workspace_uuid")
                    .trim_matches(['{', '}']),
            )
            .map_err(|_| FleetError::Invariant("invalid Bitbucket workspace UUID"))?;
            let workspace_uuid = NonNilTargetUuid::new(workspace_uuid)
                .map_err(|_| FleetError::Invariant("nil Bitbucket workspace UUID"))?;
            if kind == "bitbucket_workspace" {
                Ok(CiTarget::BitbucketWorkspace {
                    connection_id,
                    workspace_uuid,
                })
            } else {
                let repository_uuid = uuid::Uuid::parse_str(
                    row.get::<String, _>("repository_uuid")
                        .trim_matches(['{', '}']),
                )
                .map_err(|_| FleetError::Invariant("invalid Bitbucket repository UUID"))?;
                Ok(CiTarget::BitbucketRepository {
                    connection_id,
                    workspace_uuid,
                    repository_uuid: NonNilTargetUuid::new(repository_uuid)
                        .map_err(|_| FleetError::Invariant("nil Bitbucket repository UUID"))?,
                })
            }
        }
        _ => Err(FleetError::Invariant("invalid target kind")),
    }
}

fn parse_runtime_kind(value: &str) -> Result<RuntimeKind, FleetError> {
    match value {
        "docker" => Ok(RuntimeKind::Docker),
        "tart_vm" => Ok(RuntimeKind::TartVm),
        "native_process" => Ok(RuntimeKind::NativeProcess),
        _ => Err(FleetError::Invariant("invalid runtime kind")),
    }
}

fn parse_execution_os(value: &str) -> Result<ExecutionOs, FleetError> {
    match value {
        "linux" => Ok(ExecutionOs::Linux),
        "macos" => Ok(ExecutionOs::Macos),
        "windows" => Ok(ExecutionOs::Windows),
        _ => Err(FleetError::Invariant("invalid execution OS")),
    }
}

fn parse_architecture(value: &str) -> Result<Architecture, FleetError> {
    match value {
        "x64" => Ok(Architecture::X64),
        "arm64" => Ok(Architecture::Arm64),
        _ => Err(FleetError::Invariant("invalid architecture")),
    }
}

fn parse_id<T>(value: &str) -> Result<T, FleetError>
where
    T: std::str::FromStr<Err = FleetIdParseError>,
{
    value
        .parse::<T>()
        .map_err(|error| FleetError::InvalidIdentifier(error.to_string()))
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod admission_tests;

fn new_id<T>() -> Result<T, FleetError>
where
    T: TryFrom<Uuid, Error = super::ids::NilFleetUuid>,
{
    T::try_from(Uuid::new_v4()).map_err(|_| FleetError::Invariant("generated nil UUID"))
}

trait ResourceAmountExt {
    fn fits_request(self, request: ResourceAmount) -> bool;
}

impl ResourceAmountExt for ResourceAmount {
    fn fits_request(self, request: ResourceAmount) -> bool {
        self.cpu_millis() >= request.cpu_millis()
            && self.memory_mib() >= request.memory_mib()
            && self.disk_bytes() >= request.disk_bytes()
    }
}
