//! Fleet application service: scoped intent submission and operation reads.

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row as _, SqlitePool};
use thiserror::Error;
use uuid::Uuid;

use super::{
    authorization::Principal,
    ids::{CiTargetId, FleetIdParseError, Generation, OperationId, ProfileId, WorkloadId},
    protocol::primitives::IdempotencyKey,
    store,
};

/// The kind of durable workload requested by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadKind {
    CiRunner,
    FixSandbox,
}

impl WorkloadKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CiRunner => "ci_runner",
            Self::FixSandbox => "fix_sandbox",
        }
    }
}

/// Client input for a workload intent. Resources and host ownership are
/// loaded from the profile and host rows inside the service transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitIntent {
    pub idempotency_key: String,
    pub pool_id: String,
    pub profile_id: ProfileId,
    pub target_id: CiTargetId,
    pub workload_id: WorkloadId,
    pub generation: Generation,
    pub kind: WorkloadKind,
    pub agent_run_id: Option<String>,
    pub expected_profile_revision: Option<u64>,
}

/// Result of accepting an intent. A replay keeps the original operation ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionState {
    Queued,
    Replay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submission {
    pub operation_id: OperationId,
    pub workload_id: WorkloadId,
    pub generation: Generation,
    pub state: SubmissionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationStatus {
    pub operation_id: OperationId,
    pub principal_scope: String,
    pub status: String,
    pub reason_code: Option<String>,
    pub workload_id: Option<WorkloadId>,
    pub generation: Option<Generation>,
}

#[derive(Debug, Error)]
pub enum FleetError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("invalid fleet identifier: {0}")]
    InvalidIdentifier(String),
    #[error("invalid fleet request: {0}")]
    InvalidRequest(&'static str),
    #[error("fleet operation is not authorized")]
    Forbidden,
    #[error("fleet operation was not found")]
    NotFound,
    #[error("idempotency key was reused with a different request")]
    IdempotencyConflict,
    #[error("profile revision is stale")]
    StaleProfileRevision,
    #[error("fleet admission invariant failed: {0}")]
    Invariant(&'static str),
    #[error("fleet request remains queued: {0}")]
    Queued(&'static str),
    #[error("JSON encoding failed")]
    Json(#[from] serde_json::Error),
}

impl From<FleetIdParseError> for FleetError {
    fn from(value: FleetIdParseError) -> Self {
        Self::InvalidIdentifier(value.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct FleetService {
    pool: SqlitePool,
}

impl FleetService {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn submit_intent(
        &self,
        principal: &Principal,
        request: SubmitIntent,
    ) -> Result<Submission, FleetError> {
        validate_request(&request)?;
        let identity = PrincipalIdentity::from_principal(principal)?;
        let request_hash = request_hash(&request)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        if let Some(existing) =
            store::operation_by_key(&mut tx, &identity.scope, &request.idempotency_key).await?
        {
            if existing.request_hash != request_hash {
                return Err(FleetError::IdempotencyConflict);
            }
            let Some(intent) = store::intent_for_operation(&mut tx, &existing.id).await? else {
                return Err(FleetError::NotFound);
            };
            // Replay is idempotent with respect to the original request, but
            // authorization is always evaluated against current policy.
            ensure_operation_access(&mut tx, principal, &intent).await?;
            let operation_id = parse_id::<OperationId>(&existing.id)?;
            let result = Submission {
                operation_id,
                workload_id: request.workload_id,
                generation: request.generation,
                state: SubmissionState::Replay,
            };
            tx.commit().await?;
            return Ok(result);
        }

        ensure_profile_and_target(&mut tx, &request).await?;
        ensure_target_identity(&mut tx, &request).await?;
        ensure_pool_access(&mut tx, principal, &request.pool_id).await?;
        ensure_target_access(&mut tx, principal, &request.target_id.to_string()).await?;
        ensure_workload_policy(&mut tx, principal, &request).await?;

        let duplicate_generation: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workload_intents WHERE workload_id=? AND generation=?",
        )
        .bind(request.workload_id.to_string())
        .bind(
            i64::try_from(request.generation.get())
                .map_err(|_| FleetError::InvalidRequest("generation exceeds SQLite integer"))?,
        )
        .fetch_one(&mut *tx)
        .await?;
        if duplicate_generation != 0 {
            return Err(FleetError::IdempotencyConflict);
        }

        let operation_id = new_id::<OperationId>()?;
        let now = crate::db::now_millis();
        sqlx::query(
            "INSERT INTO fleet_operations
             (id,kind,requested_by_user_id,principal_kind,principal_scope,idempotency_key,
              request_hash,status,created_at,updated_at)
             VALUES (?,?,?,?,?,?,?,'queued',?,?)",
        )
        .bind(operation_id.to_string())
        .bind("submit_workload")
        .bind(identity.user_id.as_deref())
        .bind(identity.kind)
        .bind(&identity.scope)
        .bind(&request.idempotency_key)
        .bind(&request_hash)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO workload_intents
             (id,workload_id,generation,kind,pool_id,profile_id,target_id,agent_run_id,
              expected_profile_revision,status,created_at,updated_at)
             SELECT ?,?,?,?,?,?,?,?,revision,'queued',?,?
               FROM pool_execution_profiles WHERE id = ? AND pool_id = ?",
        )
        .bind(operation_id.to_string())
        .bind(request.workload_id.to_string())
        .bind(
            i64::try_from(request.generation.get())
                .map_err(|_| FleetError::InvalidRequest("generation exceeds SQLite integer"))?,
        )
        .bind(request.kind.as_str())
        .bind(&request.pool_id)
        .bind(request.profile_id.to_string())
        .bind(request.target_id.to_string())
        .bind(request.agent_run_id.as_deref())
        .bind(now)
        .bind(now)
        .bind(request.profile_id.to_string())
        .bind(&request.pool_id)
        .execute(&mut *tx)
        .await?;

        let inserted: i64 = sqlx::query_scalar("SELECT changes()")
            .fetch_one(&mut *tx)
            .await?;
        if inserted != 1 {
            return Err(FleetError::Invariant(
                "profile disappeared during intent insert",
            ));
        }
        tx.commit().await?;
        Ok(Submission {
            operation_id,
            workload_id: request.workload_id,
            generation: request.generation,
            state: SubmissionState::Queued,
        })
    }

    pub async fn load_operation(
        &self,
        principal: &Principal,
        operation_id: OperationId,
    ) -> Result<OperationStatus, FleetError> {
        let mut tx = self.pool.begin().await?;
        let Some(operation) = store::operation_by_id(&mut tx, &operation_id.to_string()).await?
        else {
            return Err(FleetError::NotFound);
        };
        let Some(intent) = store::intent_for_operation(&mut tx, &operation.id).await? else {
            return Err(FleetError::NotFound);
        };
        // The original principal scope is an audit attribute, not a lasting
        // capability.  Current resource authorization controls status reads.
        ensure_operation_access(&mut tx, principal, &intent).await?;
        let workload_id = parse_id::<WorkloadId>(&intent.workload_id)?;
        let generation = Generation::new(
            u64::try_from(intent.generation)
                .map_err(|_| FleetError::Invariant("invalid stored workload generation"))?,
        )
        .map_err(|_| FleetError::Invariant("invalid stored workload generation"))?;
        tx.commit().await?;
        Ok(OperationStatus {
            operation_id,
            principal_scope: operation.principal_scope,
            status: operation.status,
            reason_code: operation.reason_code,
            workload_id: Some(workload_id),
            generation: Some(generation),
        })
    }

    #[cfg(test)]
    pub(crate) async fn run_scheduling_slice(
        &self,
        controller: &Principal,
        limit: super::admission::ScanLimit,
    ) -> Result<super::admission::SliceReport, FleetError> {
        super::admission::run_slice(&self.pool, controller, limit).await
    }
}

pub(crate) async fn recheck_intent_authorization(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    controller: &Principal,
    intent: &store::IntentRow,
) -> Result<(), FleetError> {
    if !matches!(
        controller,
        Principal::Reconciler | Principal::Autoscaler | Principal::FixAgent
    ) {
        return Err(FleetError::Forbidden);
    }
    let Some(row) =
        sqlx::query("SELECT principal_kind,requested_by_user_id FROM fleet_operations WHERE id=?")
            .bind(&intent.id)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Err(FleetError::NotFound);
    };
    let principal_kind: String = row.get("principal_kind");
    let requested_by: Option<String> = row.get("requested_by_user_id");
    let principal = match principal_kind.as_str() {
        "user" => Principal::AuthenticatedUser {
            user_id: requested_by.ok_or(FleetError::Forbidden)?,
        },
        "reconciler" => Principal::Reconciler,
        "autoscaler" => Principal::Autoscaler,
        "fix_agent" => Principal::FixAgent,
        _ => return Err(FleetError::Invariant("invalid operation principal")),
    };
    ensure_operation_access(tx, &principal, intent).await?;
    ensure_current_intent_policy(tx, intent, true, &principal).await
}

pub(crate) async fn ensure_operation_access(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    principal: &Principal,
    intent: &store::IntentRow,
) -> Result<(), FleetError> {
    ensure_pool_access(tx, principal, &intent.pool_id).await?;
    ensure_target_access(tx, principal, &intent.target_id).await?;
    ensure_current_intent_policy(tx, intent, false, principal).await?;

    let Some(placement) = sqlx::query("SELECT host_id FROM workload_placements WHERE intent_id=?")
        .bind(&intent.id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(());
    };
    let host_id: String = placement.get("host_id");
    let target_grant: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM host_target_grants
          WHERE host_id=? AND target_id=? AND allow_schedule=1 AND revoked_at IS NULL",
    )
    .bind(&host_id)
    .bind(&intent.target_id)
    .fetch_one(&mut **tx)
    .await?;
    if target_grant != 1 {
        return Err(FleetError::Forbidden);
    }
    ensure_target_identity_for_pool(tx, &intent.pool_id, &intent.target_id).await?;
    if let Principal::AuthenticatedUser { user_id } = principal {
        let user_grant: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM host_user_grants
              WHERE host_id=? AND user_id=? AND permission IN ('read','operator','admin')
                AND revoked_at IS NULL",
        )
        .bind(&host_id)
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await?;
        let system_admin: Option<String> = sqlx::query_scalar("SELECT role FROM users WHERE id=?")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
        if user_grant != 1 && system_admin.as_deref() != Some("admin") {
            return Err(FleetError::Forbidden);
        }
    }
    Ok(())
}

pub(crate) async fn ensure_current_intent_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    intent: &store::IntentRow,
    admission: bool,
    principal: &Principal,
) -> Result<(), FleetError> {
    ensure_target_identity_for_pool(tx, &intent.pool_id, &intent.target_id).await?;
    let Some((enabled, revision)): Option<(i64, i64)> = sqlx::query_as(
        "SELECT enabled,revision FROM pool_execution_profiles WHERE id=? AND pool_id=?",
    )
    .bind(&intent.profile_id)
    .bind(&intent.pool_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::NotFound);
    };
    if enabled == 0 {
        return Err(FleetError::Forbidden);
    }
    if admission && revision != intent.expected_profile_revision {
        return Err(FleetError::StaleProfileRevision);
    }
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM profile_ci_targets
          WHERE profile_id=? AND target_id=? AND enabled=1 AND revoked_at IS NULL",
    )
    .bind(&intent.profile_id)
    .bind(&intent.target_id)
    .fetch_one(&mut **tx)
    .await?;
    if linked != 1 {
        return Err(FleetError::Forbidden);
    }
    if intent.kind == WorkloadKind::FixSandbox.as_str() {
        ensure_fix_run_policy(tx, intent, principal, admission).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct PrincipalIdentity {
    kind: &'static str,
    scope: String,
    user_id: Option<String>,
}

impl PrincipalIdentity {
    fn from_principal(principal: &Principal) -> Result<Self, FleetError> {
        match principal {
            Principal::AuthenticatedUser { user_id } if !user_id.is_empty() => Ok(Self {
                kind: "user",
                scope: format!("user:{user_id}"),
                user_id: Some(user_id.clone()),
            }),
            Principal::Reconciler => Ok(Self {
                kind: "reconciler",
                scope: "controller:reconciler".to_owned(),
                user_id: None,
            }),
            Principal::Autoscaler => Ok(Self {
                kind: "autoscaler",
                scope: "controller:autoscaler".to_owned(),
                user_id: None,
            }),
            Principal::FixAgent => Ok(Self {
                kind: "fix_agent",
                scope: "controller:fix_agent".to_owned(),
                user_id: None,
            }),
            Principal::AuthenticatedUser { .. } => Err(FleetError::InvalidRequest(
                "user identity must not be empty",
            )),
        }
    }
}

fn validate_request(request: &SubmitIntent) -> Result<(), FleetError> {
    IdempotencyKey::parse(request.idempotency_key.clone())
        .map_err(|_| FleetError::InvalidRequest("invalid idempotency key"))?;
    if request.pool_id.is_empty() {
        return Err(FleetError::InvalidRequest("pool ID must not be empty"));
    }
    if request.generation.get() != 1 {
        return Err(FleetError::InvalidRequest(
            "only initial workload generation is supported",
        ));
    }
    if request.expected_profile_revision == Some(0) {
        return Err(FleetError::InvalidRequest(
            "profile revision must be positive",
        ));
    }
    match (request.kind, request.agent_run_id.as_deref()) {
        (WorkloadKind::CiRunner, None) => Ok(()),
        (WorkloadKind::CiRunner, Some(_)) => Err(FleetError::InvalidRequest(
            "CI runner intent cannot carry an agent run",
        )),
        (WorkloadKind::FixSandbox, Some(value)) if !value.is_empty() => Ok(()),
        (WorkloadKind::FixSandbox, _) => Err(FleetError::InvalidRequest(
            "fix sandbox requires an agent run",
        )),
    }
}

async fn ensure_workload_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    principal: &Principal,
    request: &SubmitIntent,
) -> Result<(), FleetError> {
    if request.kind != WorkloadKind::FixSandbox {
        return Ok(());
    }
    let Some((runtime_kind, execution_os)): Option<(String, String)> = sqlx::query_as(
        "SELECT runtime_kind,execution_os FROM pool_execution_profiles WHERE id=? AND pool_id=?",
    )
    .bind(request.profile_id.to_string())
    .bind(&request.pool_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::NotFound);
    };
    if runtime_kind != "docker" || execution_os != "linux" {
        return Err(FleetError::Forbidden);
    }
    let Some(target_kind): Option<String> =
        sqlx::query_scalar("SELECT kind FROM fleet_ci_targets WHERE id=?")
            .bind(request.target_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Err(FleetError::NotFound);
    };
    if target_kind != "github_repository" {
        return Err(FleetError::Forbidden);
    }
    ensure_fix_run_row_policy(
        tx,
        &request.pool_id,
        &request.target_id.to_string(),
        request
            .agent_run_id
            .as_deref()
            .ok_or(FleetError::Forbidden)?,
        principal,
        None,
    )
    .await
}

async fn ensure_fix_run_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    intent: &store::IntentRow,
    principal: &Principal,
    admission: bool,
) -> Result<(), FleetError> {
    let Some((runtime_kind, execution_os, target_kind)): Option<(String, String, String)> =
        sqlx::query_as(
            "SELECT p.runtime_kind,p.execution_os,t.kind
               FROM pool_execution_profiles p JOIN fleet_ci_targets t ON t.id=?
              WHERE p.id=? AND p.pool_id=?",
        )
        .bind(&intent.target_id)
        .bind(&intent.profile_id)
        .bind(&intent.pool_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Err(FleetError::NotFound);
    };
    if runtime_kind != "docker" || execution_os != "linux" || target_kind != "github_repository" {
        return Err(FleetError::Forbidden);
    }
    ensure_fix_run_row_policy(
        tx,
        &intent.pool_id,
        &intent.target_id,
        intent
            .agent_run_id
            .as_deref()
            .ok_or(FleetError::Forbidden)?,
        principal,
        Some((&intent.id, admission)),
    )
    .await?;
    Ok(())
}

async fn ensure_fix_run_row_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    _pool_id: &str,
    target_id: &str,
    agent_run_id: &str,
    principal: &Principal,
    current: Option<(&str, bool)>,
) -> Result<(), FleetError> {
    let Some(row) = sqlx::query(
        "SELECT ar.status,ar.cancel_requested,ar.requested_by,
                ar.repository_id AS run_repository_id,
                t.repository_id AS target_repository_id
           FROM agent_runs ar JOIN fleet_ci_targets t ON t.id=?
          WHERE ar.id=?",
    )
    .bind(target_id)
    .bind(agent_run_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::Forbidden);
    };
    let status: String = row.get("status");
    let cancelled: i64 = row.get("cancel_requested");
    let requested_by: String = row.get("requested_by");
    let run_repository: i64 = row.get("run_repository_id");
    let target_repository: Option<i64> = row.get("target_repository_id");
    if Some(run_repository) != target_repository {
        return Err(FleetError::Forbidden);
    }
    if let Principal::AuthenticatedUser { user_id } = principal {
        if requested_by != *user_id {
            return Err(FleetError::Forbidden);
        }
    }
    let has_current = current.is_some();
    let (current_id, admission) = current.unwrap_or(("", true));
    if admission {
        if !matches!(status.as_str(), "queued" | "running") || cancelled != 0 {
            return Err(FleetError::Forbidden);
        }
        let duplicate: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workload_intents
              WHERE agent_run_id=? AND status IN ('queued','admitted') AND id != ?",
        )
        .bind(agent_run_id)
        .bind(current_id)
        .fetch_one(&mut **tx)
        .await?;
        if duplicate != 0 {
            return Err(FleetError::IdempotencyConflict);
        }
        if has_current && current_id.is_empty() {
            return Err(FleetError::Invariant("fix intent lacks operation identity"));
        }
    }
    Ok(())
}

async fn ensure_profile_and_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &SubmitIntent,
) -> Result<(), FleetError> {
    let Some((revision, enabled)): Option<(i64, i64)> = sqlx::query_as(
        "SELECT revision,enabled FROM pool_execution_profiles WHERE id = ? AND pool_id = ?",
    )
    .bind(request.profile_id.to_string())
    .bind(&request.pool_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::NotFound);
    };
    if enabled == 0 {
        return Err(FleetError::Forbidden);
    }
    if let Some(expected) = request.expected_profile_revision {
        if i64::try_from(expected).ok() != Some(revision) {
            return Err(FleetError::StaleProfileRevision);
        }
    }
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM profile_ci_targets
          WHERE profile_id = ? AND target_id = ? AND enabled=1 AND revoked_at IS NULL",
    )
    .bind(request.profile_id.to_string())
    .bind(request.target_id.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if linked != 1 {
        return Err(FleetError::Forbidden);
    }
    Ok(())
}

async fn ensure_target_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    request: &SubmitIntent,
) -> Result<(), FleetError> {
    ensure_target_identity_for_pool(tx, &request.pool_id, &request.target_id.to_string()).await
}

pub(crate) async fn ensure_target_identity_for_pool(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    pool_id: &str,
    target_id: &str,
) -> Result<(), FleetError> {
    let Some(kind): Option<String> =
        sqlx::query_scalar("SELECT kind FROM fleet_ci_targets WHERE id=?")
            .bind(target_id)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Err(FleetError::NotFound);
    };
    let valid = match kind.as_str() {
        "github_repository" => {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM fleet_ci_targets t
                  JOIN repositories r ON r.id=t.repository_id
                  JOIN installations ins ON ins.id=t.installation_id
                  WHERE t.id=? AND r.installation_id=t.installation_id
                   AND EXISTS (
                     SELECT 1 FROM runner_pools p
                      WHERE p.id=? AND (
                        (p.repository_id=r.id AND p.installation_id=t.installation_id)
                        OR EXISTS (
                          SELECT 1 FROM runner_pool_repositories pr
                           WHERE pr.pool_id=p.id AND pr.repository_id=r.id
                        )
                      )
                   )",
            )
            .bind(target_id)
            .bind(pool_id)
            .fetch_one(&mut **tx)
            .await?
                != 0
        }
        "github_organization" => {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM fleet_ci_targets t
                  JOIN installations ins ON ins.id=t.installation_id
                 WHERE t.id=? AND ins.account_id=t.organization_id
                   AND EXISTS (
                     SELECT 1 FROM runner_pool_installations rpi
                      WHERE rpi.pool_id=? AND rpi.installation_id=t.installation_id
                   )",
            )
            .bind(target_id)
            .bind(pool_id)
            .fetch_one(&mut **tx)
            .await?
                != 0
        }
        "bitbucket_workspace" | "bitbucket_repository" => {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM fleet_ci_targets t
                  JOIN bitbucket_connections bc ON bc.id=t.bitbucket_connection_id
                 WHERE t.id=? AND EXISTS (
                   SELECT 1 FROM runner_pool_bitbucket_connections rpc
                    WHERE rpc.pool_id=? AND rpc.connection_id=bc.id
                 )
                   AND lower(trim(bc.workspace_uuid,'{}'))=lower(trim(t.workspace_uuid,'{}'))",
            )
            .bind(target_id)
            .bind(pool_id)
            .fetch_one(&mut **tx)
            .await?
                != 0
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(FleetError::Forbidden)
    }
}

async fn ensure_pool_access(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    principal: &Principal,
    pool_id: &str,
) -> Result<(), FleetError> {
    let Some(user_id) = (match principal {
        Principal::AuthenticatedUser { user_id } => Some(user_id.as_str()),
        Principal::Reconciler | Principal::Autoscaler | Principal::FixAgent => None,
    }) else {
        return Ok(());
    };
    let Some(role) = sqlx::query_scalar::<_, String>("SELECT role FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Err(FleetError::Forbidden);
    };
    let inaccessible_installation: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runner_pool_installations rpi
          JOIN installations ins ON ins.id=rpi.installation_id
          LEFT JOIN user_installations ui
            ON ui.user_id=? AND ui.installation_id=rpi.installation_id
         WHERE rpi.pool_id=? AND (
           ins.suspended_at IS NOT NULL OR ui.installation_id IS NULL OR
           (? <> 'admin' AND ui.permission <> 'admin')
         )",
    )
    .bind(user_id)
    .bind(pool_id)
    .bind(&role)
    .fetch_one(&mut **tx)
    .await?;
    let installation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runner_pool_installations WHERE pool_id=?")
            .bind(pool_id)
            .fetch_one(&mut **tx)
            .await?;
    if installation_count == 0 || inaccessible_installation != 0 {
        return Err(FleetError::Forbidden);
    }
    let missing_connection: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runner_pool_bitbucket_connections rpc
          WHERE rpc.pool_id = ? AND NOT EXISTS (
            SELECT 1 FROM bitbucket_user_grants bug
             WHERE bug.connection_id = rpc.connection_id AND bug.user_id = ?
               AND bug.revoked_at IS NULL
               AND (bug.permission='admin' OR ?='admin')
          )",
    )
    .bind(pool_id)
    .bind(user_id)
    .bind(&role)
    .fetch_one(&mut **tx)
    .await?;
    if missing_connection != 0 {
        return Err(FleetError::Forbidden);
    }
    Ok(())
}

async fn ensure_target_access(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    principal: &Principal,
    target_id: &str,
) -> Result<(), FleetError> {
    let user_id = match principal {
        Principal::AuthenticatedUser { user_id } => Some(user_id.as_str()),
        Principal::Reconciler | Principal::Autoscaler | Principal::FixAgent => None,
    };
    let role = if let Some(user_id) = user_id {
        sqlx::query_scalar::<_, String>("SELECT role FROM users WHERE id=?")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(FleetError::Forbidden)?
    } else {
        String::new()
    };
    let Some(row) = sqlx::query(
        "SELECT kind,installation_id,bitbucket_connection_id
           FROM fleet_ci_targets WHERE id=?",
    )
    .bind(target_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Err(FleetError::NotFound);
    };
    let kind: String = row.get("kind");
    let allowed = if kind.starts_with("github_") {
        let installation_id: i64 = row.get("installation_id");
        let suspended: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM installations WHERE id=? AND suspended_at IS NOT NULL",
        )
        .bind(installation_id)
        .fetch_one(&mut **tx)
        .await?;
        if suspended != 0 {
            return Err(FleetError::Forbidden);
        }
        match user_id {
            Some(user_id) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM user_installations ui
                      WHERE ui.user_id=? AND ui.installation_id=?
                        AND (ui.permission='admin' OR ?='admin')",
                )
                .bind(user_id)
                .bind(installation_id)
                .bind(&role)
                .fetch_one(&mut **tx)
                .await?
                    != 0
            }
            None => true,
        }
    } else {
        let connection_id: String = row.get("bitbucket_connection_id");
        match user_id {
            Some(user_id) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM bitbucket_user_grants
                      WHERE user_id=? AND connection_id=? AND revoked_at IS NULL
                        AND (permission='admin' OR ?='admin')",
                )
                .bind(user_id)
                .bind(connection_id)
                .bind(&role)
                .fetch_one(&mut **tx)
                .await?
                    != 0
            }
            None => true,
        }
    };
    if !allowed {
        return Err(FleetError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
struct CanonicalRequest<'a> {
    pool_id: &'a str,
    profile_id: String,
    target_id: String,
    workload_id: String,
    generation: u64,
    kind: &'static str,
    agent_run_id: Option<&'a str>,
    expected_profile_revision: Option<u64>,
}

fn request_hash(request: &SubmitIntent) -> Result<String, FleetError> {
    let canonical = CanonicalRequest {
        pool_id: &request.pool_id,
        profile_id: request.profile_id.to_string(),
        target_id: request.target_id.to_string(),
        workload_id: request.workload_id.to_string(),
        generation: request.generation.get(),
        kind: request.kind.as_str(),
        agent_run_id: request.agent_run_id.as_deref(),
        expected_profile_revision: request.expected_profile_revision,
    };
    let bytes = serde_json::to_vec(&canonical)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn new_id<T>() -> Result<T, FleetError>
where
    T: TryFrom<Uuid, Error = super::ids::NilFleetUuid>,
{
    T::try_from(Uuid::new_v4()).map_err(|error| FleetError::InvalidIdentifier(error.to_string()))
}

fn parse_id<T>(value: &str) -> Result<T, FleetError>
where
    T: std::str::FromStr<Err = FleetIdParseError>,
{
    value.parse().map_err(FleetError::from)
}
