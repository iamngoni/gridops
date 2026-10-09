//! Typed enrollment, host approval, and recovery commands.
//!
//! These values are transport-independent. They keep bounded host metadata
//! and exact target scope separate from policy grants, which are rechecked in
//! the registry transaction and are never inferred from enrollment.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::super::{
    authorization::Principal,
    domain::{Architecture, ExecutionOs},
    ids::{CiTargetId, HostId, Revision},
};
use super::credentials::OneTimeSecret;

pub(crate) const DEFAULT_ENROLLMENT_TTL_MILLIS: i64 = 10 * 60 * 1000;
pub(crate) const MAX_ENROLLMENT_TTL_MILLIS: i64 = 60 * 60 * 1000;
pub(crate) const ROTATION_OVERLAP_MILLIS: i64 = 5 * 60 * 1000;
pub(crate) const ROTATION_TTL_MILLIS: i64 = 15 * 60 * 1000;

/// An exact, immutable target scope attached to an enrollment request.
const MAX_ENROLLMENT_TARGETS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentScope {
    target_ids: Vec<CiTargetId>,
}

impl EnrollmentScope {
    /// Build a scope. An empty scope is valid and grants no target authority.
    pub fn new(
        target_ids: impl IntoIterator<Item = CiTargetId>,
    ) -> Result<Self, EnrollmentInputError> {
        let mut target_ids = target_ids.into_iter().collect::<Vec<_>>();
        if target_ids.len() > MAX_ENROLLMENT_TARGETS {
            return Err(EnrollmentInputError::ScopeTooLarge);
        }
        let original_len = target_ids.len();
        target_ids.sort_by_key(|target| target.to_string());
        target_ids.dedup();
        if target_ids.len() != original_len {
            return Err(EnrollmentInputError::DuplicateScopeTarget);
        }
        Ok(Self { target_ids })
    }

    pub fn target_ids(&self) -> impl Iterator<Item = CiTargetId> + '_ {
        self.target_ids.iter().copied()
    }
}

impl<'de> Deserialize<'de> for EnrollmentScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireScope {
            target_ids: Vec<CiTargetId>,
        }
        let wire = WireScope::deserialize(deserializer)?;
        Self::new(wire.target_ids).map_err(serde::de::Error::custom)
    }
}

/// Host metadata supplied during first enrollment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRegistration {
    name: HostName,
    host_os: ExecutionOs,
    architecture: Architecture,
    hardware_fingerprint: Option<HardwareFingerprint>,
}

impl HostRegistration {
    pub fn new(
        name: impl Into<String>,
        host_os: ExecutionOs,
        architecture: Architecture,
        hardware_fingerprint: Option<String>,
    ) -> Result<Self, EnrollmentInputError> {
        Ok(Self {
            name: HostName::new(name.into())?,
            host_os,
            architecture,
            hardware_fingerprint: hardware_fingerprint
                .map(HardwareFingerprint::new)
                .transpose()?,
        })
    }

    #[must_use]
    pub(crate) fn name(&self) -> &str {
        self.name.as_str()
    }

    #[must_use]
    pub(crate) const fn host_os(&self) -> ExecutionOs {
        self.host_os
    }

    #[must_use]
    pub(crate) const fn architecture(&self) -> Architecture {
        self.architecture
    }

    #[must_use]
    pub(crate) fn hardware_fingerprint(&self) -> Option<&str> {
        self.hardware_fingerprint
            .as_ref()
            .map(HardwareFingerprint::as_str)
    }
}

/// Browser-side administrator request for a single-use enrollment code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEnrollment {
    pub idempotency_key: String,
    pub scope: EnrollmentScope,
    pub intended_host_id: Option<HostId>,
    pub ttl_millis: Option<i64>,
}

impl IssueEnrollment {
    pub(crate) fn ttl_millis(&self) -> Result<i64, EnrollmentInputError> {
        let ttl = self.ttl_millis.unwrap_or(DEFAULT_ENROLLMENT_TTL_MILLIS);
        if !(60_000..=MAX_ENROLLMENT_TTL_MILLIS).contains(&ttl) {
            return Err(EnrollmentInputError::InvalidTtl);
        }
        Ok(ttl)
    }
}

/// Public metadata and one-time plaintext returned after issuance.
#[derive(Clone)]
pub struct IssuedEnrollment {
    pub id: uuid::Uuid,
    pub code: OneTimeSecret,
    pub expires_at: i64,
}

/// A metadata-only replay of a previously issued enrollment request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentMetadata {
    pub id: uuid::Uuid,
    pub expires_at: i64,
    pub consumed_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub consumed_host_id: Option<HostId>,
}

/// Issuance is secret-bearing only on the first committed response.
#[derive(Debug, Clone)]
pub enum EnrollmentIssue {
    Created(IssuedEnrollment),
    Replay(EnrollmentMetadata),
}

impl fmt::Debug for IssuedEnrollment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedEnrollment")
            .field("id", &self.id)
            .field("code", &"REDACTED")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Result of consuming a code. The host ID is durable; the credential is not.
#[derive(Clone)]
pub struct EnrollmentReceipt {
    pub host_id: HostId,
    pub credential_id: uuid::Uuid,
    pub credential_generation: u64,
    pub credential: OneTimeSecret,
}

impl fmt::Debug for EnrollmentReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnrollmentReceipt")
            .field("host_id", &self.host_id)
            .field("credential_id", &self.credential_id)
            .field("credential_generation", &self.credential_generation)
            .field("credential", &"REDACTED")
            .finish()
    }
}

/// Explicit recovery of a known host after an enrollment response was lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverHost {
    pub host_id: HostId,
    pub expected_revision: Revision,
    pub idempotency_key: String,
}

/// Recovery code and exact host binding returned to an administrator.
#[derive(Clone)]
pub struct IssuedRecoveryEnrollment {
    pub id: uuid::Uuid,
    pub host_id: HostId,
    pub code: OneTimeSecret,
    pub expires_at: i64,
}

impl fmt::Debug for IssuedRecoveryEnrollment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedRecoveryEnrollment")
            .field("id", &self.id)
            .field("host_id", &self.host_id)
            .field("code", &"REDACTED")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Recovery issuance is secret-bearing only on the first committed response.
#[derive(Debug, Clone)]
pub enum RecoveryIssue {
    Created(IssuedRecoveryEnrollment),
    Replay(EnrollmentMetadata),
}

/// Manual approval binds both policy revision and the inventory observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveHost {
    pub host_id: HostId,
    pub expected_revision: Revision,
    pub expected_inventory_revision: u64,
    pub expected_inventory_digest: String,
    pub scope: EnrollmentScope,
}

/// Typed errors raised before or during registry transitions.
#[derive(Debug, Error)]
pub enum EnrollmentInputError {
    #[error("host name must be between 1 and 128 characters")]
    InvalidHostName,
    #[error("host name contains control characters")]
    ControlCharacter,
    #[error("hardware fingerprint must be between 1 and 256 characters")]
    InvalidFingerprint,
    #[error("hardware fingerprint contains control characters")]
    FingerprintControlCharacter,
    #[error("enrollment lifetime must be between 1 minute and 1 hour")]
    InvalidTtl,
    #[error("enrollment target scope is too large")]
    ScopeTooLarge,
    #[error("enrollment target scope contains a duplicate")]
    DuplicateScopeTarget,
    #[error("revision or epoch is outside the supported range")]
    InvalidCounter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostName(String);

impl HostName {
    fn new(value: String) -> Result<Self, EnrollmentInputError> {
        let length = value.chars().count();
        if !(1..=128).contains(&length) {
            return Err(EnrollmentInputError::InvalidHostName);
        }
        if value.chars().any(char::is_control) {
            return Err(EnrollmentInputError::ControlCharacter);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HardwareFingerprint(String);

impl HardwareFingerprint {
    fn new(value: String) -> Result<Self, EnrollmentInputError> {
        let length = value.chars().count();
        if !(1..=256).contains(&length) {
            return Err(EnrollmentInputError::InvalidFingerprint);
        }
        if value.chars().any(char::is_control) {
            return Err(EnrollmentInputError::FingerprintControlCharacter);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// The actor used by browser-side registry transitions.
pub(crate) fn require_user(principal: &Principal) -> Result<&str, EnrollmentInputError> {
    match principal {
        Principal::AuthenticatedUser { user_id } => Ok(user_id),
        Principal::Reconciler | Principal::Autoscaler | Principal::FixAgent => {
            Err(EnrollmentInputError::InvalidCounter)
        }
    }
}
