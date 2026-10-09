# Spec Delta

## Purpose

This capability places new runner and fix-sandbox capacity across eligible hosts
with durable shared accounting, fairness, and explanations that survive retries
and concurrent demand.

## ADDED Requirements

### Requirement: Execution profiles express hard eligibility
The system SHALL allow a pool to define target, runtime, execution OS/architecture, session and isolation requirements, image/release, labels, resource request, host/tag selectors, and preference without changing active placements.

#### Scenario: Mixed pool has two profiles
- **WHEN** a pool contains Linux Docker and macOS Tart profiles
- **THEN** each profile evaluates its own backend constraints while the pool's global limits remain shared

#### Scenario: Selector lacks a grant
- **WHEN** a profile selects a host or tag outside the caller's host grant
- **THEN** the profile save is rejected with an authorization reason and no hidden capacity is exposed

### Requirement: Admission is atomic and durable
The system MUST recheck host epoch/lifecycle/freshness, permissions, profile revision, pool limits, and aggregate allocations in one SQLite write transaction that creates placement, ancestor allocations, and command intent together.

#### Scenario: API and autoscaler race
- **WHEN** two demand sources request the last available host capacity concurrently
- **THEN** at most one transaction commits the allocation and the other receives a durable queued or capacity-exhausted result

#### Scenario: Provider API is slow
- **WHEN** admission commits while GitHub or Bitbucket is unavailable
- **THEN** the transaction still completes without provider I/O inside it and reconciliation later progresses the command

### Requirement: Physical and child budgets are shared
The system SHALL charge Docker runners, Tart VMs, native processes, and fix-agent sandboxes against one physical host budget and SHALL enforce bounded child engine/VM budgets without double counting inclusive samples.

#### Scenario: Fix sandbox competes with runner
- **WHEN** a fix sandbox request would exceed the same host's remaining memory or disk reserve
- **THEN** it is queued or rejected with the host allocation explanation even if a provider-specific budget appears free

#### Scenario: Unknown allocation remains
- **WHEN** a host loses contact while a placement's process state is unknown
- **THEN** its allocation remains counted against host and pool maxima and other hosts can continue admission

### Requirement: Placement is fair and deterministic
The system SHALL schedule queued demand through the shared admission path using equal-weight round-robin across eligible pools, round-robin profiles within each pool, oldest eligible demand within a profile, bounded aging, a durable cursor, and stable host/backend tie-breaks after hard constraints.

#### Scenario: One pool floods the queue
- **WHEN** one pool continuously adds demand while another has an eligible request
- **THEN** each eligible pool gets at most one successful admission per round, and the waiting pool gets its opportunity when a compatible request fits

#### Scenario: Two hosts tie
- **WHEN** eligible hosts have equal normalized utilization, readiness and configured preference
- **THEN** stable host ID followed by backend ID makes the placement reproducible without moving live workloads

#### Scenario: Head request does not fit
- **WHEN** the oldest request in a profile cannot fit any currently eligible host
- **THEN** it is skipped with an explanation, another compatible request can proceed, and the durable cursor continues from its prior position after restart

### Requirement: Rejection and placement explanations are complete
The system MUST return a durable placement explanation containing selected host/backend or queue state and each hard rejection category, including target, capability, grant, health, session, image, lifecycle, and budget reasons.

#### Scenario: All backends are stale
- **WHEN** a profile has compatible hosts but each backend is stale
- **THEN** the request explains stale health and remains queued or rejected without reporting zero capacity as healthy

#### Scenario: Manual action requests capacity
- **WHEN** an administrator retries a runner manually
- **THEN** the request enters the same fair admission path and returns an operation ID tied to its placement explanation
