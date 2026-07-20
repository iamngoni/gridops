import { Link, createFileRoute } from "@tanstack/react-router";
import { useServerFn } from "@tanstack/react-start";
import {
  Activity,
  ArrowRight,
  CheckCircle2,
  CircleDot,
  Clock3,
  Github,
  MoreHorizontal,
  Play,
  RefreshCw,
  Server,
  Settings2,
  TriangleAlert,
  Users,
  Workflow,
} from "lucide-react";

import { AppShell } from "~/components/app-shell";
import { AsyncActionButton } from "~/components/async-action-button";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "~/components/ui/table";
import { getDashboardOverview } from "~/features/dashboard/dashboard.functions";
import { syncGitHubAction } from "~/features/operations/operations.functions";
import type { DashboardOverview } from "~/features/dashboard/types";
import { formatDuration, formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/")({
  loader: () => getDashboardOverview(),
  component: OverviewPage,
});

function OverviewPage() {
  const data = Route.useLoaderData();
  const syncGitHub = useServerFn(syncGitHubAction);
  const configurationComplete =
    data.configuration.githubOAuth &&
    data.configuration.githubAppControl &&
    data.configuration.webhookVerification &&
    data.configuration.secureStorage &&
    data.configuration.runnerManager;

  return (
    <AppShell>
      <div className="flex flex-col gap-6">
        <div className="flex flex-col justify-between gap-4 xl:flex-row xl:items-center">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight md:text-3xl">Operations overview</h1>
            <p className="mt-1 text-sm text-muted-foreground">
              Monitor capacity, runners, and workflow activity across your GitHub installations.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <AsyncActionButton action={() => syncGitHub()} icon={<RefreshCw />} success="GitHub installations and repositories synced.">Sync GitHub</AsyncActionButton>
            <Link className={buttonVariants()} to="/runner-pools/new"><Server />Provision runners</Link>
          </div>
        </div>

        {!configurationComplete && <ConfigurationBanner data={data} />}

        <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" aria-label="Runner metrics">
          <MetricCard
            icon={Users}
            label="Runners"
            value={data.metrics.runners}
            footer={`${data.metrics.online} online`}
            tone="green"
          />
          <MetricCard
            icon={Activity}
            label="Busy"
            value={data.metrics.busy}
            footer="Running jobs now"
            tone="green"
          />
          <MetricCard
            icon={Clock3}
            label="Queued jobs"
            value={data.metrics.queuedJobs}
            footer={data.metrics.queuedJobs > 0 ? "Waiting to be assigned" : "Queue is clear"}
            tone="amber"
          />
          <MetricCard
            icon={CheckCircle2}
            label="Success rate"
            value={data.metrics.successRate === null ? "—" : `${data.metrics.successRate}%`}
            footer="Completed runs"
            tone="green"
          />
        </section>

        <section className="grid gap-4 2xl:grid-cols-[minmax(0,1.45fr)_minmax(360px,0.8fr)]">
          <CapacityPanel data={data} />
          <ActivityPanel data={data} />
        </section>

        <section className="grid gap-4 2xl:grid-cols-[minmax(0,1.1fr)_minmax(520px,1fr)]">
          <RunnerPoolsPanel data={data} />
          <WorkflowRunsPanel data={data} />
        </section>
      </div>
    </AppShell>
  );
}

function ConfigurationBanner({ data }: { data: DashboardOverview }) {
  const missing = [
    !data.configuration.githubOAuth && "OAuth credentials",
    !data.configuration.githubAppControl && "App ID and private key",
    !data.configuration.webhookVerification && "webhook secret",
    !data.configuration.secureStorage && "secure storage keys",
    !data.configuration.runnerManager && "runner manager token",
  ].filter(Boolean) as string[];

  return (
    <Card className="border-amber-500/25 bg-amber-500/[0.04]">
      <CardContent className="flex flex-col gap-4 p-4 sm:flex-row sm:items-center">
        <div className="grid size-10 shrink-0 place-items-center rounded-md border border-amber-500/20 bg-amber-500/10 text-amber-400">
          <TriangleAlert className="size-5" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium">Finish GitHub App configuration</p>
          <p className="mt-1 text-xs text-muted-foreground">
            Still required: {missing.join(", ")}. GridOps keeps operational controls disabled until secure credentials are complete.
          </p>
        </div>
        <Link className={buttonVariants({ variant: "outline" })} to="/settings"><Settings2 />Open setup</Link>
      </CardContent>
    </Card>
  );
}

function MetricCard({
  icon: Icon,
  label,
  value,
  footer,
  tone,
}: {
  icon: typeof Users;
  label: string;
  value: number | string;
  footer: string;
  tone: "green" | "amber";
}) {
  return (
    <Card className="min-h-32">
      <CardContent className="flex h-full items-start gap-4 p-4">
        <div
          className={
            tone === "green"
              ? "grid size-10 place-items-center rounded-full bg-emerald-500/10 text-emerald-400"
              : "grid size-10 place-items-center rounded-full bg-amber-500/10 text-amber-400"
          }
        >
          <Icon className="size-5" />
        </div>
        <div className="min-w-0">
          <p className="text-xs text-muted-foreground">{label}</p>
          <p className="mt-1 text-3xl font-semibold tracking-tight">{value}</p>
          <p className="mt-3 flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className={tone === "green" ? "size-1.5 rounded-full bg-emerald-400" : "size-1.5 rounded-full bg-amber-400"} />
            {footer}
          </p>
        </div>
      </CardContent>
    </Card>
  );
}

function CapacityPanel({ data }: { data: DashboardOverview }) {
  return (
    <Card>
      <CardHeader>
        <div>
          <CardTitle>Runner capacity</CardTitle>
          <div className="mt-3 flex items-center gap-4 text-[11px] text-muted-foreground">
            <Legend color="bg-emerald-400" label="Available" />
            <Legend color="bg-blue-400" label="Busy" />
            <Legend color="bg-amber-400" label="Queued" />
          </div>
        </div>
        <div className="flex gap-1">
          <Button size="sm" variant="secondary">24h</Button>
          <Button size="sm" variant="ghost">7d</Button>
          <Button size="sm" variant="ghost">30d</Button>
        </div>
      </CardHeader>
      <CardContent>
        <div className="capacity-grid relative flex min-h-64 items-center justify-center overflow-hidden rounded-md border border-border/70 bg-background/30">
          <div className="relative z-10 max-w-sm px-6 text-center">
            <Workflow className="mx-auto size-6 text-muted-foreground/70" />
            <p className="mt-3 text-sm font-medium">
              {data.installations === 0 ? "Connect GitHub to begin collecting capacity data" : "Capacity history is being collected"}
            </p>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">
              Available, busy, and queued capacity appears here as runner pools process workflow jobs.
            </p>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function Legend({ color, label }: { color: string; label: string }) {
  return <span className="flex items-center gap-1.5"><span className={`size-1.5 rounded-full ${color}`} />{label}</span>;
}

function ActivityPanel({ data }: { data: DashboardOverview }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Live activity</CardTitle>
        <LinkButton label="View all" to="/live-logs" />
      </CardHeader>
      <CardContent>
        {data.activity.length === 0 ? (
          <EmptyState icon={CircleDot} title="No runner activity yet" body="Runner lifecycle and assignment events will stream here." />
        ) : (
          <div className="divide-y divide-border">
            {data.activity.map((item) => (
              <div className="flex gap-3 py-3 first:pt-0" key={item.id}>
                <span className="mt-1.5 size-2 shrink-0 rounded-full bg-emerald-400" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">{item.event}</p>
                  <p className="mt-1 truncate text-xs text-muted-foreground">{item.message}</p>
                </div>
                <time className="shrink-0 text-[11px] text-muted-foreground">{formatRelativeTime(item.createdAt)}</time>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function RunnerPoolsPanel({ data }: { data: DashboardOverview }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Runner pools</CardTitle>
        <LinkButton label="Manage pools" to="/runner-pools" />
      </CardHeader>
      <CardContent className="px-0 pb-0">
        {data.pools.length === 0 ? (
          <div className="px-4 pb-4">
            <EmptyState icon={Server} title="No runner pools" body="Create a pool after connecting a GitHub installation." action="Create pool" actionHref="/runner-pools/new" />
          </div>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Pool</TableHead>
                <TableHead>Scope</TableHead>
                <TableHead>Desired</TableHead>
                <TableHead>Online</TableHead>
                <TableHead>Busy</TableHead>
                <TableHead>Queue</TableHead>
                <TableHead>Mode</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="w-10" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.pools.map((pool) => (
                <TableRow key={pool.id}>
                  <TableCell className="font-medium"><span className="mr-2 inline-block size-1.5 rounded-full bg-emerald-400" />{pool.name}</TableCell>
                  <TableCell className="text-muted-foreground">{pool.scope}</TableCell>
                  <TableCell>{pool.desired}</TableCell>
                  <TableCell>{pool.online}</TableCell>
                  <TableCell>{pool.busy}</TableCell>
                  <TableCell>{pool.queue}</TableCell>
                  <TableCell className="capitalize text-muted-foreground">{pool.mode}</TableCell>
                  <TableCell><StatusBadge status={pool.status} /></TableCell>
                  <TableCell><Button aria-label={`Actions for ${pool.name}`} size="icon" variant="ghost"><MoreHorizontal /></Button></TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  );
}

function WorkflowRunsPanel({ data }: { data: DashboardOverview }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Workflow runs</CardTitle>
        <LinkButton label="View all runs" to="/workflow-runs" />
      </CardHeader>
      <CardContent className="px-0 pb-0">
        {data.runs.length === 0 ? (
          <div className="px-4 pb-4">
            <EmptyState icon={Play} title="No workflow runs synced" body="Recent runs appear after a repository installation is connected." />
          </div>
        ) : (
          <div className="divide-y divide-border">
            {data.runs.map((run) => (
              <div className="grid gap-3 px-4 py-3 sm:grid-cols-[minmax(0,1fr)_auto]" key={run.id}>
                <div className="min-w-0">
                  <p className="flex items-center gap-2 truncate text-sm font-medium"><Github className="size-4 shrink-0" />{run.repository}</p>
                  <p className="mt-1 truncate text-xs text-muted-foreground">{run.workflow} · {run.branch ?? "detached"}</p>
                </div>
                <div className="flex items-center justify-between gap-4 sm:justify-end">
                  <span className="text-xs text-muted-foreground">{formatDuration(run.startedAt, run.completedAt)}</span>
                  <StatusBadge status={run.conclusion ?? run.status} />
                </div>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function StatusBadge({ status }: { status: string }) {
  const normalized = status.toLowerCase();
  const variant = normalized === "success" || normalized === "active" || normalized === "healthy"
    ? "success"
    : normalized === "queued" || normalized === "paused" || normalized === "draining"
      ? "warning"
      : normalized === "failure" || normalized === "failed"
        ? "destructive"
        : normalized === "in_progress" || normalized === "running"
          ? "info"
          : "outline";
  return <Badge variant={variant}>{normalized.replaceAll("_", " ")}</Badge>;
}

function EmptyState({
  icon: Icon,
  title,
  body,
  action,
  actionHref,
}: {
  icon: typeof Server;
  title: string;
  body: string;
  action?: string;
  actionHref?: string;
}) {
  return (
    <div className="flex min-h-48 flex-col items-center justify-center rounded-md border border-dashed border-border bg-background/25 px-6 py-8 text-center">
      <div className="grid size-9 place-items-center rounded-md border border-border bg-muted text-muted-foreground"><Icon className="size-4" /></div>
      <p className="mt-3 text-sm font-medium">{title}</p>
      <p className="mt-1 max-w-xs text-xs leading-5 text-muted-foreground">{body}</p>
      {action && actionHref ? <Link className={`${buttonVariants({ size: "sm", variant: "outline" })} mt-4`} to={actionHref}>{action}<ArrowRight /></Link> : null}
    </div>
  );
}

function LinkButton({ label, to }: { label: string; to: string }) {
  return <Link className="text-xs font-medium text-primary hover:underline" to={to}>{label}</Link>;
}
