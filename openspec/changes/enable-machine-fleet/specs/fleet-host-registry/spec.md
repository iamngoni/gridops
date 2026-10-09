# Spec Delta

## Purpose

This capability makes physical machines and their execution backends durable,
observable resources that can safely host several independent workloads.

## ADDED Requirements

### Requirement: Host identity is a durable physical boundary
The system SHALL represent each enrolled physical machine with a stable host ID, observed OS/architecture/hardware metadata, agent identity, boot/session identity, authority epoch, enrollment state, lifecycle intent, and revisioned budget policy.

#### Scenario: One host exposes several backends
- **WHEN** an enrolled Mac exposes native process and Tart VM runtimes
- **THEN** the system shows one physical host with two backend records and one shared root resource domain

#### Scenario: Duplicate enrollment is detected
- **WHEN** a new enrollment reports hardware signals matching an approved host but lacks an approved replacement operation
- **THEN** the system keeps it pending and explains that hardware signals do not prove identity or ownership

### Requirement: Backend capabilities are explicit and versioned
The system SHALL record runtime kind, execution OS/architecture, CI targets and modes, toolchain/image version, enforcement capabilities, session requirements, resource domain, configuration revision, and readiness for every enabled backend.

#### Scenario: A Docker engine runs inside a Mac VM
- **WHEN** inventory reports a Linux Docker engine hosted by a Mac
- **THEN** the backend is labeled Linux execution attached to the Mac host and its child budget is linked to the Mac physical domain

#### Scenario: Unsupported capability is observed
- **WHEN** a backend cannot prove a requested target or architecture
- **THEN** readiness is unsupported with a reason and the scheduler does not silently substitute another backend

### Requirement: Inventory and health preserve freshness and provenance
The system SHALL retain host samples, backend probes, uptime, CPU/memory/disk observations, runner counts, source timestamps, and freshness states without converting missing measurements into healthy zeroes.

#### Scenario: A host misses heartbeats
- **WHEN** no heartbeat arrives for 45 seconds and then 90 seconds
- **THEN** connectivity is respectively stale and offline, new placement is disabled at stale, and measured values remain stale or unknown

#### Scenario: One backend probe fails
- **WHEN** the host remains connected but a Bitbucket probe fails
- **THEN** the host remains visible while that backend has a scoped reconciling reason and unrelated backends continue independently

### Requirement: Resource domains prevent double counting
The system SHALL charge each placement once at every constrained ancestor and SHALL distinguish inclusive physical samples from bounded child engine or VM budgets.

#### Scenario: Docker and Tart share one machine
- **WHEN** a Docker runner, Tart VM, native process, and fix sandbox request capacity on one host
- **THEN** all four consume the host root allocation and Docker/Tart also consume their declared child domains

#### Scenario: External usage reduces headroom
- **WHEN** an authorized inventory sample reports unmanaged host consumption
- **THEN** headroom reflects the observation or configured external reserve without fabricating a managed placement

### Requirement: Registry history survives lifecycle changes
The system MUST retain host, backend, sample, event, runner-link, and allocation history through pause, retirement, migration, and credential replacement.

#### Scenario: A host is retired while unreachable
- **WHEN** an administrator retires an offline host with unknown workloads
- **THEN** retirement remains pending with cleanup obligations and history until verified cleanup or an explicit force-forget acknowledgment

#### Scenario: An authority epoch is replaced
- **WHEN** recovery advances the host epoch
- **THEN** the old identity generation and allocations retain their original epoch while new inventory and admissions use the new epoch
