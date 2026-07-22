# GridOps

[![GridOps — Self-hosted GitHub Actions runner control plane](public/social-preview.jpg)](https://github.com/iamngoni/gridops)

[![CI](https://github.com/iamngoni/gridops/actions/workflows/ci.yml/badge.svg)](https://github.com/iamngoni/gridops/actions/workflows/ci.yml)

GridOps is a self-hosted control plane for GitHub Actions runners. Connect a GitHub App, select repositories or organizations, and operate isolated Linux container or macOS virtual-machine runner pools from one interface.

## Capabilities

- GitHub App OAuth, installations, encrypted user tokens, and short-lived installation tokens
- Multi-repository and organization-scoped pools that can share capacity across multiple runner providers
- Ephemeral and persistent Docker runners with labels, CPU, memory, PID, and capability limits
- Ephemeral Apple Silicon macOS runners in copy-on-write Tart virtual machines
- Editable pool configuration with generation-tracked, busy-safe rolling runner replacement
- Provision, scale, reconcile, pause, resume, stop, restart, rebuild, drain, and delete controls
- Queue-driven autoscaling and idle scale-down
- Host-wide CPU, memory, runner-count, disk, and log-storage guardrails with capacity leases
- Provisioning backoff, per-pool circuit breaking, and a global provisioning pause
- Minute-level available, busy, and queued capacity history with 24-hour, 7-day, and 30-day views
- Workflow runs, jobs, cancellation, reruns, downloadable logs, and runner log streaming
- Webhook-driven updates plus installation-token polling for localhost and webhook-outage recovery
- Signed, idempotent GitHub webhooks with delivery retry and audit history
- SQLite WAL persistence, retention policy, and consistent downloadable backups
- A continuously running reconciler that repairs desired versus actual runner state

## Architecture

- TanStack Router, React 19, Tailwind CSS 4, and shadcn-style UI components in the browser
- Rust/Axum control-plane API
- Rust/SQLx with SQLite migrations
- Rust/Bollard runner manager as the only service with Docker socket access and the provider gateway
- Authenticated native macOS agent for Tart VM lifecycle, capacity, and clean job-log streaming
- Rust background reconciler
- Axum serves the compiled client, same-origin API, and OAuth traffic; Traefik handles private ingress and TLS

TypeScript is confined to the browser application. All authentication, GitHub credentials, persistence, webhooks, runner orchestration, Docker access, and reconciliation live in Rust.

## GitHub App setup

Create a GitHub App with these repository permissions:

- Actions: read and write
- Administration: read and write
- Metadata: read-only

For organization-scoped pools, grant organization self-hosted runners read and write access. Subscribe to `workflow_job` and `workflow_run`; GitHub Apps receive `installation`, `installation_repositories`, and `github_app_authorization` automatically, so those events must not be included in a manifest's `default_events`. Enable expiring user access tokens.

Grant organization members read access as well. GridOps uses that permission to distinguish organization owners, who may manage runner infrastructure, from members with read-only visibility.

Configure:

- Callback URL: `${GRIDOPS_BASE_URL}/auth/github/callback`
- Webhook URL: `${GRIDOPS_BASE_URL}/api/webhooks/github`

After signing in, Settings can launch GitHub's App-manifest flow with the exact permissions and webhook events GridOps needs. GitHub returns the App ID, private key, OAuth credentials, slug, and webhook secret directly to GridOps; they are authenticated-encrypted in SQLite and become active without a restart. GridOps then reauthorizes with the newly-created App and sends completed installations through the same state-verified OAuth flow before synchronizing their repositories. Environment values remain supported as bootstrap or deployment-managed overrides.

For localhost development the generated manifest leaves webhook delivery disabled because GitHub cannot reach a loopback URL. Set `GRIDOPS_BASE_URL` to a public HTTPS origin before enabling deliveries. For a private HTTPS origin reachable only through Tailscale or another VPN, set `GRIDOPS_GITHUB_WEBHOOK_ACTIVE=false`; installation-token polling continues to synchronize workflow runs and jobs without inbound GitHub delivery. OAuth, App credentials, and runner control continue to work in both modes.

## Run with Docker

```sh
cp .env.example .env
# Fill the required values, then:
docker compose up --build -d
```

Open `http://localhost:3000`. For a different public origin, set `GRIDOPS_BASE_URL` and configure the same URL in the GitHub App.

The published API/UI port binds to `127.0.0.1` by default. Set `GRIDOPS_BIND_ADDRESS` explicitly only when direct host exposure is intended; reverse-proxy deployments should attach `api` to an ingress network instead.

For local credentials already stored in `.env.local`:

```sh
GRIDOPS_ENV_FILE=.env.local docker compose --env-file .env.local up --build
```

Only `manager` receives `/var/run/docker.sock`. Runner containers do not receive it. The API and reconciler share the `gridops-data` volume for SQLite and retained logs.

The manager is the authoritative capacity boundary. Before GitHub runner credentials are issued, the API or reconciler reserves a short-lived capacity lease; container creation consumes that lease and revalidates the configured runner network and disk watermark. By default GridOps reserves 25% of Docker-host CPU and memory, stops admitting runners below the larger of 25 GB or 15% free disk, limits runner logs to five 20 MB files, and derives a conservative global runner count from a 2 CPU / 2 GB runner shape. Override the `GRIDOPS_RUNNER_*` and `GRIDOPS_MIN_FREE_DISK_*` values from `.env.example` when the Docker host has a deliberately different envelope.

Apple Silicon hosts may additionally run ephemeral macOS pools through Tart. The provider is optional and remains disabled unless both its URL and bearer token are configured. See [docs/macos-runners.md](docs/macos-runners.md) for image preparation, native-agent installation, isolation, and capacity limits.

## Develop

Requirements: Node.js 22+, Rust 1.96+, Docker, and the values from `.env.example`.

Run the services in separate terminals:

```sh
set -a; source .env.local; set +a
cargo run -p gridops-manager
cargo run -p gridops-api
cargo run -p gridops-reconciler
npm run dev
```

The Vite server proxies `/api` and `/auth` to Axum at `127.0.0.1:8080`.

## Verify

```sh
npm run lint
npm test
npm run build
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
docker compose --env-file .env.local config
```

Back up the `gridops-data` volume or use the database backup control in Settings. SQLite runs in WAL mode; the download endpoint uses SQLite's consistent `VACUUM INTO` snapshot operation. Backups contain encrypted GitHub credentials, so treat them as sensitive and retain the matching `GRIDOPS_ENCRYPTION_KEY`; without that key the credentials cannot be recovered.
