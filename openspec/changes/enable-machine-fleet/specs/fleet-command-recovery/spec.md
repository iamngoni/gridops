# Spec Delta

## Purpose

This capability makes host commands and workload ownership recoverable across
retries, crashes, partitions, replacements, and restore operations without
premature release or duplicate control.

## ADDED Requirements

### Requirement: Commands have immutable identity and bounded deadlines
The system SHALL persist a versioned command envelope containing command ID, idempotency key, host ID/epoch, placement/generation, configuration revision, issued time, deadline, and typed operation before delivery.

#### Scenario: Agent receives a duplicate command
- **WHEN** the same ID and identical payload is delivered to an agent again after reconnect
- **THEN** the agent returns its journaled outcome without repeating non-idempotent local work

#### Scenario: ID is reused with changed payload
- **WHEN** a command ID or idempotency key is presented with different payload bytes
- **THEN** the server rejects it as a conflict and preserves the original outcome

### Requirement: Agent journal precedes acknowledgment and side effects
The agent MUST durably journal receipt and local runtime state transitions before acknowledging or performing non-repeatable work, while the reconciler owns upstream registration and persists its result/secret reference before dispatching minimum bootstrap material.

#### Scenario: Controller crashes after registration
- **WHEN** the reconciler process exits after upstream registration may have succeeded but before its result reaches the server
- **THEN** the reconciler queries the target-scoped registration and records success, failure, or uncertain without blind duplicate registration

#### Scenario: Agent crashes after local runtime creation
- **WHEN** the host agent exits after creating a labeled local runtime but before acknowledging the command
- **THEN** its journal/inventory reattaches the proven object or records uncertainty without asking the agent to create an upstream registration

#### Scenario: Controller receives a duplicate result
- **WHEN** an identical command result is submitted more than once
- **THEN** the server idempotently applies it once and acknowledges the stored outcome

#### Scenario: Server crashes after intent commit
- **WHEN** the control plane stops after committing a command intent
- **THEN** restart replays the outbox and does not create a second placement or allocation

### Requirement: Epoch and generation fence stale control
The system SHALL reject stale host epochs, boot/session identities, and placement generations from changing current placement, allocation, command, or cleanup state, while preserving the original epoch on every historical placement and allocation.

#### Scenario: Replaced host reconnects
- **WHEN** an old agent returns after an administrator-approved replacement advanced the epoch
- **THEN** its commands and results are rejected, old allocations retain their original epoch, and the new identity must reconcile before starts

#### Scenario: Stale result arrives out of order
- **WHEN** a result for generation one arrives after generation two is current
- **THEN** it is stored as historical cleanup evidence and cannot stop, release, or rewrite generation two

### Requirement: Unknown outcomes retain ownership
The system MUST model uncertain placement/command states explicitly and SHALL NOT infer process termination, provider deregistration, or capacity release from a timeout, missed heartbeat, failed listing, or lost acknowledgment alone; definitive local absence can release physical capacity while upstream cleanup remains tombstoned.

#### Scenario: Host disappears during stop
- **WHEN** a stop command times out while the host is offline
- **THEN** the placement and allocation remain uncertain, the host is isolated from new starts, and cleanup resumes after evidence returns

#### Scenario: Offline runner still has valid authority
- **WHEN** a disconnected host retains a currently valid credential and its runner may still be listening upstream
- **THEN** the control plane cannot claim physical termination; it expires future start authority, records the limitation, and requires revocation or recovery when contact returns

#### Scenario: Bitbucket secret is lost
- **WHEN** a persistent registration may exist but its one-time OAuth secret is unavailable
- **THEN** the system records an orphan-recovery operation to identify/quiesce/delete it and create a fresh registration rather than claiming recovery

#### Scenario: Local absence is proven before provider cleanup
- **WHEN** the owned process/resource is definitively absent and delayed starts are excluded but provider deletion is unavailable
- **THEN** physical CPU/memory/slot allocation is released, while the upstream registration remains an explicit cleanup tombstone

### Requirement: Cleanup and restore are fenced workflows
The system SHALL retain cleanup tombstones until local and upstream removal are separately confirmed and SHALL restore with a fresh control-plane incarnation, paused scheduling, invalidated restored credentials/start grants, and approved host-by-host recovery; an offline agent may not learn the new incarnation until it reconnects.

#### Scenario: Restore is initiated
- **WHEN** an administrator runs the supported restore command from a backup
- **THEN** a new incarnation is recorded, the restored controller refuses new grants and invalidates its pending starts, reconnecting agents reject old authority after approved recovery, and unknown allocations remain accounted for

#### Scenario: Previously delivered grant is offline
- **WHEN** a disconnected agent still holds a not-yet-expired start grant during restore
- **THEN** the server cannot remotely stop that agent instantly, the old grant permits a start only before bounded suspend-aware local expiry, already-running work may finish, and reconnect requires approved handshake/inventory before any new Start

#### Scenario: Local cleanup succeeds first
- **WHEN** a local process is removed but provider deregistration is still unconfirmed
- **THEN** the tombstone remains visible while proven local capacity is released and provider registration state remains pending
