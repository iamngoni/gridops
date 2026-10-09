# Fleet orchestration: product and interaction specification

Status: proposed, 2026-10-09. This describes the completed change; all listed core journeys are part of delivery. Workstreams in `tasks.md` are implementation ordering, not reduced product scope.

## Product outcome

An operator connects a collection of Macs, Linux machines and Windows PCs to one GridOps installation, sees the machine behind every managed runner and the provenance of external observations, and lets workload pools consume eligible capacity across the fleet. Adding a machine does not require running another control plane or editing deployment environment variables. Existing GitHub and Bitbucket connections, pools, logs, autoscaling and fix-agent workflows remain part of the product.

The reference screenshot contributes two concrete ideas: a machine can contain several runners, and physical-machine health is visible alongside runner status. Its screenshot does not establish that the reference application implements placement, enrollment, recovery or adoption; those are GridOps design decisions.

### Core user journeys

1. **Connect mixed machines.** From Fleet, choose Add machine, obtain a short-lived enrollment instruction appropriate to the OS, run the installer locally, inspect the discovered machine and backends, set its permitted workloads and resource reserve, and approve it. A NATed laptop makes outbound connections. A connected machine with no runners is still useful inventory.
2. **Operate one pool across machines.** Select eligible machines by explicit membership or labels, constrain backend, architecture, trust and toolchain capabilities, preview eligible capacity, and save. The same pool can consume several compatible hosts; one host can serve multiple authorized pools. New capacity is placed where policy permits; GitHub or Bitbucket assigns actual jobs.
3. **Find a job's physical owner.** Open a queued job's placement explanation or an active job's runner, then follow its machine link. See the actual backend, execution architecture, relevant software version and last observation time.
4. **Take a laptop away.** Drain the machine; see the running work that prevents completion; finish those jobs and confirm that managed listeners have stopped accepting work, then reach Drained. A listener whose state cannot be safely confirmed keeps the operation blocked with a concrete reason. New demand can use other eligible machines. Resume restores eligibility only after a fresh health check. Closing a laptop without draining shows stale/offline state and uncertain jobs, never fabricated completion.
5. **Bring an existing runner fleet into view.** Discover connected-platform registrations and correlate those reported by a host agent. Unmatched registrations appear under Unassigned external runners. Observe without altering their labels, registration, service or lifecycle. An explicit adoption workflow checks idle status and ownership, describes the necessary stop/re-register operation, then enrolls a replacement managed runner without transferring plaintext credentials.
6. **Expand onto Windows.** Install a Windows agent, register native service execution or an explicitly configured interactive user session, verify toolchain capabilities, and let compatible GitHub/Bitbucket jobs use it. The UI distinguishes session-dependent availability from service health. Windows is a core delivery workstream.
7. **Recover a lost machine or control plane.** A reconnect reconciles the durable command journal and real resources with placements before new work is admitted. Unknown work remains visible. An operator can inspect and resolve a quarantine with an audited explanation.
8. **Maintain the fleet.** Update agents with a versioned rollout and concurrency limits, inspect preflight failures, drain for maintenance, rotate or revoke host credentials, and retire hosts while retaining job and audit history.

## Information architecture

Add **Fleet** to the existing Operate navigation. Preserve Runner pools, Runners, Workflow runs, Repositories and Live logs. Fleet is another view of the same execution resources, not a parallel runner store.

| Surface | Contents and actions |
| --- | --- |
| Fleet overview `/fleet` | Connected/eligible, busy, draining, stale/offline and attention counts; eligible versus reserved capacity; machine list; filters; Add machine |
| Machine list | Name, OS/architecture, health and freshness, eligible backends, busy/total managed runners, observed runners, CPU, memory, disk, uptime, agent version, issue indicator |
| Machine detail `/fleet/machines/$hostId` | Overview; Runners; Backends and capabilities; Activity; Settings. Resource charts, current jobs, disk/storage volumes and maintenance status are linked to their owners |
| Machine hierarchy | Expand a machine into its runners, with busy/idle/unknown status and pool association; show empty connected hosts and unmatched external registrations explicitly |
| Pool placement settings | Allowed machines/labels, runtime/capability constraints, trust restrictions, capacity ceilings, preference order and eligible-host preview with rejection reasons |
| Queue diagnosis | Explain no matching OS/architecture, missing image/toolchain/session, insufficient capacity, stale metrics, drain, exhausted credentials/provider API, and permission restrictions separately |
| Global runners/jobs/logs | Machine and backend filters and links, source freshness, managed/observed ownership, durable log cursor and gaps |
| Fleet activity | Enrollment, approval, placement rejection, drain, disconnect, recovery, adoption, credential rotation and upgrade operations with actor and outcome |

The supported runtime matrix and provider-specific limits remain visible in setup and placement explanations. For Bitbucket, distinguish configured capacity and runner health from queue-driven autoscaling; advertise queue-based behavior only where the adapter has verified upstream demand evidence. A Docker engine inside a VM executes Linux jobs even when its physical host runs macOS or Windows.

Use the existing visual language, dense tables, status indicators and responsive layout. Enrollment, editing, adoption and maintenance details use accessible right-side drawers; full machine pages support deep links. State stays in the URL where sharing a filtered operational view is useful.

## Enrollment experience

The Add machine drawer explains the selected OS installation method, control-plane address and connection requirement. Proposed installer commands are generated by the future API, not static examples that imply a shipped installer today. No API token, user password or cloud secret appears in the reusable command history or fleet telemetry: enrollment credentials are short-lived and entered through a protected input/file flow. The installer verifies the release checksum/signature, installs the least-privileged service or user agent, probes capabilities and begins pending enrollment.

An administrator reviews the machine name, OS, architecture, agent version, execution identities, discovered backend inventory, resource envelope and requested permissions before approval. Auto-approval is available only through an explicitly scoped, time-limited administrator enrollment policy. A declined or expired attempt does not create eligible capacity. Revoking a pending token and revoking an enrolled machine are separate operations.

Enrollment must identify which physical machine owns a nested Docker VM/engine. Multiple installations claiming the same machine identity enter a conflict state until resolved; showing two rows must not silently double its capacity. Renaming a machine changes its display name, not placement identity.

## Capacity and freshness

Show **measured usage**, **committed capacity**, **pending reservations**, and **operator reserve** as separate quantities. Do not label reserved CPUs as CPU utilization. Show host totals once and backend sublimits underneath. Missing metrics display Unknown, and stale measurements show their age. Host connectivity, local resource health and CI registration status are independent indicators.

Charts retain fleet and per-machine history for the established 24-hour, 7-day and 30-day periods. The fleet totals exclude unsupported/unknown values from denominators and explain partial coverage. A machine that reconnects supplies historical events with original timestamps without presenting them as live samples.

## Operational states and actions

Enrollment (pending/approved/rejected), connectivity (online/stale/offline), scheduling (active/paused/draining/drained/maintenance), and integrity (healthy/quarantined/revoked/retired) are separate fields with documented valid combinations. The UI derives one primary status and exposes the reasons. A green agent heartbeat cannot hide a broken backend or revoked platform registration.

Drain, resume, maintenance, upgrade, rotate credentials, revoke and retire actions show affected hosts and current jobs. Asynchronous actions return an operation with progress, cancellation rules and a durable result. Bulk actions return per-machine outcomes and do not claim all succeeded when some failed. Force-stop is an explicit destructive operation; ordinary drain does not intentionally cancel active CI jobs. Drain immediately blocks new GridOps placements, while GitHub/Bitbucket may still assign work to an existing listener until that listener is quiesced. A job arriving during that interval joins the work being drained; the UI does not declare Drained until quiescence is confirmed.

A server-side revoke or new authority epoch prevents future authorized control operations; it cannot instantly stop a disconnected process or erase its credentials. Offline agents stop accepting GridOps starts when their local authority deadline expires. Existing workloads remain uncertain and accounted for until execution and upstream state are reconciled. Valid host-credential theft is a host compromise: conflicting identities are quarantined when detected, and recovery requires revocation/rotation and ownership checks rather than a claim of hardware attestation.

Normal restart and backup restore are distinct flows. A supported restore starts paused, invalidates restored host credentials and pending start grants, creates a fresh control-plane incarnation, and requires administrator-approved per-host credential recovery and inventory reconciliation before scheduling resumes.

Native execution is visibly marked as using the host user/service account. Its workspace cleanup does not imply VM isolation. Interactive execution defaults to one concurrent job per authorized user session across all pools and CI platforms. Additional session concurrency requires explicitly declared and tested independent capacity. Interactive execution requires a present, permitted user session; a background service cannot grant macOS accessibility/screen-recording permission or create a Windows desktop session automatically.

## Accessibility and responsive behavior

Fleet tables support keyboard navigation, sort announcements, focus-visible controls and text labels for status. Expand/collapse works without hover. Drawers trap focus and restore it to their trigger. At 390px width, prioritize machine name, status, runner occupancy and an attention indicator, with remaining metrics in the detail view; no page-wide horizontal overflow. Respect reduced motion. Large fleets use server-side pagination and bounded expansion requests.

## Completion means

The delivery is accepted only when the committed runtime/platform matrix in `design.md` is implemented and each mandatory combination has recorded proof, regardless of which capabilities an unfinished implementation advertises; enrollment, placement, shared budgets, recovery, observation/adoption, fleet operations and UI journeys pass their scenarios. Two healthy hosts are useful test infrastructure, not the complete feature. An unavailable Windows or remote-host test environment is an outstanding acceptance item, not grounds for claiming fleet support shipped.
