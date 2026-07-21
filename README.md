# GridOps

GridOps is a self-hosted control plane for GitHub Actions runners. Connect a GitHub App, select repositories or organizations, and operate isolated Docker runner pools from one interface.

## Capabilities

- GitHub App OAuth, installations, encrypted user tokens, and short-lived installation tokens
- Repository and organization-scoped runner pools
- Ephemeral and persistent Docker runners with labels, CPU, memory, PID, and capability limits
- Editable pool configuration with generation-tracked, busy-safe rolling runner replacement
- Provision, scale, reconcile, pause, resume, stop, restart, rebuild, drain, and delete controls
- Queue-driven autoscaling and idle scale-down
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
- Rust/Bollard runner manager as the only service with Docker socket access
- Rust background reconciler
- Nginx serves the client and proxies same-origin API and OAuth traffic

TypeScript is confined to the browser application. All authentication, GitHub credentials, persistence, webhooks, runner orchestration, Docker access, and reconciliation live in Rust.

## GitHub App setup

Create a GitHub App with these repository permissions:

- Actions: read and write
- Administration: read and write
- Metadata: read-only

For organization-scoped pools, grant organization self-hosted runners read and write access. Subscribe to `installation`, `installation_repositories`, `workflow_job`, `workflow_run`, and `github_app_authorization` events. Enable expiring user access tokens.

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

The published web port binds to `127.0.0.1` by default. Set `GRIDOPS_BIND_ADDRESS` explicitly only when direct host exposure is intended; reverse-proxy deployments should attach `web` to an ingress network instead.

For local credentials already stored in `.env.local`:

```sh
GRIDOPS_ENV_FILE=.env.local docker compose --env-file .env.local up --build
```

### `ops.antonlabs.cc`

The private Anton Labs deployment uses `compose.ops.yaml` to attach the web container to the existing `media-server_default` ingress network. Its `.env.local` sets `GRIDOPS_BASE_URL=https://ops.antonlabs.cc`, `GRIDOPS_GITHUB_WEBHOOK_ACTIVE=false`, `GRIDOPS_BIND_ADDRESS=127.0.0.1`, and `GRIDOPS_PORT=3002`.

```sh
GRIDOPS_ENV_FILE=.env.local \
  docker compose --env-file .env.local \
  -f compose.yaml -f compose.ops.yaml up --build -d
```

Install `deploy/traefik/ops.antonlabs.cc.yml` in the host Traefik file-provider directory. The Cloudflare record must remain an unproxied `A` record to the Mac mini's Tailscale IP and must not be added to the public Cloudflare tunnel. This keeps the hostname on the same private lane as the other admin services while preserving a valid wildcard TLS certificate.

Only `manager` receives `/var/run/docker.sock`. Runner containers do not receive it. The API and reconciler share the `gridops-data` volume for SQLite and retained logs.

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
