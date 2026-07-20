# GridOps

GridOps is a self-hosted control plane for GitHub Actions runners. It connects to GitHub through a GitHub App and provides one place to provision Docker-based runners, manage runner pools, inspect workflow runs, and follow job activity and logs.

## Planned capabilities

- GitHub App authentication and repository installation management
- Repository and organization-scoped runner pools
- Ephemeral and persistent Docker runners
- Runner provisioning, draining, pausing, restarting, rebuilding, and deletion
- Labels, resource limits, concurrency, and autoscaling policies
- Workflow run and job monitoring
- Live log streaming, completed log retention, and downloads
- Run cancellation, reruns, and operational controls
- Runner image and update management
- Webhook delivery history, reconciliation, and audit trails
- Backup, recovery, retention, and notification settings

## Technology

- TanStack Start, Router, Query, Table, and Virtual
- Tailwind CSS and shadcn/ui
- SQLite
- Server-Sent Events for live operational updates
- Docker Compose for self-hosting

GridOps is under active development.
