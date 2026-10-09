//! Scoped authorization values for fleet operations.
//!
//! A [`GrantSnapshot`] is the immutable row read by an authorization check.
//! Proofs can only be minted through its validation methods, which keeps
//! callers from manufacturing capability-bearing values. The database layer
//! can compare a proof with a fresh snapshot in the same admission transaction
//! using [`GrantSnapshot::recheck`].

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    domain::{CiTarget, ExecutionOs, RuntimeKind},
    ids::{Generation, HostId, PlacementId, Revision, SessionId, WorkloadId},
};

/// The actor represented by a fleet grant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum Principal {
    AuthenticatedUser { user_id: String },
    Reconciler,
    Autoscaler,
    FixAgent,
}

/// A named operation that may be authorized independently of other powers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadHost,
    ReadWorkload,
    EnrollHost,
    Schedule,
    DrainHost,
    MaintainHost,
    RetireHost,
    RevokeHost,
    NativeExecution,
    InteractiveSession,
    DockerSocket,
    ExecuteSandbox,
}

/// A durable, immutable view of one grant row.
///
/// The fields are private intentionally. Database/authentication code creates
/// snapshots through the crate-private trusted constructor, while callers
/// obtain proofs only after the checks below have succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantSnapshot {
    principal: Principal,
    host_id: HostId,
    target: Option<CiTarget>,
    capabilities: BTreeSet<Capability>,
    revision: Revision,
    expires_at_millis: i64,
    revoked: bool,
    session: Option<SessionGrantSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGrantSnapshot {
    session_id: SessionId,
    host_id: HostId,
    target: CiTarget,
    revision: Revision,
    target_grant_revision: Revision,
    ready: bool,
    authorized: bool,
}

impl SessionGrantSnapshot {
    #[allow(dead_code)]
    pub(crate) const fn trusted(
        session_id: SessionId,
        host_id: HostId,
        target: CiTarget,
        revision: Revision,
        target_grant_revision: Revision,
        ready: bool,
        authorized: bool,
    ) -> Self {
        Self {
            session_id,
            host_id,
            target,
            revision,
            target_grant_revision,
            ready,
            authorized,
        }
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.ready && self.authorized
    }
}

impl GrantSnapshot {
    /// Construct a snapshot from a trusted persisted grant row.
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub(crate) fn trusted(
        principal: Principal,
        host_id: HostId,
        target: Option<CiTarget>,
        capabilities: impl IntoIterator<Item = Capability>,
        revision: Revision,
        expires_at_millis: i64,
        revoked: bool,
    ) -> Self {
        Self::trusted_with_session(
            principal,
            host_id,
            target,
            capabilities,
            revision,
            expires_at_millis,
            revoked,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub(crate) fn trusted_with_session(
        principal: Principal,
        host_id: HostId,
        target: Option<CiTarget>,
        capabilities: impl IntoIterator<Item = Capability>,
        revision: Revision,
        expires_at_millis: i64,
        revoked: bool,
        session: Option<SessionGrantSnapshot>,
    ) -> Self {
        Self {
            principal,
            host_id,
            target,
            capabilities: capabilities.into_iter().collect(),
            revision,
            expires_at_millis,
            revoked,
            session,
        }
    }

    #[must_use]
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    #[must_use]
    pub fn target(&self) -> Option<&CiTarget> {
        self.target.as_ref()
    }

    pub fn capabilities(&self) -> impl Iterator<Item = Capability> + '_ {
        self.capabilities.iter().copied()
    }

    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    #[must_use]
    pub const fn expires_at_millis(&self) -> i64 {
        self.expires_at_millis
    }

    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        self.revoked
    }

    /// Validate a general capability request against this snapshot.
    pub fn authorize(
        &self,
        host_id: HostId,
        target: Option<&CiTarget>,
        capability: Capability,
        now_millis: i64,
    ) -> Result<AuthorizationProof, AuthorizationError> {
        self.validate(host_id, target, capability, now_millis)?;
        Ok(AuthorizationProof {
            principal: self.principal.clone(),
            host_id,
            target: target.cloned(),
            capability,
            revision: self.revision,
            expires_at_millis: self.expires_at_millis,
            session: None,
        })
    }

    /// Authorize a target-scoped native execution.
    pub fn authorize_native(
        &self,
        host_id: HostId,
        target: &CiTarget,
        now_millis: i64,
    ) -> Result<NativeExecutionProof, AuthorizationError> {
        Ok(NativeExecutionProof(self.authorize(
            host_id,
            Some(target),
            Capability::NativeExecution,
            now_millis,
        )?))
    }

    /// Authorize a target-scoped interactive execution in one ready session.
    pub fn authorize_interactive(
        &self,
        host_id: HostId,
        target: &CiTarget,
        session: &SessionGrantSnapshot,
        now_millis: i64,
    ) -> Result<InteractiveSessionProof, AuthorizationError> {
        if !session.is_ready() {
            return Err(AuthorizationError::SessionUnavailable);
        }
        if self.session.as_ref() != Some(session)
            || session.host_id != host_id
            || &session.target != target
        {
            return Err(AuthorizationError::SessionMismatch);
        }
        let mut proof = self.authorize(
            host_id,
            Some(target),
            Capability::InteractiveSession,
            now_millis,
        )?;
        proof.session = Some(session.clone());
        Ok(InteractiveSessionProof {
            proof,
            session_id: session.session_id,
        })
    }

    /// Authorize an explicitly configured privileged Docker socket share.
    pub fn authorize_docker_socket(
        &self,
        host_id: HostId,
        target: &CiTarget,
        now_millis: i64,
    ) -> Result<DockerSocketProof, AuthorizationError> {
        Ok(DockerSocketProof(self.authorize(
            host_id,
            Some(target),
            Capability::DockerSocket,
            now_millis,
        )?))
    }

    /// Authorize sandbox execution only for an owned Linux Docker fix placement.
    pub fn authorize_sandbox(
        &self,
        placement: &OwnedFixPlacement,
        now_millis: i64,
    ) -> Result<ExecuteSandboxProof, AuthorizationError> {
        if placement.runtime != RuntimeKind::Docker || placement.execution_os != ExecutionOs::Linux
        {
            return Err(AuthorizationError::UnsupportedSandbox);
        }
        if placement.host_id != self.host_id {
            return Err(AuthorizationError::WrongHost);
        }
        if self.target.as_ref() != Some(&placement.target) {
            return Err(AuthorizationError::TargetMismatch);
        }
        Ok(ExecuteSandboxProof {
            proof: self.authorize(
                placement.host_id,
                Some(&placement.target),
                Capability::ExecuteSandbox,
                now_millis,
            )?,
            placement: placement.clone(),
        })
    }

    /// Recheck a previously minted proof against the current grant snapshot.
    ///
    /// This is intended for the admission transaction. Revocation, expiry,
    /// scope, capability, and revision changes therefore invalidate the proof.
    pub fn recheck(
        &self,
        proof: &AuthorizationProof,
        now_millis: i64,
    ) -> Result<(), AuthorizationError> {
        self.validate(
            proof.host_id,
            proof.target.as_ref(),
            proof.capability,
            now_millis,
        )?;
        if proof.principal != self.principal
            || proof.revision != self.revision
            || proof.expires_at_millis != self.expires_at_millis
        {
            return Err(AuthorizationError::StaleProof);
        }
        if let Some(expected) = &proof.session {
            let Some(session) = self.session.as_ref() else {
                return Err(AuthorizationError::SessionMismatch);
            };
            if !session.is_ready()
                || session != expected
                || session.host_id != proof.host_id
                || Some(&session.target) != proof.target.as_ref()
            {
                return Err(AuthorizationError::SessionMismatch);
            }
        }
        Ok(())
    }

    fn validate(
        &self,
        host_id: HostId,
        target: Option<&CiTarget>,
        capability: Capability,
        now_millis: i64,
    ) -> Result<(), AuthorizationError> {
        if self.revoked {
            return Err(AuthorizationError::Revoked);
        }
        if now_millis >= self.expires_at_millis {
            return Err(AuthorizationError::Expired);
        }
        if self.host_id != host_id {
            return Err(AuthorizationError::WrongHost);
        }
        if self.target.as_ref() != target {
            return Err(AuthorizationError::TargetMismatch);
        }
        if !self.capabilities.contains(&capability) {
            return Err(AuthorizationError::MissingCapability);
        }
        Ok(())
    }
}

/// A non-sensitive explanation for a failed authorization check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AuthorizationError {
    #[error("authorization grant is revoked")]
    Revoked,
    #[error("authorization grant has expired")]
    Expired,
    #[error("authorization grant does not cover this host")]
    WrongHost,
    #[error("authorization grant does not cover this target")]
    TargetMismatch,
    #[error("authorization grant lacks the requested capability")]
    MissingCapability,
    #[error("authorization proof is stale")]
    StaleProof,
    #[error("authorized user session is unavailable")]
    SessionUnavailable,
    #[error("authorization grant does not cover this session")]
    SessionMismatch,
    #[error("sandbox execution requires an owned Linux Docker fix placement")]
    UnsupportedSandbox,
}

/// A validated, capability-bearing proof for a general fleet operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationProof {
    principal: Principal,
    host_id: HostId,
    target: Option<CiTarget>,
    capability: Capability,
    revision: Revision,
    expires_at_millis: i64,
    session: Option<SessionGrantSnapshot>,
}

impl AuthorizationProof {
    #[must_use]
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    #[must_use]
    pub fn target(&self) -> Option<&CiTarget> {
        self.target.as_ref()
    }

    #[must_use]
    pub const fn capability(&self) -> Capability {
        self.capability
    }

    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    #[must_use]
    pub const fn is_expired_at(&self, now_millis: i64) -> bool {
        now_millis >= self.expires_at_millis
    }
}

/// A proof of native execution trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeExecutionProof(AuthorizationProof);

impl NativeExecutionProof {
    #[must_use]
    pub fn authorization(&self) -> &AuthorizationProof {
        &self.0
    }
}

/// A proof of a ready, authorized user session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveSessionProof {
    proof: AuthorizationProof,
    session_id: SessionId,
}

impl InteractiveSessionProof {
    #[must_use]
    pub fn authorization(&self) -> &AuthorizationProof {
        &self.proof
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }
}

/// A proof recording an explicitly configured Docker socket trust boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerSocketProof(AuthorizationProof);

impl DockerSocketProof {
    #[must_use]
    pub fn authorization(&self) -> &AuthorizationProof {
        &self.0
    }
}

/// The only placement context accepted by [`GrantSnapshot::authorize_sandbox`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFixPlacement {
    host_id: HostId,
    workload_id: WorkloadId,
    placement_id: PlacementId,
    generation: Generation,
    host_epoch: super::ids::HostEpoch,
    target: CiTarget,
    runtime: RuntimeKind,
    execution_os: ExecutionOs,
}

impl OwnedFixPlacement {
    /// Construct a placement context from trusted placement ownership data.
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub(crate) const fn trusted(
        host_id: HostId,
        workload_id: WorkloadId,
        placement_id: PlacementId,
        generation: Generation,
        host_epoch: super::ids::HostEpoch,
        target: CiTarget,
        runtime: RuntimeKind,
        execution_os: ExecutionOs,
    ) -> Self {
        Self {
            host_id,
            workload_id,
            placement_id,
            generation,
            host_epoch,
            target,
            runtime,
            execution_os,
        }
    }

    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    #[must_use]
    pub const fn workload_id(&self) -> WorkloadId {
        self.workload_id
    }

    #[must_use]
    pub const fn placement_id(&self) -> PlacementId {
        self.placement_id
    }

    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    #[must_use]
    pub const fn host_epoch(&self) -> super::ids::HostEpoch {
        self.host_epoch
    }

    #[must_use]
    pub fn target(&self) -> &CiTarget {
        &self.target
    }

    #[must_use]
    pub const fn runtime(&self) -> RuntimeKind {
        self.runtime
    }

    #[must_use]
    pub const fn execution_os(&self) -> ExecutionOs {
        self.execution_os
    }
}

/// A sandbox proof bound to one owned fix placement and generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteSandboxProof {
    proof: AuthorizationProof,
    placement: OwnedFixPlacement,
}

impl ExecuteSandboxProof {
    #[must_use]
    pub fn authorization(&self) -> &AuthorizationProof {
        &self.proof
    }

    #[must_use]
    pub fn placement(&self) -> &OwnedFixPlacement {
        &self.placement
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fleet::ids::NilFleetUuid;
    use crate::fleet::{BitbucketConnectionId, NonNilTargetUuid, ResourceRequest};
    use uuid::Uuid;

    fn host_id() -> HostId {
        HostId::try_new(Uuid::new_v4()).unwrap_or_else(|_| unreachable!())
    }

    fn target() -> CiTarget {
        CiTarget::BitbucketWorkspace {
            connection_id: BitbucketConnectionId::new("bb-connection")
                .unwrap_or_else(|_| unreachable!()),
            workspace_uuid: NonNilTargetUuid::new(Uuid::from_u128(2))
                .unwrap_or_else(|_| unreachable!()),
        }
    }

    fn snapshot(capability: Capability, host: HostId, target: CiTarget) -> GrantSnapshot {
        GrantSnapshot::trusted(
            Principal::AuthenticatedUser {
                user_id: "user-1".to_owned(),
            },
            host,
            Some(target),
            [capability],
            Revision::new(1).unwrap_or_else(|_| unreachable!()),
            100,
            false,
        )
    }

    #[test]
    fn scoped_grant_rejects_wrong_host_and_target() {
        let host = host_id();
        let other_host = match HostId::try_new(Uuid::new_v4()) {
            Ok(value) => value,
            Err(_) => host,
        };
        let target = target();
        let grant = snapshot(Capability::Schedule, host, target.clone());
        assert_eq!(
            grant.authorize(other_host, Some(&target), Capability::Schedule, 1),
            Err(AuthorizationError::WrongHost)
        );
        assert_eq!(
            grant.authorize(host, None, Capability::Schedule, 1),
            Err(AuthorizationError::TargetMismatch)
        );
    }

    #[test]
    fn expired_revoked_and_missing_capability_are_denied() {
        let host = host_id();
        let target = target();
        let grant = snapshot(Capability::Schedule, host, target.clone());
        assert_eq!(
            grant.authorize(host, Some(&target), Capability::Schedule, 100),
            Err(AuthorizationError::Expired)
        );
        assert_eq!(
            grant.authorize(host, Some(&target), Capability::DrainHost, 1),
            Err(AuthorizationError::MissingCapability)
        );
        let revoked = GrantSnapshot::trusted(
            Principal::Reconciler,
            host,
            Some(target.clone()),
            [Capability::Schedule],
            Revision::new(2).unwrap_or_else(|_| unreachable!()),
            100,
            true,
        );
        assert_eq!(
            revoked.authorize(host, Some(&target), Capability::Schedule, 1),
            Err(AuthorizationError::Revoked)
        );
    }

    #[test]
    fn typed_proofs_require_their_declared_trust() {
        let host = host_id();
        let target = target();
        let grant = snapshot(Capability::NativeExecution, host, target.clone());
        let proof = grant
            .authorize_native(host, &target, 1)
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            proof.authorization().capability(),
            Capability::NativeExecution
        );
        assert_eq!(
            grant.authorize_interactive(
                host,
                &target,
                &SessionGrantSnapshot::trusted(
                    session_id(),
                    host,
                    target.clone(),
                    Revision::new(1).unwrap_or_else(|_| unreachable!()),
                    Revision::new(1).unwrap_or_else(|_| unreachable!()),
                    true,
                    true
                ),
                1,
            ),
            Err(AuthorizationError::SessionMismatch)
        );
    }

    #[test]
    fn interactive_proof_binds_ready_session_and_revision() {
        let host = host_id();
        let target = target();
        let session = SessionGrantSnapshot::trusted(
            session_id(),
            host,
            target.clone(),
            Revision::new(1).unwrap_or_else(|_| unreachable!()),
            Revision::new(3).unwrap_or_else(|_| unreachable!()),
            true,
            true,
        );
        let grant = GrantSnapshot::trusted_with_session(
            Principal::AuthenticatedUser {
                user_id: "user-1".to_owned(),
            },
            host,
            Some(target.clone()),
            [Capability::InteractiveSession],
            Revision::new(1).unwrap_or_else(|_| unreachable!()),
            100,
            false,
            Some(session.clone()),
        );
        let proof = grant
            .authorize_interactive(host, &target, &session, 1)
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(proof.session_id(), session.session_id());
        assert!(grant.recheck(proof.authorization(), 2).is_ok());
        let changed = GrantSnapshot::trusted_with_session(
            Principal::AuthenticatedUser {
                user_id: "user-1".to_owned(),
            },
            host,
            Some(target.clone()),
            [Capability::InteractiveSession],
            Revision::new(1).unwrap_or_else(|_| unreachable!()),
            100,
            false,
            Some(SessionGrantSnapshot::trusted(
                session.session_id(),
                host,
                target.clone(),
                Revision::new(1).unwrap_or_else(|_| unreachable!()),
                Revision::new(4).unwrap_or_else(|_| unreachable!()),
                true,
                true,
            )),
        );
        assert_eq!(
            changed.recheck(proof.authorization(), 2),
            Err(AuthorizationError::SessionMismatch)
        );
        let mut changed_session = session.clone();
        changed_session.revision = Revision::new(2).unwrap_or_else(|_| unreachable!());
        let mut changed_grant = grant.clone();
        changed_grant.session = Some(changed_session);
        assert_eq!(
            changed_grant.recheck(proof.authorization(), 2),
            Err(AuthorizationError::SessionMismatch)
        );

        let mut wrong_host = session.clone();
        wrong_host.host_id = id();
        let mut wrong_target = session.clone();
        wrong_target.target = CiTarget::BitbucketWorkspace {
            connection_id: BitbucketConnectionId::new("another-connection")
                .unwrap_or_else(|_| unreachable!()),
            workspace_uuid: NonNilTargetUuid::new(Uuid::from_u128(4))
                .unwrap_or_else(|_| unreachable!()),
        };
        for mismatched in [wrong_host, wrong_target] {
            let mut misbound = grant.clone();
            misbound.session = Some(mismatched.clone());
            assert_eq!(
                misbound.authorize_interactive(host, &target, &mismatched, 1),
                Err(AuthorizationError::SessionMismatch)
            );
            assert_eq!(
                misbound.recheck(proof.authorization(), 2),
                Err(AuthorizationError::SessionMismatch)
            );
        }
    }

    #[test]
    fn sandbox_is_limited_to_owned_linux_docker_placements() {
        let host = host_id();
        let target = target();
        let grant = snapshot(Capability::ExecuteSandbox, host, target.clone());
        let placement = OwnedFixPlacement::trusted(
            host,
            id(),
            id(),
            Generation::new(1).unwrap_or_else(|_| unreachable!()),
            crate::fleet::HostEpoch::new(1).unwrap_or_else(|_| unreachable!()),
            target.clone(),
            RuntimeKind::Docker,
            ExecutionOs::Linux,
        );
        assert!(grant.authorize_sandbox(&placement, 1).is_ok());
        let other_target = CiTarget::GithubRepository {
            installation_id: crate::fleet::ExternalId::new(1).unwrap_or_else(|_| unreachable!()),
            repository_id: crate::fleet::ExternalId::new(2).unwrap_or_else(|_| unreachable!()),
        };
        let other_placement = OwnedFixPlacement::trusted(
            host,
            id(),
            id(),
            Generation::new(1).unwrap_or_else(|_| unreachable!()),
            crate::fleet::HostEpoch::new(1).unwrap_or_else(|_| unreachable!()),
            other_target,
            RuntimeKind::Docker,
            ExecutionOs::Linux,
        );
        assert_eq!(
            grant.authorize_sandbox(&other_placement, 1),
            Err(AuthorizationError::TargetMismatch)
        );
        let native = OwnedFixPlacement::trusted(
            host,
            id(),
            id(),
            Generation::new(1).unwrap_or_else(|_| unreachable!()),
            crate::fleet::HostEpoch::new(1).unwrap_or_else(|_| unreachable!()),
            target,
            RuntimeKind::NativeProcess,
            ExecutionOs::Linux,
        );
        assert_eq!(
            grant.authorize_sandbox(&native, 1),
            Err(AuthorizationError::UnsupportedSandbox)
        );
    }

    #[test]
    fn admission_recheck_detects_revocation_and_revision_change() {
        let host = host_id();
        let target = target();
        let grant = snapshot(Capability::Schedule, host, target.clone());
        let proof = grant
            .authorize(host, Some(&target), Capability::Schedule, 1)
            .unwrap_or_else(|_| unreachable!());
        assert!(grant.recheck(&proof, 2).is_ok());
        let revoked = GrantSnapshot::trusted(
            Principal::AuthenticatedUser {
                user_id: "user-1".to_owned(),
            },
            host,
            Some(target),
            [Capability::Schedule],
            Revision::new(2).unwrap_or_else(|_| unreachable!()),
            100,
            true,
        );
        assert_eq!(revoked.recheck(&proof, 2), Err(AuthorizationError::Revoked));
    }

    fn session_id() -> SessionId {
        SessionId::try_new(Uuid::from_u128(3)).unwrap_or_else(|_| unreachable!())
    }

    fn id<T>() -> T
    where
        T: TryFrom<Uuid, Error = NilFleetUuid>,
    {
        T::try_from(Uuid::new_v4()).unwrap_or_else(|_| unreachable!())
    }

    #[allow(dead_code)]
    fn _resource_type_is_available() {
        let _ = ResourceRequest::new(1, 1, 0);
    }
}
