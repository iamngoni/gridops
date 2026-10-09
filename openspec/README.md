# Specifications in GridOps

GridOps uses [OpenSpec](https://github.com/Fission-AI/OpenSpec) to keep proposed behavior, architecture decisions, acceptance scenarios and implementation tasks with the code. The CLI is pinned to **1.14.1** in specification CI. It is development tooling, not an application dependency.

## Full machine fleet change

- [Inkdrop specification index](inkdrop://note/note:Gzb24BZR), in **GridOps → Fleet orchestration**.
- [Proposal](changes/enable-machine-fleet/proposal.md): full scope and capability boundaries.
- [Product and user experience](changes/enable-machine-fleet/product.md): enrollment, pool placement, machine hierarchy, adoption and operational journeys.
- [Architecture](changes/enable-machine-fleet/design.md): domain, protocol, admission, platform matrix, recovery and migration decisions.
- [Capability requirements](changes/enable-machine-fleet/specs): concrete requirements and acceptance scenarios across ten capabilities.
- [Implementation tasks](changes/enable-machine-fleet/tasks.md): dependency-ordered work and proof, initially unchecked.
- [Acceptance ledger](changes/enable-machine-fleet/acceptance.md): 34 release gates with explicit evidence layers.
- [Source evidence](changes/enable-machine-fleet/source-map.md): reviewed code baseline and upstream references.

The change includes Linux, macOS and Windows, supported GitHub/Bitbucket execution adapters, cross-host pools, existing-runner observation/adoption, recovery, upgrades and the Fleet UI. Delivery workstreams are sequencing, not a reduced feature boundary.

## Working with the specification

Install the same tool version when needed:

```sh
npm install -g @fission-ai/openspec@1.14.1
```

Inspect and validate from the repository root:

```sh
OPENSPEC_TELEMETRY=0 openspec status --change enable-machine-fleet
OPENSPEC_TELEMETRY=0 openspec validate --all --strict --no-interactive
```

The generated Codex skills under `.agents/skills/` support proposing, exploring, updating, applying, syncing and archiving changes. `openspec init --tools codex --no-animation` regenerates the integration when setting up another checkout. Preserve the repository's model/delegation and commit instructions while using those workflows.

Repository artifacts are the implementation baseline. Inkdrop contains the full review copy, including individual capability notes. Review edits made in either place, apply them to the corresponding repository artifact, validate, and refresh the associated Inkdrop note before accepting the affected workstream. The index links the tracking PR. Avoid maintaining an independently evolving second set of requirements.

OpenSpec artifact completeness and strict validation establish that the planning files are present and well formed. They do not establish implementation, provider execution, device behavior or deployment success. Mark tasks and release gates complete only with their recorded evidence; archive the change only after the complete agreed behavior has been delivered.

The OpenSpec GitHub Actions workflow validates spec/config changes. Existing application checks remain applicable when implementation begins.
