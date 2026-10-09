# Spec Delta

## Purpose

This capability gives operators a coherent Fleet surface and stable API views
from physical host through backend, runner, job, session, event, and log while
making stale, unknown, loading, and permission states visible.

## ADDED Requirements

### Requirement: Fleet APIs are versioned and operation-oriented
The system SHALL expose versioned host, backend, runner, sample, event, enrollment, agent, external-runner, adoption, and action resources with bounded cursors, idempotency keys, expected revisions, stable errors, and durable operation/status IDs for mutations.

#### Scenario: Host action is submitted
- **WHEN** an authorized caller drains a host with an idempotency key
- **THEN** the API returns a durable operation ID and status URL, and a retried identical request returns the same operation

#### Scenario: Concurrent edit has a stale revision
- **WHEN** a PATCH uses an old expected revision
- **THEN** the API returns a conflict with current revision metadata and does not overwrite the newer policy

### Requirement: Fleet hierarchy links physical ownership
The system SHALL provide Fleet summary and machine detail views that distinguish machine counts from runner counts and link each managed runner, job, fix placement, backend, external observation, session, event, and log to its proven host.

#### Scenario: Summary has nine machines and fourteen runners
- **WHEN** the fleet contains nine physical hosts with fourteen runners
- **THEN** the summary displays 9 machines and 14 runners as separate counts and does not count provider endpoints as machines

#### Scenario: Placement has unknown host
- **WHEN** a job's physical assignment is uncertain
- **THEN** the job view links to an unknown/stale ownership explanation rather than inventing a host association

### Requirement: Freshness and partial measurements are honest
The system MUST label online, stale, offline, unknown, partial, and permission-limited measurements, retain 24-hour/7-day/30-day views where available, and never render absent telemetry as zero or healthy.

#### Scenario: Metrics are partially reported
- **WHEN** a host reports CPU but not disk for the selected period
- **THEN** the chart marks disk coverage as partial/unknown and keeps CPU values separate

#### Scenario: Viewer lacks hardware grant
- **WHEN** a member opens a shared host without explicit host-reader access
- **THEN** workload details are limited to authorized targets and hardware/cross-target utilization is withheld

### Requirement: Browser live status replays safely
The system SHALL provide event IDs/cursors for browser SSE status and log streams, bounded reconnect backoff, cancellation on unmount, deduplication after reconnect, and a durable API record independent of the stream.

#### Scenario: Browser reconnects after event loss
- **WHEN** a Fleet detail view reconnects with its last event cursor
- **THEN** the server replays available events in order, omits duplicates, and reports a cursor gap if retention was exceeded

#### Scenario: Stream is cancelled
- **WHEN** the operator navigates away or closes a drawer
- **THEN** the browser cancels the stream and no orphan subscription continues consuming events

### Requirement: Fleet workflows use accessible drawers and full detail pages
The system SHALL provide `/fleet` and `/fleet/machines/$hostId`, use labeled right-side drawers for enrollment/edit/adoption/maintenance actions, and support keyboard, screen-reader, responsive, loading, empty, error, and permission states.

#### Scenario: Enrollment drawer is opened on a phone
- **WHEN** an operator opens Add machine at a phone width
- **THEN** the drawer is focus-managed, keyboard operable, readable without horizontal overflow, and exposes progress/error recovery

#### Scenario: Action loses permission
- **WHEN** an open maintenance drawer receives a permission failure
- **THEN** the action becomes disabled with concise recovery text and the page preserves current read-only state
