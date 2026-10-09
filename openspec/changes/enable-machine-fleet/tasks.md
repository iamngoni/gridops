# Tasks

Implementation sequencing is traced against the checked-in planning documents:
[`design.md`](design.md), [`product.md`](product.md), [`acceptance.md`](acceptance.md),
and [`source-map.md`](source-map.md). Each group names its capability spec and
release-gate evidence; every task remains unchecked until behavior and its proof
are complete.

## 1. Domain, schema, and contracts

Depends on: none. Trace: `fleet-host-registry`, `fleet-placement-admission`,
`fleet-command-recovery`; acceptance F01–F10, F29–F30.

- [ ] 1.1 Add typed `HostId`, `BackendId`, `WorkloadId`, `PlacementId`, `CommandId`, `HostEpoch`, `CiTarget`, lifecycle axes, and capability-bearing authorization values in `crates/gridops-core`; verify constructors reject malformed IDs, illegal state combinations, and cross-target grants with focused Rust tests.
- [ ] 1.2 Add additive SQLite tables for `fleet_hosts`, `host_backends`, `host_resource_domains`, profiles/selectors, placements, allocations, commands, observations, samples, events, credentials, and adoptions in `migrations/`; verify schema snapshots, indexes, FK targets, uniqueness constraints, and WAL/foreign-key integrity from a realistic legacy fixture.
- [ ] 1.3 Define versioned Rust/TypeScript request, response, error, cursor, operation-status, event, log-chunk, and placement-explanation contracts under `crates/gridops-core`, `crates/gridops-api`, and `src/`; cover the admin/agent routes, status codes, UUID/revision/cursor/idempotency rules, and bounded defaults in `design.md`; verify runtime parsing rejects unknown unsafe values, oversized payloads, and malformed cursors.
- [ ] 1.4 Implement the shared `gridops-core::fleet` application service boundary and durable intent transaction used by API, reconciler, autoscaler, and fix-agent callers; verify concurrent callers cannot create duplicate placement generations or allocations.
- [ ] 1.5 Document API/protocol limits, error codes, compatibility ranges, and domain relationships in the change's implementation-facing contract documentation; verify examples match generated/shared schemas and contain no secrets.

## 2. Host registry and inventory

Depends on: 1.1–1.4. Trace: `fleet-host-registry`; acceptance F01–F04, F08,
F20, F29.

- [ ] 2.1 Implement host enrollment state, backend discovery, resource-domain linkage, boot/session identity, epoch, revisions, and sample persistence in `crates/gridops-core` and the host-agent protocol; verify one physical host can expose Docker/Tart/native backends without duplicate root budgets.
- [ ] 2.2 Add allowlisted local runtime/service inventory and upstream observation ingestion with provenance, confidence, freshness, and scoped failure reasons; verify a name-only match never becomes host ownership or managed capacity.
- [ ] 2.3 Add host/backend/sample/event API queries and bounded pagination with authorization-aware projections; verify stale/offline/partial/unknown values remain explicit and unrelated backends continue when one probe fails.
- [ ] 2.4 Add registry and inventory unit/property tests for inclusive samples, child budgets, retirement history, duplicate enrollment, and epoch replacement; verify FK and event history survive retirement and credential generations.

## 3. Enrollment, connectivity, and agent foundation

Depends on: 1.1–1.5 and 2.1. Trace: `fleet-enrollment-connectivity`,
`fleet-command-recovery`; acceptance F03–F04, F10, F13–F15, F31.

- [ ] 3.1 Create the portable `gridops-host-agent` crate/binary with protocol supervisor, local durable journal, typed operation dispatch, and narrow privileged IPC boundary; verify arbitrary host shell/desktop/cross-host operations are rejected, typed `ExecuteSandbox` is confined to an owned Linux Docker fix placement, and explicit Docker socket sharing is a declared trust capability.
- [ ] 3.2 Implement protected enrollment issuance/consumption, pending approval, OS installers, opaque host credential storage, server-side verifier, rotation overlap, and revocation in `crates/gridops-api` and platform packaging; verify expired/replayed codes and wrong-host credentials fail without logging secret material.
- [ ] 3.3 Implement outbound HTTPS handshake, explicit proxy/CA configuration, 10-minute single-use enrollment, 5-minute credential overlap, 15-second heartbeat, 45/90-second stale/offline thresholds, 90-second local authority, 60-second start grant, 25-second maximum poll, suspend-aware expiry, acknowledged cursors, bounded chunks/spools, reserved control slots, and protocol negotiation; verify NAT/proxy success, TLS failure diagnostics, and incompatible-version maintenance.
- [ ] 3.4 Add event/log replay, quotas, backoff, cancellation, and secret redaction tests in the agent and API; verify mid-batch reconnect deduplicates event IDs and quota loss is explicit without blocking fencing.
- [ ] 3.5 Package systemd, launchd/background plus user helper, and Windows SCM plus optional interactive helper installers; verify signatures/checksums, service account ACLs, uninstall cleanup, and no automatic login or inbound firewall mutation on representative build hosts.

## 4. Platform execution adapters

Depends on: 1.1–1.5, 2.1–2.4, and 3.1–3.5. Trace: `fleet-execution-backends`;
acceptance F06, F16–F19, F22, F31, F34.

- [ ] 4.1 Extract existing Docker/Tart/native lifecycle behavior behind the backend adapter trait while preserving current GitHub behavior; verify deterministic labels/names map resources to placements and existing managed runners still reconcile.
- [ ] 4.2 Implement Linux Docker and native shell capability probes, cgroup/resource declarations, GitHub modes, and supported Bitbucket Docker/Shell registration modes; verify actual Linux x64 and ARM64 Docker/native jobs, required Docker prerequisites, persistent cleanup, and upstream-supported Shell architectures.
- [ ] 4.3 Implement macOS native x64/ARM64 and Apple Silicon Tart VM adapters with launchd/LaunchAgent distinction, TCC/session readiness, private workload trees, child budgets, GitHub ephemeral Tart mode, and persistent Bitbucket native/VM Shell modes; verify actual native x64, native ARM64, Tart GitHub, and Bitbucket jobs plus TCC/session loss behavior.
- [ ] 4.4 Implement Windows native x64/ARM64 headless and authorized interactive adapters with SCM, non-admin service account, ACL storage, Job Object/process-tree supervision, session helper, GitHub modes, and verified Bitbucket x64 PowerShell mode; verify real x64 and ARM64 headless/interactive GitHub execution where the current artifact supports it, Bitbucket x64 PowerShell, Session 0 rejection, logout pause, and pending evidence when hardware is unavailable.
- [ ] 4.5 Add compatibility-matrix tests and UI/API capability projections for every Linux/macOS/Windows and GitHub/Bitbucket row; verify unsupported combinations return stable reasons and Docker Desktop/WSL reports Linux execution attached to its physical host.
- [ ] 4.6 Record adapter-specific graceful listener shutdown, persistent lifecycle, registration secret, and resource-enforcement evidence; verify adapters retain blocked/unknown capacity when quiescence cannot be proven.

## 5. Durable placement, admission, and fairness

Depends on: 1.1–1.5, 2.1–2.4, 3.1–3.4, and 4.1. Trace: `fleet-placement-admission`,
`fleet-lifecycle-security`; acceptance F05–F09, F22, F32.

- [ ] 5.1 Implement pool execution profiles, host/tag selectors, target grants, capability filters, resource requests, global pool limits, and compatibility validation in `crates/gridops-core` and existing pool APIs; verify mixed profiles preserve existing pool associations and reject unauthorized selectors.
- [ ] 5.2 Implement atomic placement/allocation/command-intent admission with epoch, lifecycle, freshness, profile revision, authorization, ancestor budget, and pool rechecks; verify API, autoscaler, and fix-agent races cannot exceed host, child, or pool budgets.
- [ ] 5.3 Implement fixed-point CPU and integer memory/disk admission, inclusive physical plus bounded child charges, pressure hysteresis, external reserves, and unknown allocation retention; verify Docker, Tart, native, and fix workloads share one physical budget.
- [ ] 5.4 Implement durable equal-weight round-robin across eligible pools and then profiles, oldest eligible demand within each profile, one successful admission per pool per round, bounded aging, incompatible-head skipping, scan cap 100, cursor rotation across restarts, and deterministic utilization/readiness/preference/host/backend-ID placement; verify each eligible pool gets an opportunity when a compatible request fits and no wall-clock promise is made without capacity.
- [ ] 5.5 Implement durable placement explanations and rejection categories in API/UI contracts; verify stale health, missing trust, session loss, image mismatch, grant failure, and budget exhaustion each identify the affected host/backend.
- [ ] 5.6 Add concurrency, restart, and property tests for admission and document the fairness/budget invariants; verify tests exercise SQLite constraints rather than relying on an in-process mutex.

## 6. Command recovery and reconciler convergence

Depends on: 1.1–1.5, 2.1–2.4, 3.1–3.4, and 5.1–5.6. Trace: `fleet-command-recovery`,
`fleet-integration-migration`; acceptance F10–F15, F28–F30.

- [ ] 6.1 Implement host command outbox, agent journal transitions, idempotency conflict detection, at-least-once replay, deadlines, and placement-generation checks; keep upstream registration in the reconciler, and verify duplicate commands return the agent journal result while duplicate server results apply once.
- [ ] 6.2 Implement immutable original allocation epochs, boot/session fencing, stale-result storage, 90-second suspend-aware authority expiry, idle listener quiescence, and explicit uncertain/retryable/terminal transitions in reconciler and agent; verify stale commands cannot mutate new generations or release unknown allocations and sleep/resume requires handshake/inventory before Start.
- [ ] 6.3 Add target-scoped ambiguous registration recovery and Bitbucket secret-loss orphan cleanup/tombstones; verify timeout-after-create and lost acknowledgments do not duplicate registrations, local absence can release proven physical capacity, and provider cleanup remains independently pending.
- [ ] 6.4 Refactor `crates/gridops-api` and `crates/gridops-reconciler` to use one convergence service and isolate host/backend failure; verify a manager/agent outage leaves healthy hosts reconcilable and avoids global sandbox/runner sweeps.
- [ ] 6.5 Implement backup/restore incarnation, paused readiness, credential/start-grant invalidation, approved host recovery, suspend-aware expiry, and journal reconciliation; verify old agents reject restored commands, previously delivered offline grants are allowed only to expire locally, and no new start occurs before re-handshake and unknown placement review.
- [ ] 6.6 Add crash, partition, sleep/resume, deleted-journal, disk-full, lost-log, shutdown-race, and out-of-order-result fault tests; verify no duplicate ownership, premature release, secret leakage, or cross-host cleanup.

## 7. External runners and adoption

Depends on: 2.2–2.4, 3.2–3.4, and 6.1–6.5. Trace: `fleet-external-runners`,
`fleet-lifecycle-security`; acceptance F20–F21, F34.

- [ ] 7.1 Add external runner observation APIs, provenance/freshness views, local allowlist inventory, and read-only permissions; verify stale, unknown-host, conflicting-controller, and unreachable observations remain visible and non-mutating.
- [ ] 7.2 Implement dual authorization requiring both fleet and CI-target scopes (one administrator may hold both), preflight, proposed handover, quiesce/wait, exclusive old-controller shutdown proof, and replacement-registration-only adoption; verify fleet-only or target-only approval cannot mutate a runner.
- [ ] 7.3 Implement adoption journal, pre-handover rollback only after proving old registration/credentials valid and no replacement, post-retirement recovery, identity/history link, and no-dual-controller guard; verify ambiguous deletion blocks recovery and never blindly restarts an old service.
- [ ] 7.4 Add adoption API, events, operation status, and drawer recovery documentation; verify target scope, credential status, trust, resources, and planned handover are visible before approval.

## 8. Lifecycle security and fix-agent routing

Depends on: 1.1–1.5, 3.1–3.4, 5.1–5.6, and 6.1–6.6. Trace:
`fleet-lifecycle-security`, `fleet-integration-migration`; acceptance F07–F08,
F22–F24, F28.

- [ ] 8.1 Implement scoped system-admin, installation/pool-admin, member, host-reader, native-trust, session, and Bitbucket-workspace authorization proofs using existing auth/origin boundaries; verify revocation is rechecked inside admission and agent auth cannot call browser admin routes.
- [ ] 8.2 Implement pause/drain/maintenance/retire actions, busy-work explanations, disruptive-action audit, integrity quarantine/revoke, and host-scoped cleanup; verify drain blocks GridOps starts immediately, provider arrivals join outstanding drain work until listener quiescence, ordinary drain does not cancel jobs, and bulk actions report partial per-host results.
- [ ] 8.3 Route fix-agent Docker sandboxes through durable remote placements and shared budgets, preserving model/GitHub credential boundaries and host-scoped tool/artifact/log paths; verify typed sandbox exec is confined to the owned container, a remote enrolled host runs it, and interruption cleans only its own resources.
- [ ] 8.4 Add authorization, secret-redaction, native trust, session-loss, cross-tenant log, and fix-agent isolation tests; verify denied operations reveal no runner names, paths, tokens, or other tenant hardware.

## 9. Fleet UI and live observability

Depends on: 1.3–1.5, 2.3–2.4, 5.5, 6.1–6.5, and 8.1–8.2. Trace:
`fleet-observability-ui`; acceptance F08, F20, F22, F25–F27, F34.

- [ ] 9.1 Add `/fleet` and `/fleet/machines/$hostId` routes, host/backend/runner/job/fix links, distinct machine/runner counts, samples, budgets, uptime, agent version, sessions, events, observations, and remote logs; verify loading, empty, stale, offline, unknown, partial, and permission states.
- [ ] 9.2 Add accessible right-side drawers for enrollment, edit, adoption, maintenance, and upgrade operations with durable progress/status URLs; verify keyboard focus, screen-reader labels, cancellation, retry/error recovery, and phone-width layout.
- [ ] 9.3 Add pool editor profile/selector/capacity/explanation controls and runner/job/fix placement links; verify existing Docker/Tart settings migrate to explicit profiles without silently moving active workloads.
- [ ] 9.4 Implement SSE event/log streams with stable cursors, bounded reconnect backoff, deduplication, retention-gap messaging, and unmount cleanup; verify reconnect resumes ordered events and durable API status remains authoritative.
- [ ] 9.5 Add frontend typecheck, unit/accessibility tests, responsive browser checks, and API contract fixtures; verify desktop and phone widths, keyboard operation, and concise non-architectural operator copy.

## 10. Migration, release, and acceptance evidence

Depends on: 1–9 as applicable; platform evidence is mandatory for every matrix
row. Trace: `fleet-integration-migration`, `fleet-release-verification` and the
acceptance ledger; acceptance F16–F19, F28–F34.

- [ ] 10.1 Implement the exclusive guarded legacy provider-check rebuild and migration rehearsal: pause writers/executors, verify backup, set `foreign_keys=OFF` outside transaction, `BEGIN IMMEDIATE`, create/copy/drop/rename safely, restore indexes/triggers, verify rows/relationships/FKs/integrity, commit, re-enable/recheck, and record migration checksum; verify rollback leaves the pre-migration database usable and supported launchers reject unsafe old deployments.
- [ ] 10.2 Backfill only verified Docker/Tart endpoint mappings, enroll/upgrade current executors, reconcile existing upstream/local inventory, and enable one authority path; verify unverified mappings remain legacy/unassigned and active jobs/history/IDs are retained.
- [ ] 10.3 Implement signed artifact manifest, canary/bounded rollout, drain-by-default upgrade, atomic switch, prior binary retention, compatibility rollback, per-host results, and maintenance recovery; verify one failed canary does not corrupt healthy hosts or cancel unrelated work.
- [ ] 10.4 Run repository checks for changed Rust crates and frontend packages (`cargo fmt --all -- --check`, focused/all-target `cargo check`, clippy with warnings denied, focused/all tests, frontend typecheck/build/tests, and >=95% first-party line coverage per changed package); store logs and report local/CI evidence separately.
- [ ] 10.5 Run actual GitHub Linux Docker/native x64/ARM64, macOS native x64/ARM64 and Apple Silicon Tart ephemeral, Windows x64/ARM64 headless and authorized interactive jobs; verify labels, host association, drain/reconnect, process/resource behavior, session/TCC gates, and attach device evidence separately from CI results. Missing hardware leaves the mandatory row pending.
- [ ] 10.6 Run actual Bitbucket Linux Docker x64/ARM64, upstream-supported Linux Shell, macOS native/Tart Shell, and Windows x64 PowerShell jobs; record upstream artifact/architecture versions, persistent lifecycle, cleanup, queue/freshness limits, and the explicit Windows ARM64 upstream exclusion.
- [ ] 10.7 Run LAN/VPN/NAT/proxy/TLS, multi-host same-runtime, host-loss, restore, adoption, upgrade, fault-matrix, browser accessibility, SSE reconnect, and remote fix-agent scenarios; verify evidence paths are attached per gate and mandatory platform proof remains pending until observed.
- [ ] 10.8 Run F32 against 100 connected hosts and 1,000 runner records for 30 minutes with 15-second heartbeats, polls no longer than 25 seconds, ten 64 KiB/s log streams, 4 vCPU/8 GiB/local SSD; verify p95 fleet/detail reads <1s, p95 command availability <5s, RSS <6 GiB with no post-warmup growth, bounded DB/log growth, and no starvation. Record this as load evidence, not concurrent execution capacity.
