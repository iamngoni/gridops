//! Fleet domain values, independent lifecycle axes, and legal transitions.
//!
//! This module contains no persistence or provider I/O. It keeps host/backend
//! identity, target compatibility, resource arithmetic, and ownership history
//! explicit so later database transactions can recheck these invariants.

use std::{fmt, num::NonZeroU64};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::ids::{BackendId, Generation, HostEpoch, HostId, PlacementId, UpstreamText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ExternalId(NonZeroU64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("external identifier must be between 1 and {max}, got {value}")]
pub struct ExternalIdError {
    value: u64,
    max: u64,
}

impl ExternalId {
    pub fn new(value: u64) -> Result<Self, ExternalIdError> {
        Self::try_from(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<u64> for ExternalId {
    type Error = ExternalIdError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(ExternalIdError {
                value,
                max: i64::MAX as u64,
            });
        }
        let value = NonZeroU64::new(value).ok_or(ExternalIdError {
            value,
            max: i64::MAX as u64,
        })?;
        Ok(Self(value))
    }
}

impl From<ExternalId> for u64 {
    fn from(value: ExternalId) -> Self {
        value.get()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BitbucketConnectionId(UpstreamText);

impl BitbucketConnectionId {
    pub fn new(value: impl Into<String>) -> Result<Self, super::ids::FleetTextError> {
        Ok(Self(UpstreamText::new(value)?))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for BitbucketConnectionId {
    type Error = super::ids::FleetTextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BitbucketConnectionId> for String {
    fn from(value: BitbucketConnectionId) -> Self {
        value.0.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Uuid", into = "Uuid")]
pub struct NonNilTargetUuid(Uuid);

impl NonNilTargetUuid {
    pub fn new(value: Uuid) -> Result<Self, super::ids::NilFleetUuid> {
        if value.is_nil() {
            Err(super::ids::NilFleetUuid)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl TryFrom<Uuid> for NonNilTargetUuid {
    type Error = super::ids::NilFleetUuid;

    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<NonNilTargetUuid> for Uuid {
    fn from(value: NonNilTargetUuid) -> Self {
        value.0
    }
}

/// A concrete upstream identity. Runtime kind and execution OS are separate
/// values; a target never implies a host runtime or platform support fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CiTarget {
    GithubRepository {
        installation_id: ExternalId,
        repository_id: ExternalId,
    },
    GithubOrganization {
        installation_id: ExternalId,
        organization_id: ExternalId,
    },
    BitbucketWorkspace {
        connection_id: BitbucketConnectionId,
        workspace_uuid: NonNilTargetUuid,
    },
    BitbucketRepository {
        connection_id: BitbucketConnectionId,
        workspace_uuid: NonNilTargetUuid,
        repository_uuid: NonNilTargetUuid,
    },
}

impl CiTarget {
    #[must_use]
    pub const fn is_github(&self) -> bool {
        matches!(
            self,
            Self::GithubRepository { .. } | Self::GithubOrganization { .. }
        )
    }

    #[must_use]
    pub const fn is_bitbucket(&self) -> bool {
        !self.is_github()
    }

    /// Stable, non-secret scope key used for authorization comparisons and
    /// audit correlation. It does not include credentials or mutable labels.
    #[must_use]
    pub fn canonical_key(&self) -> String {
        match self {
            Self::GithubRepository {
                installation_id,
                repository_id,
            } => format!(
                "github:repository:{}:{}",
                installation_id.get(),
                repository_id.get()
            ),
            Self::GithubOrganization {
                installation_id,
                organization_id,
            } => format!(
                "github:organization:{}:{}",
                installation_id.get(),
                organization_id.get()
            ),
            Self::BitbucketWorkspace {
                connection_id,
                workspace_uuid,
            } => format!(
                "bitbucket:workspace:{}:{}",
                connection_id.as_str(),
                workspace_uuid.as_uuid()
            ),
            Self::BitbucketRepository {
                connection_id,
                workspace_uuid,
                repository_uuid,
            } => format!(
                "bitbucket:repository:{}:{}:{}",
                connection_id.as_str(),
                workspace_uuid.as_uuid(),
                repository_uuid.as_uuid()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Docker,
    TartVm,
    NativeProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOs {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X64,
    Arm64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiPlatform {
    Github,
    Bitbucket,
}

pub const MAX_CPU_MILLIS: u32 = 4_000_000;
pub const MAX_MEMORY_MIB: u64 = 4 * 1024 * 1024;
pub const MAX_DISK_BYTES: u64 = 64 * 1024 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ResourceError {
    #[error("CPU request must be positive and at most {MAX_CPU_MILLIS} milli-CPUs")]
    CpuOutOfRange,
    #[error("memory request must be positive and at most {MAX_MEMORY_MIB} MiB")]
    MemoryOutOfRange,
    #[error("disk request must be at most {MAX_DISK_BYTES} bytes")]
    DiskOutOfRange,
    #[error("resource amount must be at most {max} units")]
    AmountOutOfRange { max: u64 },
    #[error("resource arithmetic underflowed")]
    Underflow,
    #[error("resource arithmetic overflowed")]
    Overflow,
}

/// Integer resource units used by admission. No floating-point values cross
/// this domain boundary; CPU and memory are positive for every workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawResourceRequest", into = "RawResourceRequest")]
pub struct ResourceRequest {
    cpu_millis: u32,
    memory_mib: u64,
    disk_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct RawResourceRequest {
    cpu_millis: u32,
    memory_mib: u64,
    disk_bytes: u64,
}

impl TryFrom<RawResourceRequest> for ResourceRequest {
    type Error = ResourceError;

    fn try_from(value: RawResourceRequest) -> Result<Self, Self::Error> {
        Self::new(value.cpu_millis, value.memory_mib, value.disk_bytes)
    }
}

impl From<ResourceRequest> for RawResourceRequest {
    fn from(value: ResourceRequest) -> Self {
        Self {
            cpu_millis: value.cpu_millis,
            memory_mib: value.memory_mib,
            disk_bytes: value.disk_bytes,
        }
    }
}

impl ResourceRequest {
    pub fn new(cpu_millis: u32, memory_mib: u64, disk_bytes: u64) -> Result<Self, ResourceError> {
        if cpu_millis == 0 || cpu_millis > MAX_CPU_MILLIS {
            return Err(ResourceError::CpuOutOfRange);
        }
        if memory_mib == 0 || memory_mib > MAX_MEMORY_MIB {
            return Err(ResourceError::MemoryOutOfRange);
        }
        if disk_bytes > MAX_DISK_BYTES {
            return Err(ResourceError::DiskOutOfRange);
        }
        Ok(Self {
            cpu_millis,
            memory_mib,
            disk_bytes,
        })
    }

    #[must_use]
    pub const fn cpu_millis(self) -> u32 {
        self.cpu_millis
    }

    #[must_use]
    pub const fn memory_mib(self) -> u64 {
        self.memory_mib
    }

    #[must_use]
    pub const fn disk_bytes(self) -> u64 {
        self.disk_bytes
    }

    pub fn checked_add(self, other: Self) -> Result<Self, ResourceError> {
        Self::new(
            self.cpu_millis
                .checked_add(other.cpu_millis)
                .ok_or(ResourceError::Overflow)?,
            self.memory_mib
                .checked_add(other.memory_mib)
                .ok_or(ResourceError::Overflow)?,
            self.disk_bytes
                .checked_add(other.disk_bytes)
                .ok_or(ResourceError::Overflow)?,
        )
    }
}

/// Non-negative aggregate resource usage/capacity. Unlike a workload request,
/// an empty aggregate is valid and represented by [`ResourceAmount::ZERO`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawResourceAmount", into = "RawResourceAmount")]
pub struct ResourceAmount {
    cpu_millis: u64,
    memory_mib: u64,
    disk_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct RawResourceAmount {
    cpu_millis: u64,
    memory_mib: u64,
    disk_bytes: u64,
}

impl TryFrom<RawResourceAmount> for ResourceAmount {
    type Error = ResourceError;

    fn try_from(value: RawResourceAmount) -> Result<Self, Self::Error> {
        Self::new(value.cpu_millis, value.memory_mib, value.disk_bytes)
    }
}

impl From<ResourceAmount> for RawResourceAmount {
    fn from(value: ResourceAmount) -> Self {
        Self {
            cpu_millis: value.cpu_millis,
            memory_mib: value.memory_mib,
            disk_bytes: value.disk_bytes,
        }
    }
}

impl ResourceAmount {
    pub const ZERO: Self = Self {
        cpu_millis: 0,
        memory_mib: 0,
        disk_bytes: 0,
    };

    pub fn new(cpu_millis: u64, memory_mib: u64, disk_bytes: u64) -> Result<Self, ResourceError> {
        for value in [cpu_millis, memory_mib, disk_bytes] {
            if value > i64::MAX as u64 {
                return Err(ResourceError::AmountOutOfRange {
                    max: i64::MAX as u64,
                });
            }
        }
        Ok(Self {
            cpu_millis,
            memory_mib,
            disk_bytes,
        })
    }

    #[must_use]
    pub const fn cpu_millis(self) -> u64 {
        self.cpu_millis
    }

    #[must_use]
    pub const fn memory_mib(self) -> u64 {
        self.memory_mib
    }

    #[must_use]
    pub const fn disk_bytes(self) -> u64 {
        self.disk_bytes
    }

    pub fn from_request(request: ResourceRequest) -> Result<Self, ResourceError> {
        Self::new(
            u64::from(request.cpu_millis()),
            request.memory_mib(),
            request.disk_bytes(),
        )
    }

    pub fn checked_add(self, other: Self) -> Result<Self, ResourceError> {
        Self::new(
            self.cpu_millis
                .checked_add(other.cpu_millis)
                .ok_or(ResourceError::Overflow)?,
            self.memory_mib
                .checked_add(other.memory_mib)
                .ok_or(ResourceError::Overflow)?,
            self.disk_bytes
                .checked_add(other.disk_bytes)
                .ok_or(ResourceError::Overflow)?,
        )
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, ResourceError> {
        Self::new(
            self.cpu_millis
                .checked_sub(other.cpu_millis)
                .ok_or(ResourceError::Underflow)?,
            self.memory_mib
                .checked_sub(other.memory_mib)
                .ok_or(ResourceError::Underflow)?,
            self.disk_bytes
                .checked_sub(other.disk_bytes)
                .ok_or(ResourceError::Underflow)?,
        )
    }

    #[must_use]
    pub const fn fits(self, request: ResourceRequest) -> bool {
        self.cpu_millis >= request.cpu_millis() as u64
            && self.memory_mib >= request.memory_mib()
            && self.disk_bytes >= request.disk_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentState {
    Pending,
    Approved,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostIntent {
    Active,
    Paused,
    Draining,
    Maintenance,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale,
    Offline,
    Unknown,
}

/// Enrollment alone does not establish a reconciled, trusted host identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityState {
    Unverified,
    Verified,
    Quarantined,
}

/// Missing pressure measurements block admission rather than implying zero use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PressureState {
    Normal,
    Pressured,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendReadiness {
    Ready,
    Reconciling,
    Unavailable,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAxis {
    Enrollment,
    HostIntent,
    Freshness,
    BackendReadiness,
    Placement,
    Allocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("illegal {axis:?} transition from {from} to {to}")]
pub struct LifecycleTransitionError {
    axis: LifecycleAxis,
    from: String,
    to: String,
}

impl LifecycleTransitionError {
    fn new(axis: LifecycleAxis, from: impl fmt::Debug, to: impl fmt::Debug) -> Self {
        Self {
            axis,
            from: format!("{from:?}"),
            to: format!("{to:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawHostLifecycle", into = "RawHostLifecycle")]
pub struct HostLifecycle {
    enrollment: EnrollmentState,
    intent: HostIntent,
    freshness: Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct RawHostLifecycle {
    enrollment: EnrollmentState,
    intent: HostIntent,
    freshness: Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid host lifecycle combination")]
pub struct LifecycleValidationError;

impl TryFrom<RawHostLifecycle> for HostLifecycle {
    type Error = LifecycleValidationError;

    fn try_from(value: RawHostLifecycle) -> Result<Self, Self::Error> {
        if value.intent == HostIntent::Active && value.enrollment != EnrollmentState::Approved {
            return Err(LifecycleValidationError);
        }
        Ok(Self {
            enrollment: value.enrollment,
            intent: value.intent,
            freshness: value.freshness,
        })
    }
}

impl From<HostLifecycle> for RawHostLifecycle {
    fn from(value: HostLifecycle) -> Self {
        Self {
            enrollment: value.enrollment,
            intent: value.intent,
            freshness: value.freshness,
        }
    }
}

impl HostLifecycle {
    #[must_use]
    pub const fn pending() -> Self {
        Self {
            enrollment: EnrollmentState::Pending,
            intent: HostIntent::Paused,
            freshness: Freshness::Unknown,
        }
    }

    #[must_use]
    pub const fn enrollment(self) -> EnrollmentState {
        self.enrollment
    }

    #[must_use]
    pub const fn intent(self) -> HostIntent {
        self.intent
    }

    #[must_use]
    pub const fn freshness(self) -> Freshness {
        self.freshness
    }

    pub fn transition_enrollment(
        &mut self,
        next: EnrollmentState,
    ) -> Result<(), LifecycleTransitionError> {
        let legal = matches!(
            (self.enrollment, next),
            (
                EnrollmentState::Pending,
                EnrollmentState::Pending | EnrollmentState::Approved | EnrollmentState::Revoked
            ) | (
                EnrollmentState::Approved,
                EnrollmentState::Approved | EnrollmentState::Revoked
            ) | (
                EnrollmentState::Revoked,
                EnrollmentState::Revoked | EnrollmentState::Pending
            )
        );
        if !legal {
            return Err(LifecycleTransitionError::new(
                LifecycleAxis::Enrollment,
                self.enrollment,
                next,
            ));
        }
        if next == EnrollmentState::Revoked && self.intent == HostIntent::Active {
            return Err(LifecycleTransitionError::new(
                LifecycleAxis::Enrollment,
                self.enrollment,
                next,
            ));
        }
        self.enrollment = next;
        Ok(())
    }

    #[allow(clippy::match_like_matches_macro)]
    pub fn transition_intent(&mut self, next: HostIntent) -> Result<(), LifecycleTransitionError> {
        if next == HostIntent::Active && self.enrollment != EnrollmentState::Approved {
            return Err(LifecycleTransitionError::new(
                LifecycleAxis::HostIntent,
                self.intent,
                next,
            ));
        }
        let legal = match (self.intent, next) {
            (
                HostIntent::Active,
                HostIntent::Active
                | HostIntent::Paused
                | HostIntent::Draining
                | HostIntent::Maintenance
                | HostIntent::Retired,
            ) => true,
            (
                HostIntent::Paused,
                HostIntent::Paused
                | HostIntent::Active
                | HostIntent::Draining
                | HostIntent::Maintenance
                | HostIntent::Retired,
            ) => true,
            (
                HostIntent::Draining,
                HostIntent::Draining
                | HostIntent::Active
                | HostIntent::Paused
                | HostIntent::Maintenance
                | HostIntent::Retired,
            ) => true,
            (
                HostIntent::Maintenance,
                HostIntent::Maintenance
                | HostIntent::Active
                | HostIntent::Paused
                | HostIntent::Draining
                | HostIntent::Retired,
            ) => true,
            (HostIntent::Retired, HostIntent::Retired) => true,
            _ => false,
        };
        if !legal {
            return Err(LifecycleTransitionError::new(
                LifecycleAxis::HostIntent,
                self.intent,
                next,
            ));
        }
        self.intent = next;
        Ok(())
    }

    #[allow(clippy::match_like_matches_macro)]
    pub fn transition_freshness(
        &mut self,
        next: Freshness,
    ) -> Result<(), LifecycleTransitionError> {
        let legal = match (self.freshness, next) {
            (
                Freshness::Unknown,
                Freshness::Unknown | Freshness::Fresh | Freshness::Stale | Freshness::Offline,
            ) => true,
            (Freshness::Fresh, Freshness::Fresh | Freshness::Stale | Freshness::Offline) => true,
            (Freshness::Stale, Freshness::Stale | Freshness::Fresh | Freshness::Offline) => true,
            (Freshness::Offline, Freshness::Offline | Freshness::Fresh | Freshness::Unknown) => {
                true
            }
            _ => false,
        };
        if !legal {
            return Err(LifecycleTransitionError::new(
                LifecycleAxis::Freshness,
                self.freshness,
                next,
            ));
        }
        self.freshness = next;
        Ok(())
    }
}

impl BackendReadiness {
    #[allow(clippy::match_like_matches_macro)]
    pub fn transition(self, next: Self) -> Result<Self, LifecycleTransitionError> {
        let legal = match (self, next) {
            (Self::Ready, Self::Ready | Self::Reconciling | Self::Unavailable | Self::Unknown) => {
                true
            }
            (
                Self::Reconciling,
                Self::Reconciling
                | Self::Ready
                | Self::Unavailable
                | Self::Unsupported
                | Self::Unknown,
            ) => true,
            (
                Self::Unavailable,
                Self::Unavailable | Self::Ready | Self::Reconciling | Self::Unknown,
            ) => true,
            (Self::Unsupported, Self::Unsupported | Self::Reconciling | Self::Unknown) => true,
            (
                Self::Unknown,
                Self::Unknown
                | Self::Reconciling
                | Self::Ready
                | Self::Unavailable
                | Self::Unsupported,
            ) => true,
            _ => false,
        };
        if legal {
            Ok(next)
        } else {
            Err(LifecycleTransitionError::new(
                LifecycleAxis::BackendReadiness,
                self,
                next,
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementState {
    Reserved,
    Preparing,
    Registering,
    Starting,
    Running,
    Draining,
    Stopping,
    Stopped,
    Cleaned,
    #[serde(rename = "retryable")]
    RetryableFailure,
    #[serde(rename = "failed")]
    TerminalFailure,
    Uncertain,
}

impl PlacementState {
    #[allow(clippy::match_like_matches_macro)]
    pub fn transition(self, next: Self) -> Result<Self, LifecycleTransitionError> {
        let legal = match (self, next) {
            (
                Self::Reserved,
                Self::Reserved | Self::Preparing | Self::RetryableFailure | Self::Uncertain,
            ) => true,
            (
                Self::Preparing,
                Self::Preparing
                | Self::Registering
                | Self::RetryableFailure
                | Self::TerminalFailure
                | Self::Uncertain,
            ) => true,
            (
                Self::Registering,
                Self::Registering
                | Self::Starting
                | Self::RetryableFailure
                | Self::TerminalFailure
                | Self::Uncertain,
            ) => true,
            (
                Self::Starting,
                Self::Starting
                | Self::Running
                | Self::RetryableFailure
                | Self::TerminalFailure
                | Self::Uncertain,
            ) => true,
            (Self::Running, Self::Running | Self::Draining | Self::Stopping | Self::Uncertain) => {
                true
            }
            (Self::Draining, Self::Draining | Self::Stopping | Self::Stopped | Self::Uncertain) => {
                true
            }
            (
                Self::Stopping,
                Self::Stopping
                | Self::Stopped
                | Self::Cleaned
                | Self::RetryableFailure
                | Self::Uncertain,
            ) => true,
            (Self::Stopped, Self::Stopped | Self::Cleaned | Self::Uncertain) => true,
            (Self::Cleaned, Self::Cleaned) => true,
            (
                Self::RetryableFailure,
                Self::RetryableFailure | Self::Reserved | Self::TerminalFailure | Self::Uncertain,
            ) => true,
            (Self::TerminalFailure, Self::TerminalFailure | Self::Cleaned) => true,
            (
                Self::Uncertain,
                Self::Uncertain
                | Self::RetryableFailure
                | Self::TerminalFailure
                | Self::Running
                | Self::Stopped
                | Self::Cleaned,
            ) => true,
            _ => false,
        };
        if legal {
            Ok(next)
        } else {
            Err(LifecycleTransitionError::new(
                LifecycleAxis::Placement,
                self,
                next,
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationState {
    Reserved,
    Committed,
    Releasing,
    Uncertain,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCleanupState {
    NotRegistered,
    Pending,
    Uncertain,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("allocation cannot be released without proven local absence and delayed-start exclusion")]
pub struct AllocationReleaseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvenLocalAbsence {
    placement_id: PlacementId,
    observed_generation: Generation,
    observed_epoch: HostEpoch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayedStartExclusion {
    placement_id: PlacementId,
    generation: Generation,
    host_epoch: HostEpoch,
}

impl ProvenLocalAbsence {
    #[allow(dead_code)]
    pub(crate) const fn new(
        placement_id: PlacementId,
        observed_generation: Generation,
        observed_epoch: HostEpoch,
    ) -> Self {
        Self {
            placement_id,
            observed_generation,
            observed_epoch,
        }
    }
}

impl DelayedStartExclusion {
    #[allow(dead_code)]
    pub(crate) const fn new(
        placement_id: PlacementId,
        generation: Generation,
        host_epoch: HostEpoch,
    ) -> Self {
        Self {
            placement_id,
            generation,
            host_epoch,
        }
    }
}

impl AllocationState {
    #[allow(clippy::unnested_or_patterns)]
    pub fn transition(self, next: Self) -> Result<Self, LifecycleTransitionError> {
        let legal = matches!(
            (self, next),
            (
                Self::Reserved,
                Self::Reserved | Self::Committed | Self::Uncertain
            ) | (
                Self::Committed,
                Self::Committed | Self::Releasing | Self::Uncertain
            ) | (Self::Releasing, Self::Releasing | Self::Uncertain)
                | (Self::Uncertain, Self::Uncertain | Self::Releasing)
                | (Self::Released, Self::Released)
        );
        if legal {
            Ok(next)
        } else {
            Err(LifecycleTransitionError::new(
                LifecycleAxis::Allocation,
                self,
                next,
            ))
        }
    }

    pub fn release(
        self,
        expected_placement_id: PlacementId,
        expected_generation: Generation,
        expected_epoch: HostEpoch,
        local_absence: &ProvenLocalAbsence,
        delayed_start: &DelayedStartExclusion,
    ) -> Result<Self, AllocationReleaseError> {
        if local_absence.placement_id != expected_placement_id
            || delayed_start.placement_id != expected_placement_id
            || local_absence.observed_generation != expected_generation
            || delayed_start.generation != expected_generation
            || local_absence.observed_epoch != expected_epoch
            || delayed_start.host_epoch != expected_epoch
        {
            return Err(AllocationReleaseError);
        }
        match self {
            Self::Reserved | Self::Committed | Self::Releasing | Self::Uncertain => {
                Ok(Self::Released)
            }
            Self::Released => Err(AllocationReleaseError),
        }
    }
}

/// A placement's original authority epoch is immutable. Recovery creates a
/// new ownership value for a new generation rather than rewriting history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementOwnership {
    host_id: HostId,
    backend_id: BackendId,
    original_epoch: HostEpoch,
    generation: Generation,
}

impl PlacementOwnership {
    pub const fn new(
        host_id: HostId,
        backend_id: BackendId,
        original_epoch: HostEpoch,
        generation: Generation,
    ) -> Self {
        Self {
            host_id,
            backend_id,
            original_epoch,
            generation,
        }
    }

    #[must_use]
    pub const fn host_id(self) -> HostId {
        self.host_id
    }

    #[must_use]
    pub const fn backend_id(self) -> BackendId {
        self.backend_id
    }

    #[must_use]
    pub const fn original_epoch(self) -> HostEpoch {
        self.original_epoch
    }

    #[must_use]
    pub const fn generation(self) -> Generation {
        self.generation
    }

    #[must_use]
    pub const fn recovered(self, new_epoch: HostEpoch, new_generation: Generation) -> Self {
        Self {
            host_id: self.host_id,
            backend_id: self.backend_id,
            original_epoch: new_epoch,
            generation: new_generation,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fleet::ids::{AllocationId, CommandId};

    fn id<T>() -> T
    where
        T: TryFrom<Uuid, Error = super::super::ids::NilFleetUuid>,
    {
        T::try_from(Uuid::new_v4()).expect("test UUID is non-nil")
    }

    #[test]
    fn targets_require_concrete_non_nil_identity() {
        let target = CiTarget::BitbucketRepository {
            connection_id: BitbucketConnectionId::new("connection").expect("valid connection"),
            workspace_uuid: NonNilTargetUuid::new(Uuid::new_v4()).expect("valid workspace"),
            repository_uuid: NonNilTargetUuid::new(Uuid::new_v4()).expect("valid repository"),
        };
        assert!(
            target
                .canonical_key()
                .starts_with("bitbucket:repository:connection:")
        );
        assert!(serde_json::from_str::<CiTarget>(
            r#"{"platform":"bitbucket_repository","connection_id":"connection","workspace_uuid":"00000000-0000-0000-0000-000000000000","repository_uuid":"d2719c14-6bb3-49f3-8a9b-ece6a3b6e3b8"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<CiTarget>(
            r#"{"platform":"bitbucket_workspace","connection_id":"connection","workspace_uuid":"d2719c14-6bb3-49f3-8a9b-ece6a3b6e3b8","repository_uuid":"d2719c14-6bb3-49f3-8a9b-ece6a3b6e3b8"}"#
        )
        .is_err());
    }

    #[test]
    fn resources_are_integer_and_checked() {
        let request = ResourceRequest::new(500, 1024, 4096).expect("valid request");
        assert_eq!(
            request
                .checked_add(request)
                .expect("no overflow")
                .cpu_millis(),
            1000
        );
        assert!(ResourceRequest::new(0, 1024, 0).is_err());
        assert!(ResourceRequest::new(MAX_CPU_MILLIS + 1, 1, 0).is_err());
        assert!(
            serde_json::from_str::<ResourceRequest>(
                r#"{"cpu_millis":0,"memory_mib":1,"disk_bytes":0}"#
            )
            .is_err()
        );
        let amount = ResourceAmount::ZERO;
        let one = ResourceAmount::from_request(request).expect("request fits aggregate units");
        assert!(amount.checked_add(one).is_ok());
        assert!(amount.checked_sub(one).is_err());
        assert!(one.fits(request));
        assert!(
            serde_json::from_str::<ResourceAmount>(
                r#"{"cpu_millis":9223372036854775808,"memory_mib":0,"disk_bytes":0}"#
            )
            .is_err()
        );
    }

    #[test]
    fn lifecycle_axes_reject_illegal_transitions_independently() {
        let mut lifecycle = HostLifecycle::pending();
        assert!(lifecycle.transition_intent(HostIntent::Active).is_err());
        lifecycle
            .transition_enrollment(EnrollmentState::Approved)
            .expect("pending host can be approved");
        lifecycle
            .transition_intent(HostIntent::Active)
            .expect("approved host can activate");
        assert!(
            lifecycle
                .transition_enrollment(EnrollmentState::Revoked)
                .is_err()
        );
        lifecycle
            .transition_intent(HostIntent::Draining)
            .expect("active host can drain");
        lifecycle
            .transition_intent(HostIntent::Paused)
            .expect("draining host can resume paused");
        assert!(
            lifecycle
                .transition_enrollment(EnrollmentState::Revoked)
                .is_ok()
        );
        assert!(
            serde_json::from_str::<HostLifecycle>(
                r#"{"enrollment":"pending","intent":"active","freshness":"unknown"}"#
            )
            .is_err()
        );
        assert!(
            BackendReadiness::Unknown
                .transition(BackendReadiness::Ready)
                .is_ok()
        );
    }

    #[test]
    fn allocation_release_requires_both_proofs() {
        let placement_id: PlacementId = id();
        let generation = Generation::new(1).expect("valid generation");
        let epoch = HostEpoch::new(1).expect("valid epoch");
        let local = ProvenLocalAbsence::new(placement_id, generation, epoch);
        let delayed = DelayedStartExclusion::new(placement_id, generation, epoch);
        assert_eq!(
            AllocationState::Uncertain.release(placement_id, generation, epoch, &local, &delayed),
            Ok(AllocationState::Released)
        );
        let other: PlacementId = id();
        assert!(
            AllocationState::Releasing
                .release(
                    placement_id,
                    generation,
                    epoch,
                    &local,
                    &DelayedStartExclusion::new(other, generation, epoch),
                )
                .is_err()
        );
        assert!(
            AllocationState::Releasing
                .transition(AllocationState::Released)
                .is_err()
        );
        assert!(
            AllocationState::Reserved
                .release(placement_id, generation, epoch, &local, &delayed)
                .is_ok()
        );
        let next_generation = Generation::new(2).expect("valid generation");
        let next_epoch = HostEpoch::new(2).expect("valid epoch");
        for (observed_generation, observed_epoch) in
            [(next_generation, epoch), (generation, next_epoch)]
        {
            let wrong_local =
                ProvenLocalAbsence::new(placement_id, observed_generation, observed_epoch);
            let wrong_delayed =
                DelayedStartExclusion::new(placement_id, observed_generation, observed_epoch);
            assert!(
                AllocationState::Uncertain
                    .release(placement_id, generation, epoch, &wrong_local, &delayed)
                    .is_err()
            );
            assert!(
                AllocationState::Uncertain
                    .release(placement_id, generation, epoch, &local, &wrong_delayed)
                    .is_err()
            );
        }
    }

    #[test]
    fn recovery_keeps_old_epoch_immutable() {
        let old_epoch = HostEpoch::new(3).expect("valid epoch");
        let new_epoch = HostEpoch::new(4).expect("valid epoch");
        let placement = PlacementOwnership::new(
            id(),
            id(),
            old_epoch,
            Generation::new(1).expect("valid generation"),
        );
        let recovered =
            placement.recovered(new_epoch, Generation::new(2).expect("valid generation"));
        assert_eq!(placement.original_epoch(), old_epoch);
        assert_eq!(recovered.original_epoch(), new_epoch);
        assert_ne!(placement.generation(), recovered.generation());
    }

    #[test]
    fn typed_ids_remain_distinct() {
        let _allocation: AllocationId = id();
        let _command: CommandId = id();
    }

    #[test]
    fn integrity_and_pressure_preserve_unknown_states() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::from_str::<IntegrityState>("\"unverified\"")?,
            IntegrityState::Unverified
        );
        assert_eq!(
            serde_json::from_str::<IntegrityState>("\"verified\"")?,
            IntegrityState::Verified
        );
        assert_eq!(
            serde_json::from_str::<IntegrityState>("\"quarantined\"")?,
            IntegrityState::Quarantined
        );
        assert_eq!(
            serde_json::from_str::<PressureState>("\"normal\"")?,
            PressureState::Normal
        );
        assert_eq!(
            serde_json::from_str::<PressureState>("\"pressured\"")?,
            PressureState::Pressured
        );
        assert_eq!(
            serde_json::from_str::<PressureState>("\"unknown\"")?,
            PressureState::Unknown
        );
        assert!(serde_json::from_str::<IntegrityState>("\"healthy\"").is_err());
        assert!(serde_json::from_str::<PressureState>("\"idle\"").is_err());
        Ok(())
    }
}
