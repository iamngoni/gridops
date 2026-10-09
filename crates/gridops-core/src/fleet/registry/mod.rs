//! Durable fleet registry and enrollment authority.
//!
//! This module owns single-use enrollment, verifier-only host authentication,
//! manual approval scope checks, explicit recovery, and credential rotation.
//! Inventory ingestion and HTTP transport remain outside this boundary. Every
//! state transition rechecks authorization inside a short `BEGIN IMMEDIATE`
//! transaction and records an audit/event pair before commit.

mod credentials;
mod enrollment;
mod handshake;
mod inventory;
#[cfg(test)]
mod inventory_tests;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use std::{fmt, sync::Arc};

use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row as _, Sqlite, SqlitePool, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::{Vault, db::now_millis};

use super::{
    authorization::Principal,
    domain::{Architecture, ExecutionOs},
    ids::{CiTargetId, Generation, HostEpoch, HostId},
    protocol::primitives::IdempotencyKey,
};

pub use credentials::{CredentialError, OneTimeSecret};
pub use enrollment::{
    ApproveHost, EnrollmentInputError, EnrollmentIssue, EnrollmentMetadata, EnrollmentReceipt,
    EnrollmentScope, HostRegistration, IssueEnrollment, IssuedEnrollment, IssuedRecoveryEnrollment,
    RecoverHost, RecoveryIssue,
};
pub use inventory::InventoryError;

use credentials::{Verifier, validate_secret_for_authentication};
use enrollment::{ROTATION_OVERLAP_MILLIS, ROTATION_TTL_MILLIS, require_user};

/// Trusted server time source. It is intentionally not constructible from a
/// request and exists to make expiry and overlap boundaries deterministic.
pub trait RegistryClock: Send + Sync {
    fn now_millis(&self) -> i64;
}

#[derive(Debug, Default)]
struct SystemClock;

impl RegistryClock for SystemClock {
    fn now_millis(&self) -> i64 {
        now_millis()
    }
}

/// A host authenticated against a current verifier and epoch.
#[derive(Clone)]
pub struct AuthenticatedHost {
    host_id: HostId,
    credential_id: Uuid,
    generation: Generation,
    epoch: HostEpoch,
}

impl AuthenticatedHost {
    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    #[must_use]
    pub const fn credential_id(&self) -> Uuid {
        self.credential_id
    }

    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    #[must_use]
    pub const fn epoch(&self) -> HostEpoch {
        self.epoch
    }
}

impl fmt::Debug for AuthenticatedHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedHost")
            .field("host_id", &self.host_id)
            .field("credential_id", &self.credential_id)
            .field("generation", &self.generation)
            .field("epoch", &self.epoch)
            .finish()
    }
}

/// Metadata returned when a durable rotation request is created or replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationRequestReceipt {
    pub operation_id: Uuid,
    pub host_id: HostId,
    pub old_generation: Generation,
    pub next_generation: Generation,
    pub expires_at: i64,
}

/// Metadata returned after a host exchanges its securely generated next secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationExchangeReceipt {
    pub operation_id: Uuid,
    pub credential_id: Uuid,
    pub generation: Generation,
    pub overlap_expires_at: i64,
}

/// Metadata returned after the new generation acknowledges rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationAckReceipt {
    pub operation_id: Uuid,
    pub host_id: HostId,
    pub generation: Generation,
    pub acknowledged_at: i64,
}

/// Durable, secret-free rotation operation projection for `/operations/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationOperationState {
    Requested,
    Exchanged,
    Acknowledged,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationOperationStatus {
    pub operation_id: Uuid,
    pub host_id: HostId,
    pub state: RotationOperationState,
    pub old_generation: Generation,
    pub next_generation: Generation,
    pub expires_at: i64,
    pub updated_at: i64,
}

/// Request to begin a host-generated credential rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRotation {
    pub host_id: HostId,
    pub expected_generation: Generation,
    pub idempotency_key: String,
}

/// Stable domain failures. Secret material is never included in an error.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("registry operation is not authorized")]
    Forbidden,
    #[error("registry record was not found")]
    NotFound,
    #[error("enrollment code is expired, revoked, or already used")]
    EnrollmentGone,
    #[error("enrollment code does not match the requested host")]
    EnrollmentScopeMismatch,
    #[error("registry revision is stale")]
    StaleRevision,
    #[error("credential is invalid, revoked, or expired")]
    InvalidCredential,
    #[error("credential rotation conflicts with an existing exchange")]
    RotationConflict,
    #[error("idempotency key was reused with a different request")]
    IdempotencyConflict,
    #[error("credential rotation is expired or revoked")]
    RotationGone,
    #[error("credential generation is stale")]
    StaleGeneration,
    #[error("registry request is invalid: {0}")]
    InvalidRequest(&'static str),
    #[error("invalid fleet identifier: {0}")]
    InvalidIdentifier(String),
    #[error("target scope is not administered by the requesting user")]
    TargetScopeForbidden,
    #[error("JSON encoding failed")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Input(#[from] EnrollmentInputError),
}

impl From<super::ids::FleetIdParseError> for RegistryError {
    fn from(value: super::ids::FleetIdParseError) -> Self {
        Self::InvalidIdentifier(value.to_string())
    }
}

/// Clone-cheap database/vault handles for registry workflows.
#[derive(Clone)]
pub struct RegistryService {
    pool: SqlitePool,
    vault: Vault,
    clock: Arc<dyn RegistryClock>,
}

impl fmt::Debug for RegistryService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RegistryService(REDACTED_HANDLES)")
    }
}

impl RegistryService {
    #[must_use]
    pub fn new(pool: SqlitePool, vault: Vault) -> Self {
        Self {
            pool,
            vault,
            clock: Arc::new(SystemClock),
        }
    }

    /// Construct a service with trusted deterministic time for expiry tests.
    #[must_use]
    pub fn with_clock(pool: SqlitePool, vault: Vault, clock: Arc<dyn RegistryClock>) -> Self {
        Self { pool, vault, clock }
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// Issue a one-time code after rechecking current system-admin status.
    pub async fn issue_enrollment(
        &self,
        principal: &Principal,
        request: IssueEnrollment,
    ) -> Result<EnrollmentIssue, RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        validate_idempotency_key(&request.idempotency_key)?;
        let ttl = request.ttl_millis()?;
        let scope_json = serde_json::to_string(&request.scope)?;
        let request_hash = hash_request(&format!(
            "{}|{}|{}",
            scope_json,
            request
                .intended_host_id
                .map_or_else(String::new, |id| id.to_string()),
            ttl
        ));
        let code = OneTimeSecret::generate();
        let verifier = code.verifier();
        let id = Uuid::new_v4();
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        let expires_at = now.checked_add(ttl).ok_or(RegistryError::InvalidRequest(
            "enrollment lifetime overflowed",
        ))?;
        ensure_system_admin(&mut tx, user_id).await?;
        ensure_scope_admin(&mut tx, user_id, &request.scope).await?;
        if let Some(existing) = sqlx::query(
            "SELECT id,expires_at,consumed_at,revoked_at,consumed_host_id,request_hash
             FROM host_enrollments WHERE issued_by=? AND idempotency_key=?",
        )
        .bind(user_id)
        .bind(&request.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing_hash: String = existing.get("request_hash");
            if existing_hash != request_hash {
                return Err(RegistryError::IdempotencyConflict);
            }
            let consumed_host_id = existing
                .get::<Option<String>, _>("consumed_host_id")
                .map(|value| value.parse::<HostId>())
                .transpose()?;
            let metadata = EnrollmentMetadata {
                id: parse_uuid(&existing.get::<String, _>("id"))?,
                expires_at: existing.get("expires_at"),
                consumed_at: existing.get("consumed_at"),
                revoked_at: existing.get("revoked_at"),
                consumed_host_id,
            };
            tx.commit().await?;
            return Ok(EnrollmentIssue::Replay(metadata));
        }
        if request.intended_host_id.is_some() {
            return Err(RegistryError::InvalidRequest(
                "existing hosts require explicit recovery",
            ));
        }
        sqlx::query(
            "INSERT INTO host_enrollments
             (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,mode,verifier_format,idempotency_key,request_hash)
             VALUES (?,?,?,?,?,?,?,'normal','sha256',?,?)",
        )
        .bind(id.to_string())
        .bind(verifier.as_str())
        .bind(request.intended_host_id.map(|value| value.to_string()))
        .bind(scope_json)
        .bind(user_id)
        .bind(now)
        .bind(expires_at)
        .bind(&request.idempotency_key)
        .bind(&request_hash)
        .execute(&mut *tx)
        .await?;
        audit_and_event(
            &mut tx,
            Some(user_id),
            None,
            "fleet.enrollment.issued",
            "enrollment",
            &id.to_string(),
            json!({"expires_at": expires_at, "scope": request.scope}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(EnrollmentIssue::Created(IssuedEnrollment {
            id,
            code,
            expires_at,
        }))
    }

    /// Revoke a code atomically. A replay never reveals whether it was used.
    pub async fn revoke_enrollment(
        &self,
        principal: &Principal,
        enrollment_id: Uuid,
    ) -> Result<(), RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        ensure_system_admin(&mut tx, user_id).await?;
        let current = sqlx::query("SELECT consumed_at,revoked_at FROM host_enrollments WHERE id=?")
            .bind(enrollment_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(RegistryError::NotFound)?;
        if current.get::<Option<i64>, _>("revoked_at").is_some() {
            tx.commit().await?;
            return Ok(());
        }
        if current.get::<Option<i64>, _>("consumed_at").is_some() {
            return Err(RegistryError::EnrollmentGone);
        }
        let changed = sqlx::query(
            "UPDATE host_enrollments SET revoked_at=?
             WHERE id=? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(enrollment_id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(RegistryError::EnrollmentGone);
        }
        audit_and_event(
            &mut tx,
            Some(user_id),
            None,
            "fleet.enrollment.revoked",
            "enrollment",
            &enrollment_id.to_string(),
            json!({}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Read non-secret enrollment status for response-loss recovery UI.
    pub async fn enrollment_status(
        &self,
        principal: &Principal,
        enrollment_id: Uuid,
    ) -> Result<EnrollmentMetadata, RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        let mut tx = self.begin().await?;
        ensure_system_admin(&mut tx, user_id).await?;
        let Some(row) = sqlx::query(
            "SELECT id,expires_at,consumed_at,revoked_at,consumed_host_id
             FROM host_enrollments WHERE id=?",
        )
        .bind(enrollment_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Err(RegistryError::NotFound);
        };
        let metadata = EnrollmentMetadata {
            id: parse_uuid(&row.get::<String, _>("id"))?,
            expires_at: row.get("expires_at"),
            consumed_at: row.get("consumed_at"),
            revoked_at: row.get("revoked_at"),
            consumed_host_id: row
                .get::<Option<String>, _>("consumed_host_id")
                .map(|value| value.parse::<HostId>())
                .transpose()?,
        };
        tx.commit().await?;
        Ok(metadata)
    }

    /// Consume a code exactly once, creating one pending paused host.
    pub async fn consume_enrollment(
        &self,
        code: &str,
        registration: HostRegistration,
    ) -> Result<EnrollmentReceipt, RegistryError> {
        let code = OneTimeSecret::parse(code)?;
        let enrollment_verifier = code.verifier();
        let credential = OneTimeSecret::generate();
        let credential_verifier = credential.verifier();
        let host_id = HostId::try_new(Uuid::new_v4())
            .map_err(|_| RegistryError::InvalidRequest("host id generation failed"))?;
        let root_domain_id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        let row = sqlx::query(
            "SELECT id,intended_host_id,expires_at,consumed_at,revoked_at,mode,expected_host_epoch,expected_host_revision,
                    issued_by,scope_json
               FROM host_enrollments WHERE code_verifier=? AND verifier_format='sha256'",
        )
        .bind(enrollment_verifier.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(RegistryError::EnrollmentGone);
        };
        let enrollment_id: String = row.get("id");
        let intended_host_id: Option<String> = row.get("intended_host_id");
        let expires_at: i64 = row.get("expires_at");
        let consumed_at: Option<i64> = row.get("consumed_at");
        let revoked_at: Option<i64> = row.get("revoked_at");
        let mode: String = row.get("mode");
        let expected_epoch: Option<i64> = row.get("expected_host_epoch");
        let expected_revision: Option<i64> = row.get("expected_host_revision");
        let issued_by: String = row.get("issued_by");
        if expires_at <= now || consumed_at.is_some() || revoked_at.is_some() {
            return Err(RegistryError::EnrollmentGone);
        }
        let scope: EnrollmentScope = serde_json::from_str(&row.get::<String, _>("scope_json"))
            .map_err(|_| RegistryError::EnrollmentScopeMismatch)?;
        ensure_scope_admin(&mut tx, &issued_by, &scope).await?;
        if mode == "recovery" {
            let recovery_host_id = intended_host_id
                .as_deref()
                .ok_or(RegistryError::EnrollmentScopeMismatch)?
                .parse::<HostId>()?;
            let expected_epoch = expected_epoch.ok_or(RegistryError::EnrollmentScopeMismatch)?;
            let expected_revision =
                expected_revision.ok_or(RegistryError::EnrollmentScopeMismatch)?;
            let host_row = sqlx::query(
                "SELECT epoch,revision,enrollment_state,lifecycle_state FROM fleet_hosts WHERE id=?",
            )
                    .bind(recovery_host_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or(RegistryError::EnrollmentScopeMismatch)?;
            if host_row.get::<i64, _>("epoch") != expected_epoch
                || host_row.get::<i64, _>("revision") != expected_revision
                || host_row.get::<String, _>("enrollment_state") == "revoked"
            {
                return Err(RegistryError::EnrollmentScopeMismatch);
            }
            if host_row.get::<String, _>("lifecycle_state") == "retired" {
                return Err(RegistryError::Forbidden);
            }
            let new_epoch = expected_epoch
                .checked_add(1)
                .ok_or(RegistryError::InvalidRequest("host epoch overflowed"))?;
            let (max_generation, max_reserved): (i64, i64) = sqlx::query_as(
                "SELECT COALESCE((SELECT MAX(generation) FROM host_credentials WHERE host_id=?),0),
                        COALESCE((SELECT MAX(next_generation) FROM host_credential_rotations WHERE host_id=?),0)",
            )
            .bind(recovery_host_id.to_string())
            .bind(recovery_host_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
            let next_generation = max_generation.max(max_reserved).checked_add(1).ok_or(
                RegistryError::InvalidRequest("credential generation overflowed"),
            )?;
            revoke_open_rotations(
                &mut tx,
                recovery_host_id,
                now,
                None,
                "fleet.credential.rotation_revoked_by_recovery",
            )
            .await?;
            sqlx::query(
                "UPDATE host_credentials SET revoked_at=? WHERE host_id=? AND revoked_at IS NULL",
            )
            .bind(now)
            .bind(recovery_host_id.to_string())
            .execute(&mut *tx)
            .await?;
            let consumed = sqlx::query(
                "UPDATE host_enrollments SET consumed_at=?,consumed_host_id=?
                 WHERE id=? AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>?",
            )
            .bind(now)
            .bind(recovery_host_id.to_string())
            .bind(&enrollment_id)
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if consumed != 1 {
                return Err(RegistryError::EnrollmentGone);
            }
            let changed = sqlx::query(
                "UPDATE fleet_hosts SET epoch=?,revision=revision+1,enrollment_state='pending',
                 lifecycle_state='paused',integrity_state='unverified',
                 agent_session_id=NULL,boot_id=NULL,authority_expires_at=NULL,inventory_complete=0,
                 inventory_revision=0,inventory_digest=NULL,current_credential_generation=?,
                 current_enrollment_id=?,current_enrollment_epoch=?,updated_at=?
                 WHERE id=? AND epoch=? AND revision=?",
            )
            .bind(new_epoch)
            .bind(next_generation)
            .bind(&enrollment_id)
            .bind(new_epoch)
            .bind(now)
            .bind(recovery_host_id.to_string())
            .bind(expected_epoch)
            .bind(expected_revision)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if changed != 1 {
                return Err(RegistryError::StaleRevision);
            }
            sqlx::query(
                "INSERT INTO host_credentials
                 (id,host_id,host_epoch,generation,verifier,verifier_format,created_at)
                 VALUES (?,?,?, ?,?,'sha256',?)",
            )
            .bind(credential_id.to_string())
            .bind(recovery_host_id.to_string())
            .bind(new_epoch)
            .bind(next_generation)
            .bind(credential_verifier.as_str())
            .bind(now)
            .execute(&mut *tx)
            .await?;
            audit_and_event(
                &mut tx,
                None,
                Some(recovery_host_id),
                "fleet.host.recovered",
                "host",
                &recovery_host_id.to_string(),
                json!({"enrollment_id": enrollment_id, "epoch": new_epoch, "generation": next_generation}),
                now,
            )
            .await?;
            tx.commit().await?;
            return Ok(EnrollmentReceipt {
                host_id: recovery_host_id,
                credential_id,
                credential_generation: u64::try_from(next_generation)
                    .map_err(|_| RegistryError::InvalidRequest("generation is malformed"))?,
                credential,
            });
        }
        if mode != "normal" {
            return Err(RegistryError::EnrollmentScopeMismatch);
        }
        if intended_host_id.is_some() || expected_epoch.is_some() || expected_revision.is_some() {
            return Err(RegistryError::EnrollmentScopeMismatch);
        }
        let host_id_text = host_id.to_string();
        sqlx::query(
            "INSERT INTO fleet_hosts
             (id,name,host_os,architecture,enrollment_state,lifecycle_state,integrity_state,epoch,revision,
              hardware_fingerprint,current_credential_generation,created_at,updated_at)
             VALUES (?,?,?,?,'pending','paused','unverified',1,1,?,?,?,?)",
        )
        .bind(&host_id_text)
        .bind(registration.name())
        .bind(os_name(registration.host_os()))
        .bind(architecture_name(registration.architecture()))
        .bind(registration.hardware_fingerprint())
        .bind(1_i64)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO host_resource_domains
             (id,host_id,parent_domain_id,name,cpu_millis,memory_mib,disk_bytes,revision)
             VALUES (?,?,NULL,'physical-root',0,0,0,1)",
        )
        .bind(root_domain_id.to_string())
        .bind(&host_id_text)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO host_credentials
             (id,host_id,host_epoch,generation,verifier,verifier_format,created_at)
             VALUES (?,?,1,1,?,'sha256',?)",
        )
        .bind(credential_id.to_string())
        .bind(&host_id_text)
        .bind(credential_verifier.as_str())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let consumed = sqlx::query(
            "UPDATE host_enrollments SET consumed_at=?,consumed_host_id=?
             WHERE id=? AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at>?",
        )
        .bind(now)
        .bind(&host_id_text)
        .bind(&enrollment_id)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if consumed != 1 {
            return Err(RegistryError::EnrollmentGone);
        }
        let lineage_changed = sqlx::query(
            "UPDATE fleet_hosts
                SET current_enrollment_id=?,current_enrollment_epoch=epoch
              WHERE id=? AND epoch=1 AND current_enrollment_id IS NULL",
        )
        .bind(&enrollment_id)
        .bind(&host_id_text)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if lineage_changed != 1 {
            return Err(RegistryError::StaleRevision);
        }
        audit_and_event(
            &mut tx,
            None,
            Some(host_id),
            "fleet.enrollment.consumed",
            "host",
            &host_id_text,
            json!({"enrollment_id": enrollment_id, "generation": 1}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(EnrollmentReceipt {
            host_id,
            credential_id,
            credential_generation: 1,
            credential,
        })
    }

    /// Recover a known host after a committed response was lost.
    pub async fn recover_host(
        &self,
        principal: &Principal,
        request: RecoverHost,
    ) -> Result<RecoveryIssue, RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        validate_idempotency_key(&request.idempotency_key)?;
        let ttl = enrollment::DEFAULT_ENROLLMENT_TTL_MILLIS;
        let code = OneTimeSecret::generate();
        let verifier = code.verifier();
        let enrollment_id = Uuid::new_v4();
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        let expires_at = now.checked_add(ttl).ok_or(RegistryError::InvalidRequest(
            "recovery lifetime overflowed",
        ))?;
        ensure_system_admin(&mut tx, user_id).await?;
        let request_hash = hash_request(&format!(
            "{}|{}",
            request.host_id,
            request.expected_revision.get()
        ));
        if let Some(existing) = sqlx::query(
            "SELECT id,expires_at,consumed_at,revoked_at,consumed_host_id,request_hash,mode,scope_json
             FROM host_enrollments WHERE issued_by=? AND idempotency_key=?",
        )
        .bind(user_id)
        .bind(&request.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing_hash: String = existing.get("request_hash");
            if existing_hash != request_hash || existing.get::<String, _>("mode") != "recovery" {
                return Err(RegistryError::IdempotencyConflict);
            }
            let existing_scope: EnrollmentScope =
                serde_json::from_str(&existing.get::<String, _>("scope_json"))
                    .map_err(|_| RegistryError::EnrollmentScopeMismatch)?;
            ensure_scope_admin(&mut tx, user_id, &existing_scope).await?;
            let metadata = EnrollmentMetadata {
                id: parse_uuid(&existing.get::<String, _>("id"))?,
                expires_at: existing.get("expires_at"),
                consumed_at: existing.get("consumed_at"),
                revoked_at: existing.get("revoked_at"),
                consumed_host_id: existing
                    .get::<Option<String>, _>("consumed_host_id")
                    .map(|value| value.parse::<HostId>())
                    .transpose()?,
            };
            tx.commit().await?;
            return Ok(RecoveryIssue::Replay(metadata));
        }
        let host_row = sqlx::query(
            "SELECT epoch,revision,enrollment_state,lifecycle_state FROM fleet_hosts
              WHERE id=? AND enrollment_state!='revoked'",
        )
        .bind(request.host_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(host_row) = host_row else {
            return Err(RegistryError::NotFound);
        };
        if host_row.get::<String, _>("lifecycle_state") == "retired" {
            return Err(RegistryError::Forbidden);
        }
        let old_revision: i64 = host_row.get("revision");
        let expected_revision = i64::try_from(request.expected_revision.get())
            .map_err(|_| RegistryError::InvalidRequest("revision exceeds SQLite integer"))?;
        if old_revision != expected_revision {
            return Err(RegistryError::StaleRevision);
        }
        let old_epoch: i64 = host_row.get("epoch");
        let scope = if host_row.get::<String, _>("enrollment_state") == "pending" {
            let lineage = sqlx::query_scalar::<_, String>(
                "SELECT e.scope_json FROM fleet_hosts h
                   JOIN host_enrollments e ON e.id=h.current_enrollment_id
                  WHERE h.id=? AND e.consumed_host_id=h.id
                    AND e.consumed_at IS NOT NULL",
            )
            .bind(request.host_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(RegistryError::EnrollmentScopeMismatch)?;
            serde_json::from_str(&lineage).map_err(|_| RegistryError::EnrollmentScopeMismatch)?
        } else {
            let target_ids: Vec<CiTargetId> = sqlx::query_scalar(
                "SELECT target_id FROM host_target_grants
                  WHERE host_id=? AND revoked_at IS NULL ORDER BY target_id",
            )
            .bind(request.host_id.to_string())
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|target_id: String| target_id.parse())
            .collect::<Result<_, _>>()?;
            EnrollmentScope::new(target_ids)?
        };
        ensure_scope_admin(&mut tx, user_id, &scope).await?;
        let scope_json = serde_json::to_string(&scope)?;
        sqlx::query(
            "INSERT INTO host_enrollments
             (id,code_verifier,intended_host_id,scope_json,issued_by,created_at,expires_at,
              mode,expected_host_epoch,expected_host_revision,verifier_format,idempotency_key,request_hash)
             VALUES (?,?,?,?,?,?,?,'recovery',?,?, 'sha256',?,?)",
        )
        .bind(enrollment_id.to_string())
        .bind(verifier.as_str())
        .bind(request.host_id.to_string())
        .bind(scope_json)
        .bind(user_id)
        .bind(now)
        .bind(expires_at)
        .bind(old_epoch)
        .bind(old_revision)
        .bind(&request.idempotency_key)
        .bind(&request_hash)
        .execute(&mut *tx)
        .await?;
        audit_and_event(
            &mut tx,
            Some(user_id),
            Some(request.host_id),
            "fleet.host.recovery_issued",
            "host",
            &request.host_id.to_string(),
            json!({"expires_at": expires_at, "expected_epoch": old_epoch, "expected_revision": old_revision}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(RecoveryIssue::Created(IssuedRecoveryEnrollment {
            id: enrollment_id,
            host_id: request.host_id,
            code,
            expires_at,
        }))
    }

    /// Approve exact targets after checking current target-administration grants.
    pub async fn approve_host(
        &self,
        principal: &Principal,
        request: ApproveHost,
    ) -> Result<(), RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        ensure_system_admin(&mut tx, user_id).await?;
        let Some(row) = sqlx::query(
            "SELECT revision,inventory_revision,inventory_digest,inventory_complete,
                    inventory_epoch,inventory_session_id,inventory_boot_id,authority_incarnation,
                    agent_session_id,boot_id,last_inventory_at,
                    epoch,enrollment_state,lifecycle_state,current_credential_generation
               FROM fleet_hosts WHERE id=?",
        )
        .bind(request.host_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Err(RegistryError::NotFound);
        };
        let revision: i64 = row.get("revision");
        let inventory_revision: i64 = row.get("inventory_revision");
        let inventory_digest: Option<String> = row.get("inventory_digest");
        let inventory_complete: i64 = row.get("inventory_complete");
        let inventory_epoch: Option<i64> = row.get("inventory_epoch");
        let inventory_session_id: Option<String> = row.get("inventory_session_id");
        let inventory_boot_id: Option<String> = row.get("inventory_boot_id");
        let authority_incarnation: Option<String> = row.get("authority_incarnation");
        let agent_session_id: Option<String> = row.get("agent_session_id");
        let boot_id: Option<String> = row.get("boot_id");
        let last_inventory_at: Option<i64> = row.get("last_inventory_at");
        let epoch: i64 = row.get("epoch");
        let state: String = row.get("enrollment_state");
        let lifecycle_state: String = row.get("lifecycle_state");
        if state == "revoked" || lifecycle_state == "retired" {
            return Err(RegistryError::Forbidden);
        }
        if state != "pending" {
            return Err(RegistryError::StaleRevision);
        }
        let expected_revision = i64::try_from(request.expected_revision.get())
            .map_err(|_| RegistryError::InvalidRequest("revision exceeds SQLite integer"))?;
        let expected_inventory_revision = i64::try_from(request.expected_inventory_revision)
            .map_err(|_| {
                RegistryError::InvalidRequest("inventory revision exceeds SQLite integer")
            })?;
        let control_plane_incarnation = sqlx::query_scalar::<_, String>(
            "SELECT incarnation FROM fleet_control_plane WHERE singleton=1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let inventory_is_fresh = last_inventory_at.is_some_and(|received_at| {
            received_at <= now && now.saturating_sub(received_at) < 45_000
        });
        let digest_is_canonical = inventory_digest
            .as_deref()
            .is_some_and(|digest| OneTimeSecret::parse(digest).is_ok());
        if revision != expected_revision
            || inventory_revision != expected_inventory_revision
            || expected_inventory_revision <= 0
            || inventory_digest.as_deref() != Some(request.expected_inventory_digest.as_str())
            || !digest_is_canonical
            || inventory_complete != 1
            || inventory_epoch != Some(epoch)
            || inventory_session_id != agent_session_id
            || inventory_boot_id != boot_id
            || authority_incarnation.as_deref() != control_plane_incarnation.as_deref()
            || !inventory_is_fresh
        {
            return Err(RegistryError::StaleRevision);
        }
        let observation_is_current = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)
               FROM fleet_observation_sessions s
               JOIN fleet_inventory_snapshots i
                 ON i.host_id=s.host_id
                AND i.authority_session_id=s.session_id
                AND i.inventory_sequence=s.current_inventory_sequence
               JOIN host_credentials c
                 ON c.host_id=s.host_id
                AND c.id=s.current_credential_id
                AND c.generation=s.current_credential_generation
                AND c.host_epoch=s.host_epoch
              WHERE s.host_id=?
                AND s.session_id=?
                AND s.host_epoch=?
                AND s.boot_id=?
                AND s.control_plane_incarnation=?
                AND s.ended_at IS NULL
                AND s.state!='conflicted'
                AND s.lease_expires_at>?
                AND s.current_inventory_digest=?
                AND s.current_inventory_received_at=?
                AND i.host_epoch=s.host_epoch
                AND i.control_plane_incarnation=s.control_plane_incarnation
                AND i.inventory_sequence>0
                AND i.digest=s.current_inventory_digest
                AND i.received_at=s.current_inventory_received_at
                AND i.inventory_revision=?
                AND c.revoked_at IS NULL
                AND (c.overlap_expires_at IS NULL OR c.overlap_expires_at>?)",
        )
        .bind(request.host_id.to_string())
        .bind(agent_session_id.as_deref())
        .bind(epoch)
        .bind(boot_id.as_deref())
        .bind(authority_incarnation.as_deref())
        .bind(now)
        .bind(request.expected_inventory_digest.as_str())
        .bind(last_inventory_at)
        .bind(expected_inventory_revision)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        if observation_is_current != 1 {
            return Err(RegistryError::StaleRevision);
        }
        let enrollment = sqlx::query(
            "SELECT e.scope_json,e.issued_by FROM fleet_hosts h
               JOIN host_enrollments e ON e.id=h.current_enrollment_id
              WHERE h.id=? AND e.consumed_host_id=h.id
                AND e.consumed_at IS NOT NULL",
        )
        .bind(request.host_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RegistryError::EnrollmentScopeMismatch)?;
        let issued_scope: EnrollmentScope =
            serde_json::from_str(&enrollment.get::<String, _>("scope_json"))
                .map_err(|_| RegistryError::EnrollmentScopeMismatch)?;
        let issuer_id: String = enrollment.get("issued_by");
        ensure_scope_admin(&mut tx, &issuer_id, &issued_scope).await?;
        if !scope_is_subset(&request.scope, &issued_scope) {
            return Err(RegistryError::EnrollmentScopeMismatch);
        }
        ensure_scope_admin(&mut tx, user_id, &request.scope).await?;
        for target_id in request.scope.target_ids() {
            ensure_target_admin(&mut tx, user_id, target_id).await?;
        }
        let current_target_ids: Vec<CiTargetId> = sqlx::query_scalar(
            "SELECT target_id FROM host_target_grants
              WHERE host_id=? AND revoked_at IS NULL ORDER BY target_id",
        )
        .bind(request.host_id.to_string())
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|target_id: String| target_id.parse())
        .collect::<Result<_, _>>()?;
        for target_id in current_target_ids.iter().copied() {
            ensure_target_admin(&mut tx, user_id, target_id).await?;
        }
        let requested_target_ids = request.scope.target_ids().collect::<Vec<_>>();
        sqlx::query(
            "UPDATE host_target_grants SET revoked_at=?
               WHERE host_id=? AND revoked_at IS NULL
                 AND target_id NOT IN (SELECT value FROM json_each(?))",
        )
        .bind(now)
        .bind(request.host_id.to_string())
        .bind(serde_json::to_string(
            &requested_target_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )?)
        .execute(&mut *tx)
        .await?;
        for target_id in requested_target_ids {
            let target_id = target_id.to_string();
            let existing = sqlx::query(
                "SELECT revoked_at FROM host_target_grants WHERE host_id=? AND target_id=?",
            )
            .bind(request.host_id.to_string())
            .bind(&target_id)
            .fetch_optional(&mut *tx)
            .await?;
            match existing {
                None => {
                    sqlx::query(
                        "INSERT INTO host_target_grants
                         (host_id,target_id,allow_schedule,allow_native,allow_interactive,allow_docker_socket,revision,granted_by)
                         VALUES (?, ?, 0, 0, 0, 0, 1, ?)",
                    )
                    .bind(request.host_id.to_string())
                    .bind(&target_id)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                }
                Some(row) if row.get::<Option<i64>, _>("revoked_at").is_some() => {
                    sqlx::query(
                        "UPDATE host_target_grants
                            SET allow_schedule=0,allow_native=0,allow_interactive=0,
                                allow_docker_socket=0,revision=revision+1,revoked_at=NULL,
                                granted_by=?
                          WHERE host_id=? AND target_id=? AND revoked_at IS NOT NULL",
                    )
                    .bind(user_id)
                    .bind(request.host_id.to_string())
                    .bind(&target_id)
                    .execute(&mut *tx)
                    .await?;
                }
                Some(_) => {}
            }
        }
        sqlx::query(
            "UPDATE fleet_hosts SET enrollment_state='approved',lifecycle_state='paused',
             integrity_state='verified',revision=revision+1,updated_at=?
             WHERE id=? AND revision=?",
        )
        .bind(now)
        .bind(request.host_id.to_string())
        .bind(revision)
        .execute(&mut *tx)
        .await?;
        audit_and_event(
            &mut tx,
            Some(user_id),
            Some(request.host_id),
            "fleet.host.approved",
            "host",
            &request.host_id.to_string(),
            json!({"inventory_revision": request.expected_inventory_revision, "targets": request.scope}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Authenticate a bearer secret without exposing its verifier or row.
    pub async fn authenticate(
        &self,
        host_id: HostId,
        secret: &str,
    ) -> Result<AuthenticatedHost, RegistryError> {
        validate_secret_for_authentication(secret)?;
        let now = self.clock.now_millis();
        let verifier = Verifier::from_secret(secret);
        let row = sqlx::query(
            "SELECT c.id,c.generation,c.host_epoch,c.verifier,h.epoch,h.enrollment_state,h.lifecycle_state
               FROM host_credentials c JOIN fleet_hosts h ON h.id=c.host_id
              WHERE c.host_id=? AND c.verifier_format='sha256'
                AND c.revoked_at IS NULL AND (c.overlap_expires_at IS NULL OR c.overlap_expires_at>?)",
        )
        .bind(host_id.to_string())
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        let Some(row) = row.into_iter().find(|row| {
            let stored: String = row.get("verifier");
            verifier.matches_verifier(&Verifier::from_hash(stored))
        }) else {
            return Err(RegistryError::InvalidCredential);
        };
        let enrollment_state: String = row.get("enrollment_state");
        let lifecycle_state: String = row.get("lifecycle_state");
        if enrollment_state == "revoked" || lifecycle_state == "retired" {
            return Err(RegistryError::InvalidCredential);
        }
        let credential_epoch: i64 = row.get("host_epoch");
        let host_epoch: i64 = row.get("epoch");
        let generation: i64 = row.get("generation");
        let credential_id: String = row.get("id");
        Ok(AuthenticatedHost {
            host_id,
            credential_id: Uuid::parse_str(&credential_id)
                .map_err(|_| RegistryError::InvalidCredential)?,
            generation: Generation::new(
                u64::try_from(generation).map_err(|_| RegistryError::InvalidCredential)?,
            )
            .map_err(|_| RegistryError::InvalidCredential)?,
            epoch: HostEpoch::new(
                u64::try_from(host_epoch).map_err(|_| RegistryError::InvalidCredential)?,
            )
            .map_err(|_| RegistryError::InvalidCredential)?,
        })
        .and_then(|authenticated| {
            if credential_epoch == host_epoch {
                Ok(authenticated)
            } else {
                Err(RegistryError::InvalidCredential)
            }
        })
    }

    /// Create or replay a durable host-generated rotation request.
    pub async fn request_rotation(
        &self,
        principal: &Principal,
        request: RequestRotation,
    ) -> Result<RotationRequestReceipt, RegistryError> {
        validate_idempotency_key(&request.idempotency_key)?;
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        let request_method = "fleet.credential.request";
        let request_resource = request.host_id.to_string();
        let request_hash = hash_request(&request.expected_generation.get().to_string());
        let expires_at =
            now.checked_add(ROTATION_TTL_MILLIS)
                .ok_or(RegistryError::InvalidRequest(
                    "rotation lifetime overflowed",
                ))?;
        ensure_system_admin(&mut tx, user_id).await?;
        // Idempotency is resolved before lifecycle, epoch, or generation
        // checks.  A committed operation is immutable evidence: the exact
        // originating actor/request gets the original metadata even after a
        // terminal transition or host recovery.
        if let Some(existing) = sqlx::query(
            "SELECT id,host_id,old_generation,next_generation,expires_at,
                    requested_by,request_method,request_resource,request_hash
               FROM host_credential_rotations
              WHERE host_id=? AND idempotency_key=?",
        )
        .bind(request.host_id.to_string())
        .bind(&request.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let bound_to_requester = existing.get::<Option<String>, _>("requested_by").as_deref()
                == Some(user_id)
                && existing
                    .get::<Option<String>, _>("request_method")
                    .as_deref()
                    == Some(request_method)
                && existing
                    .get::<Option<String>, _>("request_resource")
                    .as_deref()
                    == Some(request_resource.as_str())
                && existing.get::<Option<String>, _>("request_hash").as_deref()
                    == Some(request_hash.as_str());
            if !bound_to_requester {
                return Err(RegistryError::IdempotencyConflict);
            }
            let old_generation = generation_from_i64(existing.get("old_generation"))?;
            let receipt = RotationRequestReceipt {
                operation_id: parse_uuid(&existing.get::<String, _>("id"))?,
                host_id: request.host_id,
                old_generation,
                next_generation: generation_from_i64(existing.get("next_generation"))?,
                expires_at: existing.get("expires_at"),
            };
            tx.commit().await?;
            return Ok(receipt);
        }
        let host = sqlx::query(
            "SELECT epoch,enrollment_state,lifecycle_state,current_credential_generation
               FROM fleet_hosts WHERE id=?",
        )
        .bind(request.host_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RegistryError::NotFound)?;
        if host.get::<String, _>("enrollment_state") == "revoked"
            || host.get::<String, _>("lifecycle_state") == "retired"
        {
            return Err(RegistryError::Forbidden);
        }
        let expected_generation = i64::try_from(request.expected_generation.get())
            .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?;
        if host
            .get::<Option<i64>, _>("current_credential_generation")
            .is_some_and(|current| current != expected_generation)
        {
            return Err(RegistryError::StaleGeneration);
        }
        revoke_expired_rotations(&mut tx, request.host_id, now, user_id).await?;
        if sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM host_credential_rotations
              WHERE host_id=? AND phase IN ('requested','exchanged')
                AND revoked_at IS NULL AND acknowledged_at IS NULL",
        )
        .bind(request.host_id.to_string())
        .fetch_one(&mut *tx)
        .await?
            != 0
        {
            return Err(RegistryError::RotationConflict);
        }
        let current: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT id,generation,host_epoch FROM host_credentials
               WHERE host_id=? AND generation=? AND host_epoch=? AND revoked_at IS NULL
                 AND (overlap_expires_at IS NULL OR overlap_expires_at>?)",
        )
        .bind(request.host_id.to_string())
        .bind(expected_generation)
        .bind(host.get::<i64, _>("epoch"))
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((old_credential_id, old_generation, old_host_epoch)) = current else {
            return Err(RegistryError::StaleGeneration);
        };
        let operation_id = Uuid::new_v4();
        let (max_generation, max_reserved): (i64, i64) = sqlx::query_as(
            "SELECT COALESCE((SELECT MAX(generation) FROM host_credentials WHERE host_id=?),0),
                    COALESCE((SELECT MAX(next_generation) FROM host_credential_rotations WHERE host_id=?),0)",
        )
        .bind(request.host_id.to_string())
        .bind(request.host_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        let next_generation = max_generation.max(max_reserved).checked_add(1).ok_or(
            RegistryError::InvalidRequest("credential generation overflowed"),
        )?;
        sqlx::query(
            "INSERT INTO host_credential_rotations
             (id,host_id,old_credential_id,old_generation,old_host_epoch,next_generation,
              idempotency_key,requested_by,request_method,request_resource,request_hash,
              phase,requested_at,expires_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,'requested',?,?)",
        )
        .bind(operation_id.to_string())
        .bind(request.host_id.to_string())
        .bind(old_credential_id)
        .bind(old_generation)
        .bind(old_host_epoch)
        .bind(next_generation)
        .bind(&request.idempotency_key)
        .bind(user_id)
        .bind(request_method)
        .bind(&request_resource)
        .bind(&request_hash)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        audit_and_event(
            &mut tx,
            Some(user_id),
            Some(request.host_id),
            "fleet.credential.rotation_requested",
            "host",
            &request.host_id.to_string(),
            json!({"operation_id": operation_id, "old_generation": old_generation, "next_generation": next_generation}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(RotationRequestReceipt {
            operation_id,
            host_id: request.host_id,
            old_generation: request.expected_generation,
            next_generation: Generation::new(
                u64::try_from(next_generation)
                    .map_err(|_| RegistryError::InvalidRequest("generation invalid"))?,
            )
            .map_err(|_| RegistryError::InvalidRequest("generation invalid"))?,
            expires_at,
        })
    }

    /// Read rotation progress without returning either verifier or secret.
    pub async fn rotation_status(
        &self,
        principal: &Principal,
        operation_id: Uuid,
    ) -> Result<RotationOperationStatus, RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        let mut tx = self.begin().await?;
        ensure_system_admin(&mut tx, user_id).await?;
        let Some(row) = sqlx::query(
            "SELECT host_id,old_generation,next_generation,expires_at,requested_at,exchanged_at,
                    proposed_verifier,overlap_expires_at,phase,acknowledged_at,revoked_at
               FROM host_credential_rotations WHERE id=?",
        )
        .bind(operation_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Err(RegistryError::NotFound);
        };
        let now = self.clock.now_millis();
        let state = if row.get::<String, _>("phase") == "revoked" {
            RotationOperationState::Revoked
        } else if row.get::<Option<i64>, _>("acknowledged_at").is_some() {
            RotationOperationState::Acknowledged
        } else if row.get::<Option<String>, _>("proposed_verifier").is_some() {
            RotationOperationState::Exchanged
        } else if row.get::<i64, _>("expires_at") <= now {
            RotationOperationState::Expired
        } else {
            RotationOperationState::Requested
        };
        let updated_at = match state {
            RotationOperationState::Requested | RotationOperationState::Expired => {
                row.get("requested_at")
            }
            RotationOperationState::Exchanged => row
                .get::<Option<i64>, _>("exchanged_at")
                .unwrap_or_else(|| row.get("requested_at")),
            RotationOperationState::Acknowledged => row
                .get::<Option<i64>, _>("acknowledged_at")
                .unwrap_or_else(|| row.get("requested_at")),
            RotationOperationState::Revoked => row
                .get::<Option<i64>, _>("revoked_at")
                .unwrap_or_else(|| row.get("requested_at")),
        };
        let result = RotationOperationStatus {
            operation_id,
            host_id: row.get::<String, _>("host_id").parse()?,
            state,
            old_generation: generation_from_i64(row.get("old_generation"))?,
            next_generation: generation_from_i64(row.get("next_generation"))?,
            expires_at: row.get("expires_at"),
            updated_at,
        };
        tx.commit().await?;
        Ok(result)
    }

    /// Exchange a host-generated next secret using the old authenticated generation.
    pub async fn exchange_rotation(
        &self,
        authenticated: &AuthenticatedHost,
        operation_id: Uuid,
        next_secret: &str,
    ) -> Result<RotationExchangeReceipt, RegistryError> {
        let secret = OneTimeSecret::parse(next_secret)?;
        let next_verifier = secret.verifier();
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        ensure_authenticated_generation(&mut tx, authenticated, now).await?;
        let row = sqlx::query(
            "SELECT host_id,old_credential_id,old_generation,old_host_epoch,next_generation,expires_at,
                    proposed_verifier,exchanged_at,overlap_expires_at,phase,acknowledged_at,revoked_at
               FROM host_credential_rotations WHERE id=?",
        )
        .bind(operation_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(RegistryError::NotFound);
        };
        let host_id: String = row.get("host_id");
        if host_id != authenticated.host_id.to_string() {
            return Err(RegistryError::Forbidden);
        }
        let old_generation = generation_from_i64(row.get("old_generation"))?;
        if old_generation != authenticated.generation {
            return Err(RegistryError::StaleGeneration);
        }
        let authenticated_epoch = i64::try_from(authenticated.epoch.get())
            .map_err(|_| RegistryError::InvalidCredential)?;
        if row.get::<String, _>("old_credential_id") != authenticated.credential_id.to_string()
            || row.get::<i64, _>("old_host_epoch") != authenticated_epoch
        {
            return Err(RegistryError::StaleGeneration);
        }
        if row.get::<String, _>("phase") == "revoked"
            || (row.get::<String, _>("phase") == "requested"
                && row.get::<i64, _>("expires_at") <= now)
        {
            return Err(RegistryError::RotationGone);
        }
        if let Some(existing) = row.get::<Option<String>, _>("proposed_verifier") {
            let existing = Verifier::from_hash(existing);
            if !existing.matches_verifier(&next_verifier) {
                return Err(RegistryError::RotationConflict);
            }
            let next_generation = generation_from_i64(row.get("next_generation"))?;
            let credential_id: String =
                sqlx::query_scalar(
                    "SELECT id FROM host_credentials WHERE host_id=? AND generation=?",
                )
                .bind(&host_id)
                .bind(i64::try_from(next_generation.get()).map_err(|_| {
                    RegistryError::InvalidRequest("generation exceeds SQLite integer")
                })?)
                .fetch_one(&mut *tx)
                .await?;
            let overlap_expires_at: i64 = row
                .get::<Option<i64>, _>("overlap_expires_at")
                .ok_or(RegistryError::RotationConflict)?;
            tx.commit().await?;
            return Ok(RotationExchangeReceipt {
                operation_id,
                credential_id: parse_uuid(&credential_id)?,
                generation: next_generation,
                overlap_expires_at,
            });
        }
        let next_generation = generation_from_i64(row.get("next_generation"))?;
        let overlap_expires_at = now
            .checked_add(ROTATION_OVERLAP_MILLIS)
            .ok_or(RegistryError::InvalidRequest("overlap lifetime overflowed"))?;
        let credential_id = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO host_credentials
             (id,host_id,host_epoch,generation,verifier,verifier_format,created_at,overlap_expires_at)
             SELECT ?,id,epoch,?,?,'sha256',?,NULL
             FROM fleet_hosts WHERE id=?",
        )
        .bind(credential_id.to_string())
        .bind(i64::try_from(next_generation.get()).map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?)
        .bind(next_verifier.as_str())
        .bind(now)
        .bind(&host_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted != 1 {
            return Err(RegistryError::RotationConflict);
        }
        let old_updated = sqlx::query(
            "UPDATE host_credentials SET overlap_expires_at=?
               WHERE id=? AND host_id=? AND generation=? AND host_epoch=? AND revoked_at IS NULL",
        )
        .bind(overlap_expires_at)
        .bind(row.get::<String, _>("old_credential_id"))
        .bind(&host_id)
        .bind(
            i64::try_from(old_generation.get())
                .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?,
        )
        .bind(row.get::<i64, _>("old_host_epoch"))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if old_updated != 1 {
            return Err(RegistryError::StaleGeneration);
        }
        let exchanged = sqlx::query(
            "UPDATE host_credential_rotations
                SET phase='exchanged',proposed_verifier=?,exchanged_at=?,overlap_expires_at=?
              WHERE id=? AND phase='requested' AND revoked_at IS NULL AND expires_at>?",
        )
        .bind(next_verifier.as_str())
        .bind(now)
        .bind(overlap_expires_at)
        .bind(operation_id.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if exchanged != 1 {
            return Err(RegistryError::RotationConflict);
        }
        audit_and_event(
            &mut tx,
            None,
            Some(authenticated.host_id),
            "fleet.credential.rotation_exchanged",
            "host",
            &host_id,
            json!({"operation_id": operation_id, "generation": next_generation.get(), "overlap_expires_at": overlap_expires_at}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(RotationExchangeReceipt {
            operation_id,
            credential_id,
            generation: next_generation,
            overlap_expires_at,
        })
    }

    /// A new-generation authenticated host acknowledges rotation and fences old credentials.
    pub async fn acknowledge_rotation(
        &self,
        authenticated: &AuthenticatedHost,
        operation_id: Uuid,
    ) -> Result<RotationAckReceipt, RegistryError> {
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        ensure_authenticated_generation(&mut tx, authenticated, now).await?;
        let row = sqlx::query(
            "SELECT host_id,next_generation,exchanged_at,acknowledged_at,revoked_at,expires_at,phase
             FROM host_credential_rotations WHERE id=?",
        )
        .bind(operation_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(RegistryError::NotFound);
        };
        if row.get::<String, _>("host_id") != authenticated.host_id.to_string()
            || generation_from_i64(row.get("next_generation"))? != authenticated.generation
        {
            return Err(RegistryError::StaleGeneration);
        }
        if row.get::<String, _>("phase") == "revoked"
            || row.get::<Option<i64>, _>("exchanged_at").is_none()
        {
            return Err(RegistryError::RotationGone);
        }
        if let Some(acknowledged_at) = row.get::<Option<i64>, _>("acknowledged_at") {
            tx.commit().await?;
            return Ok(RotationAckReceipt {
                operation_id,
                host_id: authenticated.host_id,
                generation: authenticated.generation,
                acknowledged_at,
            });
        }
        let acknowledged = sqlx::query(
            "UPDATE host_credential_rotations
                SET phase='acknowledged',acknowledged_at=?
              WHERE id=? AND phase='exchanged' AND acknowledged_at IS NULL AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(operation_id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if acknowledged != 1 {
            return Err(RegistryError::RotationConflict);
        }
        let marked = sqlx::query(
            "UPDATE host_credentials SET acknowledged_at=?
              WHERE host_id=? AND generation=? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(authenticated.host_id.to_string())
        .bind(
            i64::try_from(authenticated.generation.get())
                .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if marked != 1 {
            return Err(RegistryError::StaleGeneration);
        }
        sqlx::query(
            "UPDATE host_credentials SET revoked_at=?
             WHERE host_id=? AND generation<? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(authenticated.host_id.to_string())
        .bind(
            i64::try_from(authenticated.generation.get())
                .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE fleet_hosts SET current_credential_generation=?,revision=revision+1,updated_at=? WHERE id=?")
            .bind(i64::try_from(authenticated.generation.get()).map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?)
            .bind(now)
            .bind(authenticated.host_id.to_string())
            .execute(&mut *tx)
            .await?;
        audit_and_event(
            &mut tx,
            None,
            Some(authenticated.host_id),
            "fleet.credential.rotation_acknowledged",
            "host",
            &authenticated.host_id.to_string(),
            json!({"operation_id": operation_id, "generation": authenticated.generation.get()}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(RotationAckReceipt {
            operation_id,
            host_id: authenticated.host_id,
            generation: authenticated.generation,
            acknowledged_at: now,
        })
    }

    /// Revoke a generation and fence its current host authority.
    pub async fn revoke_credential(
        &self,
        principal: &Principal,
        host_id: HostId,
        generation: Generation,
    ) -> Result<(), RegistryError> {
        let user_id = require_user(principal).map_err(|_| RegistryError::Forbidden)?;
        let mut tx = self.begin().await?;
        let now = self.clock.now_millis();
        ensure_system_admin(&mut tx, user_id).await?;
        let lifecycle: String =
            sqlx::query_scalar("SELECT lifecycle_state FROM fleet_hosts WHERE id=?")
                .bind(host_id.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(RegistryError::NotFound)?;
        let revoked = sqlx::query(
            "UPDATE host_credentials SET revoked_at=? WHERE host_id=? AND generation=? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(host_id.to_string())
        .bind(i64::try_from(generation.get()).map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if revoked != 1 {
            return Err(RegistryError::NotFound);
        }
        revoke_open_rotations(
            &mut tx,
            host_id,
            now,
            Some(user_id),
            "fleet.credential.rotation_revoked",
        )
        .await?;
        if lifecycle != "retired" {
            sqlx::query(
                "UPDATE fleet_hosts SET lifecycle_state='paused',integrity_state='quarantined',
                 revision=revision+1,agent_session_id=NULL,boot_id=NULL,authority_expires_at=NULL,
                 inventory_complete=0,inventory_revision=0,inventory_digest=NULL,updated_at=? WHERE id=?",
            )
            .bind(now)
            .bind(host_id.to_string())
            .execute(&mut *tx)
            .await?;
        }
        audit_and_event(
            &mut tx,
            Some(user_id),
            Some(host_id),
            "fleet.credential.revoked",
            "host",
            &host_id.to_string(),
            json!({"generation": generation.get()}),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn begin(&self) -> Result<Transaction<'_, Sqlite>, RegistryError> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }
}

async fn ensure_system_admin(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: &str,
) -> Result<(), RegistryError> {
    let role = sqlx::query_scalar::<_, String>("SELECT role FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?;
    if role.as_deref() == Some("admin") {
        Ok(())
    } else {
        Err(RegistryError::Forbidden)
    }
}

async fn ensure_target_admin(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: &str,
    target_id: CiTargetId,
) -> Result<(), RegistryError> {
    let row = sqlx::query(
        "SELECT t.kind,t.installation_id,t.repository_id,t.organization_id,
                t.bitbucket_connection_id,t.workspace_uuid,
                c.workspace_uuid AS connection_workspace_uuid,
                i.account_id AS installation_account_id
           FROM fleet_ci_targets t
           LEFT JOIN bitbucket_connections c ON c.id=t.bitbucket_connection_id
           LEFT JOIN installations i ON i.id=t.installation_id
          WHERE t.id=?",
    )
    .bind(target_id.to_string())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RegistryError::NotFound)?;
    let allowed = if let Some(installation_id) = row.get::<Option<i64>, _>("installation_id") {
        let kind: String = row.get("kind");
        let installation_identity_matches = if kind == "github_organization" {
            row.get::<Option<i64>, _>("organization_id")
                == row.get::<Option<i64>, _>("installation_account_id")
        } else {
            true
        };
        let repository_matches = match row.get::<Option<i64>, _>("repository_id") {
            Some(repository_id) => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM repositories
                      WHERE id=? AND installation_id=?",
                )
                .bind(repository_id)
                .bind(installation_id)
                .fetch_one(&mut **tx)
                .await?
                    != 0
            }
            None => true,
        };
        installation_identity_matches
            && repository_matches
            && sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM user_installations ui
                  JOIN installations i ON i.id=ui.installation_id
                 WHERE ui.user_id=? AND ui.installation_id=?
                   AND ui.permission='admin' AND i.suspended_at IS NULL",
            )
            .bind(user_id)
            .bind(installation_id)
            .fetch_one(&mut **tx)
            .await?
                != 0
    } else if let Some(connection_id) = row.get::<Option<String>, _>("bitbucket_connection_id") {
        row.get::<Option<String>, _>("workspace_uuid")
            .zip(row.get::<Option<String>, _>("connection_workspace_uuid"))
            .is_some_and(|(target, connection)| {
                target
                    .trim_matches(['{', '}'])
                    .eq_ignore_ascii_case(connection.trim_matches(['{', '}']))
            })
            && sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM bitbucket_user_grants
                  WHERE user_id=? AND connection_id=? AND permission='admin'
                    AND revoked_at IS NULL",
            )
            .bind(user_id)
            .bind(connection_id)
            .fetch_one(&mut **tx)
            .await?
                != 0
    } else {
        false
    };
    if allowed {
        Ok(())
    } else {
        Err(RegistryError::TargetScopeForbidden)
    }
}

async fn ensure_scope_admin(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: &str,
    scope: &EnrollmentScope,
) -> Result<(), RegistryError> {
    ensure_system_admin(tx, user_id).await?;
    for target_id in scope.target_ids() {
        ensure_target_admin(tx, user_id, target_id).await?;
    }
    Ok(())
}

fn scope_is_subset(requested: &EnrollmentScope, issued: &EnrollmentScope) -> bool {
    requested.target_ids().all(|requested_id| {
        issued
            .target_ids()
            .any(|issued_id| issued_id == requested_id)
    })
}

async fn ensure_authenticated_generation(
    tx: &mut Transaction<'_, Sqlite>,
    authenticated: &AuthenticatedHost,
    now: i64,
) -> Result<(), RegistryError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM host_credentials c JOIN fleet_hosts h ON h.id=c.host_id
         WHERE c.id=? AND c.host_id=? AND c.generation=? AND c.host_epoch=?
           AND h.epoch=?
           AND c.revoked_at IS NULL AND (c.overlap_expires_at IS NULL OR c.overlap_expires_at>?)
           AND h.enrollment_state!='revoked' AND h.lifecycle_state!='retired'",
    )
    .bind(authenticated.credential_id.to_string())
    .bind(authenticated.host_id.to_string())
    .bind(
        i64::try_from(authenticated.generation.get())
            .map_err(|_| RegistryError::InvalidRequest("generation exceeds SQLite integer"))?,
    )
    .bind(
        i64::try_from(authenticated.epoch.get())
            .map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?,
    )
    .bind(
        i64::try_from(authenticated.epoch.get())
            .map_err(|_| RegistryError::InvalidRequest("epoch exceeds SQLite integer"))?,
    )
    .bind(now)
    .fetch_one(&mut **tx)
    .await?;
    if count == 1 {
        Ok(())
    } else {
        Err(RegistryError::InvalidCredential)
    }
}

// The event envelope deliberately keeps actor, host, action, target, typed
// metadata, and server time explicit so every caller supplies the audit scope.
#[allow(clippy::too_many_arguments)]
async fn audit_and_event(
    tx: &mut Transaction<'_, Sqlite>,
    actor_user_id: Option<&str>,
    host_id: Option<HostId>,
    action: &str,
    target_type: &str,
    target_id: &str,
    metadata: serde_json::Value,
    now: i64,
) -> Result<(), RegistryError> {
    let metadata = serde_json::to_string(&metadata)?;
    sqlx::query(
        "INSERT INTO audit_events
         (id,actor_user_id,actor_label,action,target_type,target_id,metadata,created_at)
         VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(actor_user_id)
    .bind(actor_user_id.unwrap_or("fleet-agent"))
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(&metadata)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO fleet_events (id,host_id,actor_user_id,kind,detail_json,created_at)
         VALUES (?,?,?,?,?,?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(host_id.map(|value| value.to_string()))
    .bind(actor_user_id)
    .bind(action)
    .bind(metadata)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn revoke_expired_rotations(
    tx: &mut Transaction<'_, Sqlite>,
    host_id: HostId,
    now: i64,
    actor_user_id: &str,
) -> Result<(), RegistryError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM host_credential_rotations
          WHERE host_id=? AND phase='requested' AND expires_at<=?",
    )
    .bind(host_id.to_string())
    .bind(now)
    .fetch_all(&mut **tx)
    .await?;
    for rotation_id in ids {
        let changed = sqlx::query(
            "UPDATE host_credential_rotations SET phase='revoked',revoked_at=?
              WHERE id=? AND phase='requested' AND expires_at<=?",
        )
        .bind(now)
        .bind(&rotation_id)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if changed == 1 {
            audit_and_event(
                tx,
                Some(actor_user_id),
                Some(host_id),
                "fleet.credential.rotation_expired",
                "rotation",
                &rotation_id,
                json!({}),
                now,
            )
            .await?;
        }
    }
    Ok(())
}

async fn revoke_open_rotations(
    tx: &mut Transaction<'_, Sqlite>,
    host_id: HostId,
    now: i64,
    actor_user_id: Option<&str>,
    action: &str,
) -> Result<u64, RegistryError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM host_credential_rotations
          WHERE host_id=? AND phase IN ('requested','exchanged')",
    )
    .bind(host_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    let mut closed = 0;
    for rotation_id in ids {
        let changed = sqlx::query(
            "UPDATE host_credential_rotations SET phase='revoked',revoked_at=?
              WHERE id=? AND phase IN ('requested','exchanged')",
        )
        .bind(now)
        .bind(&rotation_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if changed == 1 {
            audit_and_event(
                tx,
                actor_user_id,
                Some(host_id),
                action,
                "rotation",
                &rotation_id,
                json!({}),
                now,
            )
            .await?;
            closed += 1;
        }
    }
    Ok(closed)
}

fn validate_idempotency_key(value: &str) -> Result<(), RegistryError> {
    IdempotencyKey::parse(value.to_owned())
        .map(|_| ())
        .map_err(|_| RegistryError::InvalidRequest("idempotency key is malformed"))
}

fn hash_request(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn parse_uuid(value: &str) -> Result<Uuid, RegistryError> {
    Uuid::parse_str(value).map_err(|_| RegistryError::InvalidRequest("stored UUID is malformed"))
}

fn generation_from_i64(value: i64) -> Result<Generation, RegistryError> {
    Generation::new(
        u64::try_from(value)
            .map_err(|_| RegistryError::InvalidRequest("stored generation is malformed"))?,
    )
    .map_err(|_| RegistryError::InvalidRequest("stored generation is malformed"))
}

fn os_name(value: ExecutionOs) -> &'static str {
    match value {
        ExecutionOs::Linux => "linux",
        ExecutionOs::Macos => "macos",
        ExecutionOs::Windows => "windows",
    }
}

fn architecture_name(value: Architecture) -> &'static str {
    match value {
        Architecture::X64 => "x64",
        Architecture::Arm64 => "arm64",
    }
}
