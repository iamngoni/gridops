# Spec Delta

## Purpose

This capability turns the fleet release into a verifiable cross-platform delivery
with signed artifacts, safe upgrades, fault recovery, and explicit evidence gates
for real hosts and upstream jobs.

## ADDED Requirements

### Requirement: Agent artifacts are versioned and verifiable
The release SHALL publish signed checksummed Linux, macOS, and Windows artifacts with supported OS/architecture metadata, installer/uninstaller behavior, least-privilege defaults, protocol range, and journal compatibility.

#### Scenario: Artifact signature is invalid
- **WHEN** an installer receives an artifact whose signature or checksum fails
- **THEN** installation is refused before service replacement and the host reports a bounded verification error

#### Scenario: Windows ARM64 evidence is pending
- **WHEN** a currently available GitHub Windows ARM64 artifact exists without proven target execution
- **THEN** the baseline row remains explicitly pending, its missing execution evidence blocks full acceptance, and the system does not relabel it unsupported merely because hardware proof is absent

### Requirement: Upgrades are drained, staged, and reversible
The system MUST support administrator-started canary and bounded-concurrency upgrades that preflight prerequisites, drain by default, atomically stage/switch binaries, preserve prior binary/journal, verify fresh inventory, and roll back only when compatibility permits.

#### Scenario: Upgrade preflight finds active work
- **WHEN** a host has running work and no disruptive approval
- **THEN** rollout waits for drain and does not cancel jobs or reboot the operating system

#### Scenario: New agent fails readiness
- **WHEN** the staged agent starts but cannot complete protocol, inventory, or session readiness
- **THEN** the host remains in maintenance and rolls back to the prior binary when journal/schema compatibility allows

### Requirement: Readiness gates protect migration and restore
The system SHALL prevent old binaries and legacy reconcilers from operating on mixed fleet state and SHALL keep a restored control plane paused until new incarnation, credential recovery, agent handshake, and unknown-placement reconciliation succeed.

#### Scenario: Obsolete deployment reaches the cutover boundary
- **WHEN** a pre-fleet deployment attempts a supported launch, agent command, or execution operation after cutover
- **THEN** the supported launcher refuses the incompatible schema/version, revoked execution credentials and obsolete direct access paths deny mutation, and new agents reject obsolete auth/protocol

#### Scenario: Restored host has no approved recovery
- **WHEN** a restored database contains a host but its administrator has not approved credential recovery
- **THEN** the restored controller issues no new grants for that host and its restored credentials/pending starts remain invalidated; previously delivered offline grants expire locally and already-running work may finish

### Requirement: Fault proofs cover durable boundaries
The release SHALL test crashes, timeout-after-create, duplicate/out-of-order results, stale epochs, registration-secret loss, disk pressure, lost logs, sleep/resume, deleted journals, replacement return, and shutdown races with assertions against duplicate ownership, premature release, and global sweeps.

#### Scenario: Result arrives after crash
- **WHEN** an agent reconnects with a durable result after a control-plane crash
- **THEN** the result is idempotently applied to its command/generation and unrelated placements are unchanged

#### Scenario: Journal is deleted
- **WHEN** a host agent starts without its local journal
- **THEN** it remains maintenance/unknown, reconciles only proven resources, and cannot blindly re-register or release allocations

### Requirement: Platform and upstream proof is real and separate
Completion SHALL report local/CI/unit/coverage checks separately from actual Linux, macOS, and Windows host runs, interactive-session/TCC evidence, GitHub jobs, Bitbucket jobs, network modes, and registration cleanup; build or documentation alone SHALL not close a platform gate.

#### Scenario: Windows service builds but no job runs
- **WHEN** the Windows installer and service compile successfully but no real headless, interactive, or required ARM64 GitHub job executes
- **THEN** Windows execution evidence remains pending and the release is not marked complete

#### Scenario: Bitbucket adapter is advertised
- **WHEN** a Bitbucket Linux Docker, Linux Shell, macOS native/Tart, or Windows Shell adapter is marked supported
- **THEN** an actual job, lifecycle cleanup, and documented architecture/mode result are attached; otherwise the adapter remains unavailable with reason

### Requirement: Rollout reports partial failure and evidence paths
The system MUST expose durable per-host rollout results, partial failures, artifact/protocol versions, proof references, and pending evidence gates without claiming final acceptance from coverage alone.

#### Scenario: Canary fails on one host
- **WHEN** one canary host fails while others pass
- **THEN** its result and recovery path remain visible, rollout concurrency stops or follows policy, and healthy hosts retain their verified state

#### Scenario: Coverage passes but device proof is missing
- **WHEN** changed crates meet the first-party 95% line target but Windows/remote execution proof is absent
- **THEN** coverage is reported as local verification and completion remains blocked by the explicit platform gate
