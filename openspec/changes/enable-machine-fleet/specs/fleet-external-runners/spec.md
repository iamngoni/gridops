# Spec Delta

## Purpose

This capability exposes existing GitHub and Bitbucket runners safely and enables
auditable adoption only after quiescence, exclusive ownership evidence, and a
replacement registration.

## ADDED Requirements

### Requirement: External inventory records provenance and confidence
The system SHALL merge authorized upstream listings with allowlisted local runtime/service inventory and classify observations as managed, external on a proven host, external with unknown host, conflicting, or unreachable.

#### Scenario: Name matches but host is unknown
- **WHEN** an upstream runner name matches a local observation without proof of identity or ownership
- **THEN** the UI shows unknown host confidence and the observation receives no managed commands or automatic pool capacity

#### Scenario: External resource consumes disk
- **WHEN** a proven host reports an externally managed runner with measured or configured reserve
- **THEN** headroom reflects that reserve while the external runner remains read-only inventory

### Requirement: Observation never grants mutation authority
The system MUST keep external observations separate from managed placements and SHALL never stop, remove, upgrade, clean, or adopt them automatically.

#### Scenario: Reconciler sees an external process
- **WHEN** reconciliation finds an external service on an enrolled host
- **THEN** it records an observation and leaves the process untouched unless an authorized adoption operation exists

#### Scenario: External listing is stale
- **WHEN** an upstream listing cannot be refreshed
- **THEN** the observation is marked stale/unreachable and is excluded from new capacity without being deleted

### Requirement: Adoption requires dual authorization and preflight
The system SHALL require both fleet/system and CI-target administrator permissions and scopes, whether held by one administrator or more, show proposed pool/profile/trust/resources, and preflight scope, local identity, credentials, runtime support, and current owner before handover.

#### Scenario: One administrator has both scopes
- **WHEN** one administrator holds and exercises both the fleet and CI-target permissions for an external runner
- **THEN** the dual-authorization requirement is satisfied and the operation can proceed after preflight

#### Scenario: Fleet-only approval is attempted
- **WHEN** only the fleet administrator approves an external runner
- **THEN** adoption remains pending for target scope approval and no upstream or local mutation occurs

#### Scenario: Conflicting controller is detected
- **WHEN** preflight finds ARC, another GridOps installation, or an unknown active controller
- **THEN** adoption is rejected with the owner evidence and the observed controller remains untouched

### Requirement: Adoption uses quiesce and replacement registration
The system MUST quiesce the old service/controller, let work finish, obtain exclusive shutdown evidence, retire the old registration, create a new managed registration, and link the identities without transferring or scraping plaintext credentials.

#### Scenario: Authorized handover succeeds
- **WHEN** both authorities approve, active work drains, and the old controller is confirmed stopped
- **THEN** the system retires the old registration, creates a replacement managed runner, and preserves visible history and placement provenance

#### Scenario: Provider registration cannot be retired
- **WHEN** the old registration remains active after the quiesce deadline
- **THEN** adoption stops before replacement, reports the conflict, and remains blocked for reconciliation without assuming the old service is still running

### Requirement: Handover failure is recoverable and never dual-owned
The system SHALL journal every handover phase, restore the old owner before handover only after proving its registration/credentials remain valid and no replacement exists, and expose a recoverable post-handover state without running two controllers for one upstream identity.

#### Scenario: Failure before ownership transfer
- **WHEN** local preflight or quiescence fails before old registration retirement
- **THEN** the operation rolls back to read-only observation only if the old registration and credentials are proven valid; otherwise it remains blocked for reconciliation

#### Scenario: Deletion outcome is uncertain
- **WHEN** quiescence stops the old owner but the old registration deletion has an ambiguous result
- **THEN** adoption remains blocked, no old service is blindly restarted, and reconciliation proves registration state before any recovery action

#### Scenario: Failure after retirement
- **WHEN** replacement registration fails after old retirement is confirmed
- **THEN** the system marks adoption incomplete, retains cleanup/history evidence, and offers a fresh authorized registration path without reviving both owners
