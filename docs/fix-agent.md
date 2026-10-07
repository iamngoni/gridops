# Fix agent

When a GitHub Actions job fails, the fix agent reads the failure, works on the
failing commit in an isolated container, and either opens a draft pull request
with a fix or explains why the failure isn't a code problem.

## Turning it on

1. **Grant the GitHub App two more permissions.** The agent needs
   **Contents: read and write** (to read the failing commit and push a fix
   branch) and **Pull requests: read and write**. Apps created from GridOps'
   manifest after this feature shipped already ask for them. For an existing
   App, open its settings on GitHub (Settings → Developer settings → GitHub
   Apps → your App → Permissions & events), add both permissions, save, and
   then approve the new permissions on each installation. Settings → AI agent
   lists installations that are still missing them.
2. **Connect an AI provider** in Settings → AI agent. You can either:
   - **Sign in with ChatGPT or Claude** to use a subscription. GridOps opens the
     provider's sign-in page; paste back the code (Claude) or the address of the
     page you land on (ChatGPT). Runs count against that plan's limits.
   - **Paste an OpenAI, Anthropic, or OpenRouter API key.** GridOps checks the
     key with the provider before saving it.

   Credentials are encrypted with `GRIDOPS_ENCRYPTION_KEY` and never sent to the
   browser.
3. **Pick a model.** The first connection is selected automatically with a
   sensible default model, so the agent is usable straight away.

Until a provider is connected and a model chosen, the **Fix with agent** button
does not appear.

## Starting a run

- **Button only** (the default): an administrator of the job's installation
  clicks **Fix with agent** on a failed job in Live logs or on a run page.
- **Automatically on failure**: GridOps starts a run for new failures, at most
  one per workflow run and up to the daily limit. Failures from before automatic
  mode was switched on, failures older than six hours, archived repositories,
  and the agent's own `gridops/fix-*` branches are skipped.

Progress streams into the job view, and a run can be cancelled at any point.

## What a run does

1. The reconciler claims the run and reads the job's log, the failed steps, the
   workflow file path, and which GridOps runner and pool the job ran on.
2. It reserves capacity from the runner manager like a runner would, so a fix
   never pushes the host past its limits. If the host is full, the run waits.
3. The manager starts a sandbox container from `GRIDOPS_AGENT_SANDBOX_IMAGE`
   (the runner image by default). GridOps streams the failing commit's tarball
   from GitHub into it and commits it as a local baseline.
4. The agent loop runs in the reconciler. The model reaches the sandbox only
   through tools: list, search, read and edit files, and run commands, which
   lets it reproduce the failure and check its fix.
5. The agent finishes in one of two ways:
   - **Pull request:** GridOps reads the diff from the sandbox, creates the
     commit through the GitHub API as the App, pushes `gridops/fix-<job>-<id>`,
     and opens a draft pull request against the failing branch. CI runs on it
     like any other pull request.
   - **Diagnosis:** for failures that are not code problems (a runner out of
     memory or disk, a label or architecture no runner has, a missing secret,
     an outage, a flaky test), the agent explains what to change, for example
     the pool's memory, and opens nothing.

A run is limited to 40 model turns and 25 minutes, and a fix to 60 files and
4 MiB.

## Isolation

The sandbox holds no secrets. The model credential stays in the reconciler, and
the GitHub token used for the run is limited to that one repository, expires
within an hour, and never enters the container. The sandbox:

- never gets the Docker socket, even when `GRIDOPS_RUNNER_DOCKER_SOCKET` shares
  it with runners;
- drops all capabilities except the few package managers need (`CHOWN`,
  `DAC_OVERRIDE`, `FOWNER`, `SETUID`, `SETGID`) and sets `no-new-privileges`;
- has CPU, memory, swap, and process limits like a runner;
- joins the runner network rather than the control-plane network, so it can
  install dependencies without sharing a network with the GridOps services;
- is deleted when the run ends, and orphans are swept every five minutes.

The agent treats logs, files, and command output as data rather than
instructions, but a repository's own build scripts still run in the sandbox
when the agent verifies a fix. Keep that in mind before enabling automatic mode
for repositories whose pull requests come from people you don't trust.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `GRIDOPS_AGENT_SANDBOX_IMAGE` | `GRIDOPS_RUNNER_IMAGE` | Sandbox image. Needs `bash` and coreutils; git is installed with apt or apk when missing. |
| `GRIDOPS_AGENT_SANDBOX_CPUS` | `2` | CPU limit per sandbox. |
| `GRIDOPS_AGENT_SANDBOX_MEMORY_MB` | `2048` | Memory limit per sandbox. |

These are read by the reconciler.
