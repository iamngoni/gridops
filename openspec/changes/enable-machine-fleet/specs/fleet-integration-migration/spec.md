# Spec Delta

## Purpose

This capability integrates fleet placement with existing pools, GitHub and
Bitbucket targets, reconciler/API convergence, and host-scoped fix-agent
sandboxing while preserving legacy history and providing guarded migration.

## ADDED Requirements

### Requirement: Upstream integrations retain target ownership
The system SHALL let GitHub and Bitbucket assign jobs through their normal labels/groups/runner registration semantics and SHALL expose GridOps host placement without claiming to move an in-flight upstream job.

#### Scenario: GitHub selects a placed runner
- **WHEN** a compatible GitHub runner is created on a selected backend
- **THEN** labels/groups and target authorization determine upstream assignment, and the job links back to the actual host after observation

#### Scenario: Bitbucket queue evidence is unavailable
- **WHEN** Bitbucket demand cannot be observed through a supported queue diagnostic
- **THEN** desired/min/max and health remain visible with freshness limits, and the system does not claim complete autoscale parity

### Requirement: API and reconciler share one convergence authority
The system MUST route manual provision, autoscale, retry, and fix-sandbox requests through the durable fleet intent/admission service and SHALL isolate a failed host/backend from healthy hosts.

#### Scenario: API request races reconciler
- **WHEN** a manual provision and reconciliation see the same desired runner
- **THEN** one placement generation and allocation are converged without duplicate registration or provider-specific transient bypass

#### Scenario: One host agent is offline
- **WHEN** a host stops heartbeating during reconciliation
- **THEN** its placements become scoped unknown/stale while other hosts continue reconciling and receiving new demand

### Requirement: Migration preserves existing data and gates legacy binaries
The system SHALL back up and verify the current schema, preserve runner/pool/job/log/event IDs and associations, backfill Docker/Tart ownership only from verified endpoint inventory, and prevent the legacy provider-only reconciler from opening mixed fleet state through supported launcher/schema gates.

#### Scenario: Existing records contain provider history
- **WHEN** migration runs against runners, events, jobs, logs, multi-provider pools, Bitbucket data, and native records
- **THEN** counts, IDs, foreign keys, indexes, and history remain intact while normalized fleet links are added

#### Scenario: Physical endpoint mapping is unknown
- **WHEN** a Docker or Tart endpoint cannot be mapped to a verified host
- **THEN** the records remain visible as legacy/unassigned, new placements are held for that mapping, and active work is retained

### Requirement: SQLite compatibility rebuild is guarded
The migration runner MUST pause all mutating writers/executors, verify a consistent backup, set `PRAGMA foreign_keys=OFF` outside a transaction, use `BEGIN IMMEDIATE` or exclusive maintenance transaction, replace definitions in safe FK order, restore indexes/triggers, verify rows/relationships/foreign keys/integrity, record the SQLx migration/checksum marker within that same transaction, commit, and re-enable/recheck foreign keys without partial commit or replay.

#### Scenario: Rebuild detects a count mismatch
- **WHEN** copied rows or indexes do not match the verified source
- **THEN** migration aborts and restores/retains the backup without enabling fleet scheduling

#### Scenario: Rebuild succeeds
- **WHEN** all counts, foreign keys, integrity checks, and readiness gates pass
- **THEN** the new normalized schema becomes authoritative and legacy values remain compatibility history only

#### Scenario: Rebuild is interrupted
- **WHEN** the special runner stops before commit or is restarted after a committed migration marker
- **THEN** SQLite rolls back the maintenance transaction or skips the already-recorded step without replaying table replacement

### Requirement: Legacy execution paths are revoked at cutover
The system SHALL stop obsolete API/reconciler/manager/Tart instances, revoke old execution credentials, remove obsolete direct Docker/agent access paths, and make new host agents reject obsolete authority/auth/protocol.

#### Scenario: Obsolete deployment attempts a command
- **WHEN** a pre-fleet deployment tries to control a current fleet resource through the supported launcher or agent gateway
- **THEN** version/readiness/auth gates reject it before mutation and record a bounded incompatibility reason

#### Scenario: Manual old binary has local root access
- **WHEN** an administrator manually grants an old binary direct Docker root access outside supported tooling
- **THEN** the system makes no protocol guarantee, while the documented migration boundary remains intact

### Requirement: Fix-agent sandboxes are fleet placements
The system SHALL place fix-agent work only on eligible trusted Linux Docker backends, debit the same physical/child budgets, route tools/artifacts/logs through durable placement identity, and clean only its own host-scoped resources.

#### Scenario: Fix sandbox uses a remote enrolled host
- **WHEN** local capacity is unavailable and a trusted remote Docker backend is eligible
- **THEN** the sandbox runs on that host, all operations use its placement identity, and host allocation includes the sandbox

#### Scenario: Control plane interrupts a fix run
- **WHEN** the control plane restarts while a sandbox is active
- **THEN** reconciliation resumes host-scoped cleanup or records unknown state without sweeping another host's sandbox or exposing model/GitHub credentials
