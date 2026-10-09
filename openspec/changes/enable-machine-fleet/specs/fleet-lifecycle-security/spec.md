# Spec Delta

## Purpose

This capability governs who may enroll, schedule, drain, maintain, revoke, or
retire fleet resources and makes shared-host trust and session boundaries
explicit.

## ADDED Requirements

### Requirement: Lifecycle axes are independent and validated
The system SHALL store enrollment, connectivity, scheduling intent, integrity, and per-backend health as separate axes with validated combinations rather than one overloaded status string.

#### Scenario: Approved host is offline and draining
- **WHEN** an approved host loses contact while an administrator has requested drain
- **THEN** the UI shows approved enrollment, offline connectivity, draining intent, and the backend's unknown reason separately

#### Scenario: Retired host is reactivated
- **WHEN** an operator requests activation of a retired host
- **THEN** the system requires a new approved recovery path and does not silently clear retired history or credentials

### Requirement: Grants constrain target and resource scope
The system MUST enforce host grants for installation, workspace, pool, CI target, and capability scope, rechecking authorization inside the admission transaction; Bitbucket workspace grants SHALL be independent of GitHub installation grants.

#### Scenario: Pool administrator selects another tenant host
- **WHEN** a pool administrator submits a selector outside their host or target grant
- **THEN** the request is denied with a stable authorization error and reveals no cross-tenant runner details

#### Scenario: Grant is revoked during admission
- **WHEN** an administrator revokes a pool's host grant while admission is racing
- **THEN** the transaction rechecks the grant and either aborts or queues without creating a usable placement

### Requirement: Pause, drain, maintenance, and retire have explicit effects
The system SHALL make pause and drain block new GridOps placements immediately, drain wait for actual managed-listener quiescence while allowing provider-assigned work to join existing drain work, maintenance gate upgrades/restarts by default, and retire preserve history and cleanup obligations.

#### Scenario: Drain has active work
- **WHEN** an administrator drains a host with a running runner
- **THEN** the host explains the blocking workload, stops new starts, and reaches drained only after no eligible listener or running workload remains

#### Scenario: Provider assigns during graceful drain
- **WHEN** GitHub or Bitbucket assigns work to an existing listener after GridOps has blocked new placements but before that listener is quiescent
- **THEN** the work joins the drain accounting, the UI does not claim Drained, and the listener must quiesce before completion

#### Scenario: Busy host needs disruptive action
- **WHEN** an administrator requests kill/reboot before graceful drain completes
- **THEN** the system requires an explicit disruptive audit action and records the affected unknown workloads

#### Scenario: Bulk lifecycle action partially fails
- **WHEN** an administrator drains or upgrades multiple hosts and one operation fails
- **THEN** the durable response reports per-host outcomes and partial failure without claiming the batch succeeded

### Requirement: Native and interactive trust is explicit
The system MUST require administrator-approved native trust, declared isolation limitations, least-privilege accounts, and a ready authorized user session for interactive work; session loss SHALL pause new interactive starts.

#### Scenario: Member requests native capability
- **WHEN** a member without the native trust grant requests a native profile
- **THEN** authorization fails before any bootstrap secret or process reaches the host

#### Scenario: macOS or Windows session logs out
- **WHEN** a required user session disappears
- **THEN** the backend becomes unavailable for new GUI work, running work follows its adapter drain contract, and no automatic login occurs

#### Scenario: Two profiles share one interactive session
- **WHEN** two CI profiles request the same authorized user session
- **THEN** the session admits at most one interactive job by default across CI targets, unless the operator has declared tested independent session capacity

### Requirement: Agent commands and secrets are least privilege
The system SHALL restrict the host agent to enrolled-host operations, use sealed placement-scoped bootstrap references, redact secrets from audit/log payloads, and deny arbitrary host shell, desktop, or cross-host operations; configured Docker socket sharing and sandbox execution require typed trust capabilities.

#### Scenario: Malformed command requests shell
- **WHEN** an authenticated agent receives a command outside its typed backend operation set
- **THEN** it rejects the command, records a bounded diagnostic, and performs no arbitrary execution

#### Scenario: Fix sandbox requests scoped execution
- **WHEN** an authenticated agent receives a typed `ExecuteSandbox` operation for an owned Linux Docker fix placement
- **THEN** it executes only inside that sandbox with no host or other-workload access, while an explicitly configured Docker socket share is recorded as a privileged runner capability

#### Scenario: User asks for another tenant's logs
- **WHEN** a caller lacks the target workload grant
- **THEN** the API returns a non-sensitive authorization error and does not disclose log paths, runner names, or host credentials

### Requirement: Autoapproval is bounded and capability-scoped
The system SHALL constrain any enrollment autoapproval policy to at most one hour and named host count, targets, pools, and capabilities; automatic approval SHALL never grant native or interactive trust outside that declaration.

#### Scenario: Autoapproval exceeds policy bounds
- **WHEN** an administrator configures an autoapproval window longer than one hour or omits a target/capability scope
- **THEN** the policy is rejected and no enrollment is autoapproved

#### Scenario: Bounded autoapproval is used
- **WHEN** a matching host enrolls during a valid scoped policy window
- **THEN** only the declared enrollment/trust scope is approved, and the use is audited
