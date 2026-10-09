//! Durable fleet-domain identifiers, lifecycle values, and authorization.

pub mod authorization;
pub mod capabilities;
pub mod cursor;
pub mod domain;
pub mod ids;
pub mod protocol;

pub use authorization::{
    AuthorizationError, AuthorizationProof, Capability, DockerSocketProof, ExecuteSandboxProof,
    GrantSnapshot, InteractiveSessionProof, NativeExecutionProof, OwnedFixPlacement, Principal,
    SessionGrantSnapshot,
};
pub use domain::{
    AllocationReleaseError, AllocationState, Architecture, BackendReadiness, BitbucketConnectionId,
    CiPlatform, CiTarget, DelayedStartExclusion, EnrollmentState, ExecutionOs, ExternalId,
    ExternalIdError, Freshness, HostIntent, HostLifecycle, IntegrityState, LifecycleAxis,
    LifecycleTransitionError, LifecycleValidationError, NonNilTargetUuid, PlacementOwnership,
    PlacementState, PressureState, ProvenLocalAbsence, ProviderCleanupState, ResourceAmount,
    ResourceError, ResourceRequest, RuntimeKind,
};
pub use ids::{
    AllocationId, BackendId, CiTargetId, CommandId, CounterError, CounterKind, FleetIdParseError,
    FleetTextError, Generation, HostEpoch, HostId, MAX_COUNTER, NilFleetUuid, NonNilUuid,
    OperationId, PlacementId, ProfileId, ResourceDomainId, Revision, SessionId, UpstreamText,
    WorkloadId,
};
