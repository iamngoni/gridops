# macOS runners with Tart

GridOps can run ephemeral Apple Silicon macOS runners in [Tart](https://github.com/openai/tart) virtual machines. Tart uses Apple's Virtualization framework and APFS copy-on-write clones, so each job receives a clean VM without copying the full base disk.

This provider is optional. Docker-only installations do not need Tart or the native agent.

## Requirements

- An Apple Silicon Mac running macOS 13 or newer
- An interactive user session with an unlocked login keychain; Virtualization.framework requires this on recent macOS releases
- Rust 1.96+, `gh`, `jq`, and Homebrew
- Approximately 25 GB for the first Cirrus Labs macOS image, plus working space for clones and job output
- A review of Tart's current license and Apple's macOS virtualization terms before organizational use

Install Tart from its current Homebrew tap:

```sh
brew install openai/tools/tart
```

## Prepare a base image

The default non-vanilla Cirrus Labs images include the Tart guest agent, which allows GridOps to execute the runner without SSH or a host-mounted workspace. Prepare a stopped local base VM containing the current GitHub Actions runner:

```sh
./scripts/prepare-tart-runner-image.sh \
  ghcr.io/cirruslabs/macos-tahoe-base:latest \
  gridops-macos-tahoe-base
```

The script obtains the latest runner release and its SHA-256 digest from GitHub, verifies the archive inside the guest, installs it at `/Users/admin/actions-runner`, and stops the VM. Rerun the script to refresh an existing stopped base image.

Remote image pulls use eight concurrent transfers by default. Set `GRIDOPS_TART_PULL_CONCURRENCY` between 1 and 32 when the registry or network needs a different balance.

Tart's standard pull resumes interrupted layer transfers. On a connection where the registry repeatedly drops large layers, GridOps also includes a verified segmented path. It downloads GHCR blobs with resumable ranges, verifies the compressed and uncompressed layer digests, and assembles the same local VM before the preparation step:

```sh
brew install aria2
./scripts/pull-tart-image-resumable.sh \
  ghcr.io/cirruslabs/macos-tahoe-base:latest \
  gridops-macos-tahoe-base
./scripts/prepare-tart-runner-image.sh \
  ghcr.io/cirruslabs/macos-tahoe-base:latest \
  gridops-macos-tahoe-base
```

Interrupted segmented pulls resume from `~/Library/Caches/GridOps/tart-images`. The default uses four simultaneous layers with eight ranged connections each. Tune the bounded `GRIDOPS_TART_DOWNLOAD_PARALLELISM` and `GRIDOPS_TART_DOWNLOAD_CONNECTIONS` values when necessary.

Use an `-xcode` Tart image instead when workflows require a preinstalled Xcode toolchain. The pool's **Tart base VM** field must contain the local prepared VM name, not the remote OCI reference.

## Network isolation

GridOps starts VMs without graphics, audio, clipboard sharing, or shared host directories. It supports two Tart network modes:

- `softnet` is the production default. [Softnet](https://github.com/openai/softnet) blocks guest access to private addresses, including the host, while retaining internet egress. It is a separate privileged component; install and review it according to its project documentation before enabling the agent.
- `nat` uses Tart's default networking. It is useful for initial testing but the guest can reach services bound on the host, so it is not the recommended untrusted-CI boundary.

The installer fails closed when `softnet` is selected but its binary is unavailable. GridOps never silently falls back to NAT.

Tart currently requires the Softnet executable to be owned by root with its SUID bit set for unattended VM launches. After installing the Homebrew dependency, resolve the versioned binary and apply the documented privilege:

```sh
softnet_binary="$(realpath "$(command -v softnet)")"
sudo chown root:wheel "${softnet_binary}"
sudo chmod u+s "${softnet_binary}"
```

The installer verifies both conditions. Recheck them after a Softnet upgrade because Homebrew installs a new versioned executable.

## Install the native agent

Generate a token independently from the manager token. The native agent stores its copy in a mode-0600 file; the Docker manager receives the same value through the deployment environment.

For a manager running directly on the Mac, keep the default loopback bind:

```sh
export GRIDOPS_TART_AGENT_TOKEN="$(openssl rand -hex 32)"
./scripts/install-tart-agent.sh
```

For a containerized manager, bind the agent on a host interface reachable as `host.docker.internal`. Keep the random bearer token secret and restrict port 8790 with the host firewall when the machine is on an untrusted LAN:

```sh
export GRIDOPS_TART_AGENT_TOKEN="$(openssl rand -hex 32)"
export GRIDOPS_TART_AGENT_BIND=0.0.0.0:8790
./scripts/install-tart-agent.sh
```

Then configure the control plane with the same token:

```dotenv
GRIDOPS_TART_RUNNER_IMAGE=gridops-macos-tahoe-base
GRIDOPS_TART_AGENT_URL=http://host.docker.internal:8790
GRIDOPS_TART_AGENT_TOKEN=<same independently generated token>
```

Restart the manager after changing these values. Its health response reports Docker and Tart independently, and the pool form will offer **Tart · macOS ARM64**.

The installer builds `gridops-tart-agent`, installs it under `~/.gridops/tart-agent/bin`, and registers a per-user launchd agent. Inspect its status and logs with:

```sh
launchctl print "gui/${UID}/dev.gridops.tart-agent"
tail -f ~/.gridops/tart-agent/logs/agent.log
```

## Host guardrails

The Tart agent independently enforces a CPU budget, memory budget, runner-count ceiling, and minimum free-disk reserve before cloning a VM. Conservative defaults are:

```dotenv
GRIDOPS_TART_CPU_BUDGET=4
GRIDOPS_TART_MEMORY_BUDGET_MB=6144
GRIDOPS_TART_MAX_RUNNERS=1
GRIDOPS_TART_MIN_FREE_DISK_MB=40960
```

These are native-host limits, separate from the Docker container limits. The central manager also includes active Tart capacity when admitting any new runner, so Docker and macOS work cannot independently claim the same CPU envelope.

Each macOS runner receives only a short-lived GitHub JIT configuration over stdin. It is never placed in a command-line argument or persisted by the Tart agent. When the job ends, the VM is stopped; the reconciler deletes the clone and its retained management record. The base VM remains stopped and unchanged.

## Workflow labels

Tart pools automatically advertise `self-hosted`, `macOS`, `ARM64`, and the pool name. Select the exact pool explicitly:

```yaml
runs-on: [self-hosted, macOS, ARM64, macos-arm64]
```

GridOps only routes a queued job to a pool when every requested label matches that provider. Linux and macOS jobs therefore cannot consume each other's capacity.
