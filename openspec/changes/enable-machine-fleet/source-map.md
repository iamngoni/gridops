# Source evidence and change boundaries

Reviewed 2026-10-09 against GridOps `master` at `a2b53ab59230d21838ccb3953635dd05ba53f819`. This records source inspection and upstream documentation, not deployed fleet verification.

## Existing behavior to preserve and extend

| Area | Current source | Consequence for this change |
| --- | --- | --- |
| Singleton topology | `crates/gridops-core/src/config.rs`; `crates/gridops-manager/src/main.rs` (`ManagerState`) | Replace one manager URL / Docker client / optional Tart agent as scheduling identities with durable host/backend ownership |
| Native macOS already exists | `migrations/0020_native_macos_runtime.sql`; `crates/gridops-tart-agent/src/main.rs` | Preserve existing native and Tart behavior; native macOS is not a newly invented missing feature |
| Provider-specific labels | `crates/gridops-core/src/autoscaling.rs` | Derive execution OS/architecture from selected backend capabilities, not the controller's compiled architecture |
| Separate provisioning I/O | `crates/gridops-core/src/provisioning.rs`; `crates/gridops-api/src/resources.rs`; `crates/gridops-reconciler/src/main.rs` | All demand sources use the same durable admission and convergence path |
| Provider-only schema | `migrations/0001_initial.sql`; `0013_runner_identity_scope.sql`; `0016_runner_provider.sql`; `0017_multi_provider_pools.sql` | Preserve target-scoped upstream IDs, child records, events and history while adding host ownership and normalized execution profiles |
| Current Bitbucket limitation | `crates/gridops-core/src/models.rs`; `crates/gridops-core/src/bitbucket.rs`; `crates/gridops-core/src/provisioning.rs` | Current Tart-only support is an adapter restriction. Full fleet delivery adds supported Linux, Windows and native macOS adapters explicitly |
| Fix-agent execution | `crates/gridops-reconciler/src/agent.rs`; `crates/gridops-manager/src/sandbox.rs` | Route Linux Docker sandboxes, tools, logs and cleanup through durable host placements and shared capacity |
| Authorization | `crates/gridops-api/src/auth.rs`; `crates/gridops-api/src/resources/route_security.rs` | Preserve browser origin checks and existing roles; add resource-scoped grants rather than trusting client filters |
| SQLite lifecycle | `crates/gridops-core/src/db.rs`; `scripts/restore-database-backup.sh` | Guard constraint rebuilds, exclusive migration, backup authority recovery and unsupported downgrades |
| Existing operations UI | `src/routes/settings.runner-host.tsx`; `src/lib/navigation.ts`; `src/features/runner-pools/schemas.ts`; `src/features/operations/operations.functions.ts` | Add Fleet and host ownership while retaining pools, jobs, logs and existing operator workflows |

Source files are relative to the repository root. Use the reviewed commit for exact historical comparisons; implementation may change line numbers and module boundaries.

## Upstream references checked

- [GitHub self-hosted runner reference](https://docs.github.com/en/actions/reference/runners/self-hosted-runners): system/architecture and assignment constraints.
- [GitHub Actions runner v2.338.0](https://github.com/actions/runner/releases/tag/v2.338.0): observed release assets for Linux x64/ARM64/ARM, macOS x64/ARM64 and Windows x64/ARM64. Available artifacts are not equivalent to GridOps adapter execution proof.
- [Bitbucket runner reference](https://support.atlassian.com/bitbucket-cloud/docs/runners/): Linux Docker/Shell, macOS and Windows modes, labels and prerequisites.
- [Bitbucket Windows setup](https://support.atlassian.com/bitbucket-cloud/docs/set-up-runners-for-windows/): native PowerShell runner setup and prerequisites.
- [OpenSpec](https://github.com/Fission-AI/OpenSpec) and [CLI reference](https://github.com/Fission-AI/OpenSpec/blob/main/docs/cli.md): local Markdown proposal/design/requirements/tasks, strict validation and Codex workflow integration. CLI package `@fission-ai/openspec@1.14.1` was installed and its version verified.

Recheck upstream supported versions, architecture maturity, registration lifecycle and drain behavior when implementing each adapter. In particular, GitHub's Windows ARM64 artifact does not establish Bitbucket Windows ARM64 support.

## Why OpenSpec is included

The fleet change crosses storage, multiple processes, three operating systems, two CI platforms and the browser. OpenSpec gives the implementation a versioned contract with concrete scenarios, dependency-aware planning artifacts and an unchecked delivery checklist. Strict CLI and CI validation catch malformed or incomplete requirement artifacts. They do not prove architectural correctness or implemented behavior, so this change also has independent architecture review and the separate acceptance ledger.

Inkdrop holds the complete human review copy under **GridOps → Fleet orchestration**. Repository artifacts are the implementation baseline; material edits are reflected in both places before the associated workstream is accepted. See the repository's OpenSpec README for the note index and update workflow.
