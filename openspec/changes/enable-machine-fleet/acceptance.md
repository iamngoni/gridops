# Fleet delivery acceptance and evidence ledger

Date: 2026-10-09. This PR starts a specification-led change. No fleet runtime behavior has been implemented or exercised by this specification work.

`proposal.md` defines scope, `design.md` records the selected architecture, `specs/` contains normative requirements, `product.md` defines operator journeys, and `tasks.md` sequences implementation. This ledger makes the release decision testable. Ordered workstreams do not remove any supported platform from completion.

## Evidence rules

Every gate starts **Pending**. Record the tested commit, environment/OS/architecture, backend and CI runner versions, setup, command or workflow URL, observed outcome, and retained redacted log/artifact location. Assign an implementation owner and an independent acceptance reviewer before executing a workstream. Unit, local integration, CI, real platform/provider, browser, and deployed fleet evidence are separate columns in the final report. A passing build, mock adapter or healthy service does not prove that a real CI job ran.

Re-run affected gates after relevant changes. Keep implementation tasks unchecked until their behavior and required proof are both present. Distinguish a specification review PASS from an implementation acceptance PASS. Unsupported upstream combinations must be explicitly unavailable; lack of test hardware for a committed combination leaves its gate pending even if the UI hides that capability. Runtime advertisement is not the delivery scope definition.

## Release gates

| ID | Required observable result | Capability / evidence layer | Status |
| --- | --- | --- | --- |
| F01 | Enroll several Linux, macOS and Windows machines into one instance. Renaming and reconnecting preserve identity; duplicate enrollment cannot double capacity. | host-registry; real fleet + API | Pending |
| F02 | One physical Mac reports Docker's Linux engine and Tart/native backends; reservations and measured totals do not duplicate physical resources. | host-registry / placement-admission; integration + real machine | Pending |
| F03 | Expired/reused/revoked enrollment and wrong-host credentials fail; pending hosts cannot run jobs. Approval/autoapproval is bounded by its exact trust and target grant. | enrollment-connectivity / lifecycle-security; API | Pending |
| F04 | NAT/VPN/proxy hosts connect using outbound HTTPS only; invalid CA/hostname and incompatible protocol fail explicitly. Rotation/revocation reject new unauthorized control operations; offline workloads remain accounted for, with local authority expiry and drain outcomes shown without promising instant remote termination. | enrollment-connectivity; network + real fleet | Pending |
| F05 | Two hosts offering the same runtime serve one pool; one host serves multiple pools without exceeding the pool-wide or machine-wide maximum. | placement-admission; real fleet + concurrency | Pending |
| F06 | Runtime OS/architecture comes from the execution backend, independent of the control-plane binary. Image/toolchain/session and target permissions are enforced before reservation. | execution-backends / placement-admission; domain + provider | Pending |
| F07 | Concurrent manual provision, autoscaler and fix-sandbox requests respect root/child/host/pool budgets. Admission rejection releases only proven unused reservations. | placement-admission; concurrency/property tests | Pending |
| F08 | Busy/committed, reserved, measured usage and operator reserve remain separate. Disk pressure on one filesystem blocks affected starts with an explanation and hysteresis. | placement-admission / observability-ui; integration + UI | Pending |
| F09 | Queued demand receives fair opportunity among eligible pools; preferences do not override hard constraints, grants or global maxima. Starvation and no-compatible-host cases are observable. | placement-admission; deterministic scheduler tests | Pending |
| F10 | A repeated identical command produces one owned runtime resource; changed payload under the same ID and stale generations/epochs are rejected. | command-recovery; crash/fault injection | Pending |
| F11 | Server and host crashes before/after registration, start, journal write and acknowledgment converge without duplicate ownership or premature capacity release. | command-recovery; boundary fault matrix | Pending |
| F12 | Lost heartbeats, failed inventory and a timed-out create leave uncertain workloads charged. Another healthy host continues eligible work within remaining pool capacity. | command-recovery; partition + real fleet | Pending |
| F13 | A laptop sleeps mid-job and reconnects; process ownership is checked using boot/start identity, delayed events keep original timestamps, and expired commands do not start. Suspend-aware elapsed time or mandatory resume re-handshake prevents sleeping clocks or wall-clock jumps from extending authority. | command-recovery; real sleep/resume | Pending |
| F14 | A stale copied/deleted journal, replacement agent or restored backup cannot replay an expired epoch/session. Conflicting host sessions are quarantined when detected; a stolen currently valid credential is explicitly treated as compromise requiring revocation. Restore starts paused and requires approved credential recovery/inventory reconciliation. | command-recovery / integration-migration; destructive fixture drills | Pending |
| F15 | Provider registration timeout and lost Bitbucket one-time secret recover through scoped lookup/cleanup. Local deletion and provider deletion have independent durable results. Proven local absence plus exclusion of delayed starts permits physical capacity release while upstream cleanup remains pending; unknown local state stays charged. | command-recovery / integration-migration; provider + faults | Pending |
| F16 | Linux Docker and native jobs run with correct labels, process/container lifecycle, workspace cleanup and declared resource controls. | execution-backends; actual CI jobs | Pending |
| F17 | macOS Tart and native jobs run on supported architectures. TCC/session-dependent jobs require actual grants; logout blocks new interactive work and does not fabricate a session. | execution-backends; real Mac + CI | Pending |
| F18 | Windows x64 and GitHub Windows ARM64 jobs run under the correct service account; interactive jobs run in an authorized user session. Stop targets the owned process tree without killing unrelated processes; restart recovery uses boot/start identity, and scoped ACLs remain enforced. | execution-backends; real Windows + CI | Pending |
| F19 | Every committed upstream-supported Bitbucket Docker/Shell/macOS/Tart/Windows matrix row executes a real job, reports compatible labels and cleans up its persistent registration. Unsupported combinations fail before provisioning. | execution-backends / integration-migration; Bitbucket jobs | Pending |
| F20 | External runner discovery shows target identity, source and freshness; unproven host matches stay unassigned. Discovery never mutates external services or credentials. | external-runners; provider + host inventory | Pending |
| F21 | Adoption requires both host and CI-target permissions (which one person may hold), drains and quiesces the old owner, creates a replacement registration and preserves provenance. Ownership conflict and interruption cannot leave two active controllers. | external-runners; real handover + fault injection | Pending |
| F22 | Host drain waits for active jobs and quiesces listeners; pause/drain/session loss/retirement have distinct outcomes. GridOps ordinary drain does not cancel busy jobs. A provider assignment during listener shutdown is included in the drain; unproven quiescence stays blocked. Explicit force-stop is separate from involuntary host/process failure. | lifecycle-security; provider + host lifecycle | Pending |
| F23 | Shared-host readers/admins see only permitted data and actions. A pool/installation admin cannot grant themselves host trust, drain other tenants' workloads or read their logs. | lifecycle-security; API authorization matrix | Pending |
| F24 | Browser CSRF/origin controls and host-auth scopes remain separate. Tokens, bootstrap secrets, model credentials and private log content do not leak through fleet events/metrics. | lifecycle-security; API + redaction tests | Pending |
| F25 | Fleet table, hierarchy, host details, placement preview and job/runner/fix-agent links show the same durable ownership. Stale/partial values show age and unknown state. | observability-ui; browser + integration | Pending |
| F26 | Enrollment, filtering, diagnostics, adoption and drain work by keyboard and at 390px width without page overflow; focus and live status are accessible. | observability-ui; browser/accessibility | Pending |
| F27 | Remote logs/events replay with cursor-based deduplication, bounded spool and explicit gaps under outage/backpressure; authorized cross-host filters do not mix streams. | observability-ui / command-recovery; stream fault tests | Pending |
| F28 | A fix-agent sandbox actually runs on a remote Docker host, shares its resource budget, routes tools/artifacts/logs and cleans up after interruption. | integration-migration; real remote sandbox | Pending |
| F29 | Existing Docker/Tart/native, GitHub/Bitbucket mixed pools, cross-installation IDs, job/log/event history and CI connection credentials survive migration; obsolete host-execution credentials/access are retired during controlled cutover. Shared physical mapping is verified, never guessed from URL. | integration-migration; populated DB + host drill | Pending |
| F30 | SQLite provider-constraint migration preserves foreign keys and all dependent records. Failure restores a consistent old or new state; deployment/restore tooling blocks unsafe binary downgrade, obsolete deployments lose direct execution access, and new agents reject old authority/protocol without relying on old binaries understanding fleet metadata. | integration-migration; exclusive migration/rollback fixtures | Pending |
| F31 | Signed/checksummed installers and service lifecycle work on systemd, launchd and Windows SCM. Bounded agent rollout drains, observes health, halts on failure and rolls back only when the previous binary can read the retained journal/schema; otherwise it stays in maintenance with recovery instructions. | release-verification; packaging + real hosts | Pending |
| F32 | At the explicit fleet-size/load target below, command polls, telemetry, logs and UI remain bounded. A disconnected/noisy host cannot starve unrelated scheduling or control-plane writes. | release-verification; load/chaos | Pending |
| F33 | Relevant repository lint/typecheck/build/test gates pass. Each changed first-party crate/package meets the ngoni-rust 95% line coverage gate with reviewed exclusions and meaningful failure tests. | release-verification; local + CI | Pending |
| F34 | A deployed mixed fleet completes enrollment → pool placement → actual jobs → drain → disconnect/recovery → upgrade. Release report accounts for every committed matrix row and unresolved gate; hiding an unimplemented capability cannot remove its gate. | release-verification; deployed end-to-end | Pending |

## Minimum environment matrix

- Two independent hosts of the same runtime to prove a pool spans machines, plus a shared physical machine with multiple backends to prove nested accounting.
- Linux x64 and ARM64 hosts for supported container/native adapters; current upstream versions must be recorded rather than assumed.
- Apple Silicon for Tart and native execution, plus native macOS x64 for the committed native x64 row. Real logged-in session and TCC permissions for GUI-dependent proof.
- Windows x64 and GitHub Windows ARM64, with service-account and interactive-session tests kept separate. A cross-compile alone never completes F18.
- Actual authorized GitHub repository/organization and Bitbucket workspace/repository fixtures, including a shared pool across granted targets. Bitbucket queue/autoscale claims require verified upstream demand evidence; configured-capacity/health proof must not be mislabeled as queue-driven autoscaling.
- Outbound-only network fixture, explicit proxy/CA fixture, sleep/partition/crash and full-disk fixtures, and an old-schema populated backup.

## Baseline load contract

F32 uses a reproducible protocol-level fleet fixture of 100 connected hosts and 1,000 managed runner records, with each host sending a heartbeat every 15 seconds, command long polls held for at most 25 seconds, and ten simultaneous log streams at 64 KiB/s each. Run the combined load for 30 minutes against a documented 4-vCPU/8-GiB control-plane host with local SSD storage. This exercises the actual API, SQLite, journal and log-storage paths; synthetic hosts do not replace real OS/provider gates.

Under this load, paginated fleet/detail reads must have p95 response time below one second, eligible durable commands must become available to a connected polling host within five seconds at p95, and control-plane resident memory must remain below 6 GiB without a growing post-warmup trend. Upstream provider API latency is measured separately. Bound per-host command/event/log queues; overload may produce explicit throttling or declared log gaps, but cannot silently drop durable state transitions or starve healthy hosts. Record database/log growth and retention cleanup. The target is a release acceptance workload, not a statement that all 1,000 jobs can execute concurrently on those hosts.

## Specification verification completed for this PR

The final PR validation record will include OpenSpec strict validation, artifact dependency completeness, proposal-to-capability coverage, requirement/scenario/task counts, internal links, unchecked implementation tasks, a clean scoped diff, and an independent architecture review. These checks validate the specification, not the pending release gates above.

## Implementation evidence entry template

```text
Gate:
Commit:
Owner / independent reviewer:
Evidence layer:
Host OS/architecture and agent/backend/runner versions:
CI target and workflow/job URL (if applicable):
Setup and action:
Expected / observed result:
Artifacts and redacted logs:
Remaining limitations:
Verdict: pending | pass | fail
```
