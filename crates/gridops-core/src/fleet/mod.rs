//! Durable fleet-domain identifiers, lifecycle values, and authorization.

mod admission;
pub mod authorization;
pub mod capabilities;
pub mod cursor;
pub mod domain;
pub mod ids;
pub mod protocol;
pub mod registry;
pub mod service;
mod store;

pub use service::{
    FleetError, FleetService, OperationStatus, Submission, SubmissionState, SubmitIntent,
    WorkloadKind,
};

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
