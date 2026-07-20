# GridOps

GridOps is a self-hosted control plane for GitHub Actions runners. It connects through a GitHub App and provides one place to provision Docker-based runners, manage runner pools, inspect workflow activity, stream logs, and audit operational changes.

## Product scope

- GitHub App authentication, installations, and repository access
- Repository and organization-scoped runner pools
- Ephemeral and persistent Docker runners
- Provision, drain, pause, resume, restart, rebuild, and delete controls
- Labels, CPU and memory limits, concurrency, and autoscaling policies
- Workflow run and job monitoring, cancellation, and reruns
- Live logs, completed-log retention, and downloads
- Webhook delivery history, reconciliation, and audit trails
- Runner image, update, notification, backup, and recovery settings

## Stack

- TanStack Start, Router, Query, Table, and Virtual
- React 19, Tailwind CSS 4, and shadcn/ui
- SQLite with Drizzle ORM
- Server-Sent Events for live operational updates
- Docker Compose for self-hosting

## Development

1. Copy `.env.example` to `.env.local` and provide the GitHub App values.
2. Install dependencies with `npm install`.
3. Run `npm run db:migrate`.
4. Start GridOps with `npm run dev`.

The GitHub App callback URL is `${GRIDOPS_BASE_URL}/auth/github/callback`. The webhook URL is `${GRIDOPS_BASE_URL}/api/webhooks/github`.
