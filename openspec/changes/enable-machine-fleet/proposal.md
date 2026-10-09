# Proposal

## Why

GridOps currently treats runner providers and a few execution endpoints as the
capacity boundary, so an operator cannot enroll mixed machines, see which physical
host owns a runner, or safely share capacity across hosts. Provisioning paths also
use transient provider-specific reservations and singleton agent assumptions, which
make recovery, adoption, and fix-agent placement unsafe. This change makes the
machine fleet a durable operational boundary while preserving existing pools,
targets, history, and upstream job assignment.

## What Changes

- Add first-class physical hosts, discovered execution backends, resource domains,
  health samples, lifecycle intent, host epochs, and host-scoped audit history.
- Add short-lived host enrollment, OS installers, outbound HTTPS agent protocol,
  credential rotation/revocation, version negotiation, durable journals, and
  reconnectable command/event/log exchange.
- Add explicit Linux, macOS, and Windows backend adapters for GitHub Actions and
  supported Bitbucket runner modes, including native/headless and authorized
  interactive-session boundaries. Unsupported combinations remain explicit.
- Replace provider-local transient admission with durable cross-host placement,
  one physical shared budget for runners, VMs, native processes, and fix sandboxes,
  fair queues, profile selectors, deterministic placement, and explanations.
- Add idempotent command recovery, epoch fencing, unknown-outcome handling,
  cleanup tombstones, backup/restore incarnation changes, and host-isolated
  reconciler convergence.
- Discover existing upstream/local runners as provenance-aware read-only inventory;
  add explicit dual-authorized replacement-registration adoption with rollback.
- Add scoped fleet grants, native/session trust controls, pause/drain/maintenance/
  retire lifecycle actions, least-privilege storage and command boundaries, and
  stable API errors with idempotency and revisions.
- Add Fleet summary/detail views, accessible drawers, host/backend/runner links,
  freshness-aware metrics, remote logs/events, placement explanations, and
  phone/keyboard/error/permission states.
- Integrate GitHub and Bitbucket target compatibility, existing pool migration,
  API/reconciler convergence, and host-owned fix-agent Docker sandbox routing.
- Ship signed versioned Linux/macOS/Windows agent artifacts, canary and bounded
  upgrades, rollback gates, migration rehearsal, and the complete platform proof
  matrix. **BREAKING**: legacy provider-only scheduling cannot run against mixed
  fleet state after the readiness gate.

## Capabilities

### New Capabilities

- `fleet-host-registry`: durable physical hosts, backends, resource domains, identity, inventory, health, and samples.
- `fleet-enrollment-connectivity`: enrollment, installers, credentials, outbound protocol, rotation, revocation, and compatibility.
- `fleet-execution-backends`: Linux/macOS/Windows execution adapters, CI target matrix, native trust, and interactive sessions.
- `fleet-placement-admission`: execution profiles, selectors, cross-host placement, shared budgets, fairness, and explanations.
- `fleet-command-recovery`: durable commands/journals, idempotency, fencing, unknown outcomes, cleanup, and backup incarnation.
- `fleet-external-runners`: external observations, provenance, read-only inventory, explicit adoption, and handover rollback.
- `fleet-lifecycle-security`: grants, authorization, pause/drain/maintenance/retire, least privilege, and isolation boundaries.
- `fleet-observability-ui`: versioned fleet APIs, SSE/cursors, browser hierarchy, metrics, logs, events, and freshness states.
- `fleet-integration-migration`: GitHub/Bitbucket convergence, pool/schema migration, legacy cutover, and fix-agent routing.
- `fleet-release-verification`: signed packaging, upgrades, compatibility gates, fault tests, rollout evidence, and acceptance proof.

### Modified Capabilities

None. This project currently has no existing `openspec/specs` capabilities;
current runner, pool, autoscaling, and fix-agent features are extended by these
new contracts rather than represented as modified main specs.

## Impact

The change spans `crates/gridops-core` domain/config/provisioning/auth models,
`gridops-api` routes and contracts, manager/reconciler ownership, the existing
Tart/native adapter, a new portable host-agent boundary, SQLite migrations, and
the React/TanStack Router Fleet surface. It adds versioned agent APIs and tables,
changes pool compatibility from provider unions to execution profiles, and adds
GitHub/Bitbucket adapter contracts. It requires OS packaging and physical-host,
upstream, network, interactive-session, and browser evidence in addition to Rust
and frontend checks. This pull request records the planned behavior and delivery
contracts; runtime behavior is shipped only after implementation and acceptance
evidence satisfy the tasks and release gates.
