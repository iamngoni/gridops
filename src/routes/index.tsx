import { Link, createFileRoute } from "@tanstack/react-router";
import {
  Activity,
  ArrowRight,
  CheckCircle2,
  CircleGauge,
  CircleDot,
  Clock3,
  Github,
  Play,
  RefreshCw,
  Server,
  Settings2,
  TriangleAlert,
  Users,
  Workflow,
} from "lucide-react";
import { useEffect, useState } from "react";
import { Area, AreaChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";

import { AppShell } from "~/components/app-shell";
import { AsyncActionButton } from "~/components/async-action-button";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-badge";
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
import { getCapacityHistory, getDashboardOverview } from "~/features/dashboard/dashboard.functions";
import { syncGitHubAction } from "~/features/operations/operations.functions";
import type { CapacityHistory, CapacityWindow, DashboardOverview } from "~/features/dashboard/types";
import { formatDuration, formatRelativeTime } from "~/lib/utils";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/")({
  loader: () => getDashboardOverview(),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Operations overview"
      description="Monitor capacity, runners, and workflow activity across your GitHub installations."
      icon={CircleGauge}
    />
  ),
  component: OverviewPage,
});

function OverviewPage() {
  const data = Route.useLoaderData();
  useLiveRouteRefresh(10_000, data.authenticated);
  const syncGitHub = syncGitHubAction;
  const configurationComplete =
    data.configuration.githubOAuth &&
    data.configuration.githubAppControl &&
    (!data.configuration.webhookActive || data.configuration.webhookVerification) &&
    data.configuration.secureStorage &&
    data.configuration.runnerManager;

  return (
    <AppShell>
      <div className="flex flex-col gap-8">
        <div className="flex flex-col justify-between gap-5 xl:flex-row xl:items-end">
          <div>
            <p className="mb-2 text-[10px] font-semibold uppercase tracking-[0.14em] text-primary/75">GridOps control plane</p>
            <h1 className="text-2xl font-semibold tracking-[-0.025em] md:text-3xl">Operations overview</h1>
            <p className="mt-2 max-w-[62ch] text-sm leading-6 text-muted-foreground">
              Monitor capacity, runners, and workflow activity across your GitHub installations.
            </p>
          </div>
          <div className="flex items-center gap-2">
            {data.authenticated ? <AsyncActionButton action={() => syncGitHub()} icon={<RefreshCw />} success="GitHub installation access refreshed.">Refresh GitHub access</AsyncActionButton> : <a className={buttonVariants({ variant: "outline" })} href="/auth/github"><Github />Connect GitHub</a>}
            <Link className={buttonVariants()} to="/runner-pools/new"><Server />Provision runners</Link>
          </div>
        </div>

        {!configurationComplete && <ConfigurationBanner data={data} />}

        <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4" aria-label="Runner metrics">
          <MetricCard
            icon={Users}
            label="Managed runners"
            value={data.metrics.runners}
            footer={`${data.metrics.online} online`}
            tone="green"
            to="/runners"
          />
          <MetricCard
            icon={Activity}
            label="Busy runners"
            value={data.metrics.busy}
            footer="Running jobs now"
            tone="green"
            to="/runners"
          />
          <MetricCard
            icon={Clock3}
            label="Queued jobs"
            value={data.metrics.queuedJobs}
            footer={data.metrics.queuedJobs > 0 ? "Waiting to be assigned" : "Queue is clear"}
            tone="amber"
            to="/workflow-runs"
          />
          <MetricCard
            icon={CheckCircle2}
            label="Success rate"
            value={data.metrics.successRate === null ? "—" : `${data.metrics.successRate}%`}
            footer="Completed runs"
            tone="green"
            to="/workflow-runs"
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
    data.configuration.webhookActive && !data.configuration.webhookVerification && "webhook secret",
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
          <p className="text-sm font-medium">Complete the runner integration</p>
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
  to,
}: {
  icon: typeof Users;
  label: string;
  value: number | string;
  footer: string;
  tone: "green" | "amber";
  to: "/runners" | "/workflow-runs";
}) {
  return (
    <Link className="group block rounded-xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" to={to}>
    <Card className="min-h-36 overflow-hidden transition-[border-color,transform,box-shadow] group-hover:-translate-y-0.5 group-hover:border-primary/30 group-hover:shadow-[0_16px_36px_hsl(160_80%_2%/0.24),inset_0_1px_0_hsl(150_70%_90%/0.04)]">
      <CardContent className="flex h-full items-start gap-4 p-5">
        <div
          className={
            tone === "green"
              ? "grid size-10 place-items-center rounded-full bg-emerald-500/10 text-emerald-400"
              : "grid size-10 place-items-center rounded-full bg-amber-500/10 text-amber-400"
          }
        >
          <Icon className="size-5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-3"><p className="text-xs font-medium text-muted-foreground">{label}</p><ArrowRight className="size-3.5 text-muted-foreground/40 transition-transform group-hover:translate-x-0.5 group-hover:text-primary" /></div>
          <p className="mt-1 text-3xl font-semibold tracking-tight">{value}</p>
          <p className="mt-3 flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className={tone === "green" ? "size-1.5 rounded-full bg-emerald-400" : "size-1.5 rounded-full bg-amber-400"} />
            {footer}
          </p>
        </div>
      </CardContent>
    </Card>
    </Link>
  );
}

function CapacityPanel({ data }: { data: DashboardOverview }) {
  const [capacityWindow, setCapacityWindow] = useState<CapacityWindow>("24h");
  const [history, setHistory] = useState<CapacityHistory["points"]>([]);
  const [loading, setLoading] = useState(data.installations > 0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (data.installations === 0) {
      return undefined;
    }
    let cancelled = false;
    async function load() {
      try {
        const response = await getCapacityHistory(capacityWindow);
        if (!cancelled) {
          setHistory(response.points);
          setError(null);
        }
      } catch (cause) {
        if (!cancelled) setError(cause instanceof Error ? cause.message : "Could not load capacity history.");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    void load();
    const interval = capacityWindow === "24h" ? window.setInterval(() => void load(), 30_000) : undefined;
    return () => {
      cancelled = true;
      if (interval !== undefined) window.clearInterval(interval);
    };
  }, [capacityWindow, data.installations]);

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
          {(["24h", "7d", "30d"] as const).map((period) => (
            <Button aria-pressed={capacityWindow === period} key={period} onClick={() => { setLoading(true); setCapacityWindow(period); }} size="sm" variant={capacityWindow === period ? "secondary" : "ghost"}>{period}</Button>
          ))}
        </div>
      </CardHeader>
      <CardContent>
        <div className="capacity-grid relative min-h-64 overflow-hidden rounded-md border border-border/70 bg-background/30">
          {history.length > 0 ? (
            <div className="h-64 w-full p-3">
              <ResponsiveContainer height="100%" width="100%">
                <AreaChart data={history} margin={{ bottom: 0, left: -22, right: 8, top: 8 }}>
                  <defs>
                    <linearGradient id="available-fill" x1="0" x2="0" y1="0" y2="1"><stop offset="5%" stopColor="#34d399" stopOpacity={0.25} /><stop offset="95%" stopColor="#34d399" stopOpacity={0} /></linearGradient>
                    <linearGradient id="busy-fill" x1="0" x2="0" y1="0" y2="1"><stop offset="5%" stopColor="#60a5fa" stopOpacity={0.22} /><stop offset="95%" stopColor="#60a5fa" stopOpacity={0} /></linearGradient>
                    <linearGradient id="queued-fill" x1="0" x2="0" y1="0" y2="1"><stop offset="5%" stopColor="#fbbf24" stopOpacity={0.2} /><stop offset="95%" stopColor="#fbbf24" stopOpacity={0} /></linearGradient>
                  </defs>
                  <CartesianGrid stroke="rgba(148,163,184,0.10)" vertical={false} />
                  <XAxis axisLine={false} dataKey="recordedAt" minTickGap={40} tick={{ fill: "#718078", fontSize: 10 }} tickFormatter={(value: string) => formatCapacityTick(value, capacityWindow)} tickLine={false} />
                  <YAxis allowDecimals={false} axisLine={false} tick={{ fill: "#718078", fontSize: 10 }} tickLine={false} width={36} />
                  <Tooltip contentStyle={{ background: "#0d1512", border: "1px solid #24332d", borderRadius: 6, fontSize: 11 }} labelFormatter={(value) => new Date(String(value)).toLocaleString()} />
                  <Area dataKey="available" fill="url(#available-fill)" name="Available" stroke="#34d399" strokeWidth={2} type="monotone" />
                  <Area dataKey="busy" fill="url(#busy-fill)" name="Busy" stroke="#60a5fa" strokeWidth={2} type="monotone" />
                  <Area dataKey="queued" fill="url(#queued-fill)" name="Queued" stroke="#fbbf24" strokeWidth={2} type="monotone" />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          ) : (
            <div className="grid min-h-64 place-items-center">
              <div className="relative z-10 max-w-sm px-6 text-center">
                <Workflow className="mx-auto size-6 text-muted-foreground/70" />
                <p className="mt-3 text-sm font-medium">{data.installations === 0 ? "Connect GitHub to begin collecting capacity data" : loading ? "Loading capacity history…" : "Waiting for the first runner-pool sample"}</p>
                <p className="mt-1 text-xs leading-5 text-muted-foreground">Available, busy, and queued capacity is sampled every minute and retained for 31 days.</p>
              </div>
            </div>
          )}
          {error ? <div className="absolute inset-x-3 bottom-3 rounded border border-red-500/20 bg-red-500/10 px-3 py-2 text-[11px] text-red-300">{error}</div> : null}
        </div>
      </CardContent>
    </Card>
  );
}

function formatCapacityTick(value: string, window: CapacityWindow) {
  const date = new Date(value);
  return window === "24h"
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString([], { day: "numeric", month: "short" });
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
          <div className="divide-y divide-border/70">
            {data.activity.map((item) => (
              <ActivityRow item={item} key={item.id} />
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function ActivityRow({ item }: { item: DashboardOverview["activity"][number] }) {
  const content = <>
    <span className={`mt-1 grid size-7 shrink-0 place-items-center rounded-full ${item.level === "error" ? "bg-red-500/10 text-red-400" : item.level === "warning" ? "bg-amber-500/10 text-amber-400" : "bg-emerald-500/10 text-emerald-400"}`}><CircleDot className="size-3.5" /></span>
    <div className="min-w-0 flex-1">
      <p className="truncate text-sm font-medium">{item.event}</p>
      <p className="mt-1 truncate text-xs text-muted-foreground">{item.message}</p>
    </div>
    <time className="shrink-0 text-[11px] text-muted-foreground">{formatRelativeTime(item.createdAt)}</time>
    {(item.runnerId || item.poolId) ? <ArrowRight className="size-3.5 shrink-0 text-muted-foreground/35 transition-transform group-hover:translate-x-0.5 group-hover:text-primary" /> : null}
  </>;
  const className = "group flex items-start gap-3 rounded-lg px-2 py-3 first:pt-2 hover:bg-muted/45";
  if (item.runnerId) return <Link className={className} search={{ target: item.runnerId }} to="/live-logs">{content}</Link>;
  if (item.poolId) return <Link className={className} params={{ poolId: item.poolId }} to="/runner-pools/$poolId">{content}</Link>;
  return <div className={className}>{content}</div>;
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
                <TableHead>Destination</TableHead>
                <TableHead>Capacity</TableHead>
                <TableHead>Demand</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="w-10" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {data.pools.map((pool) => (
                <TableRow key={pool.id}>
                  <TableCell className="font-medium"><span className="mr-2 inline-block size-1.5 rounded-full bg-emerald-400" /><Link className="hover:text-primary" params={{ poolId: pool.id }} to="/runner-pools/$poolId">{pool.name}</Link></TableCell>
                  <TableCell><div className="capitalize text-xs">{pool.scope}</div><div className="mt-1 text-[11px] capitalize text-muted-foreground">{pool.mode} runners</div></TableCell>
                  <TableCell><div className="text-xs">{pool.desired} desired · {pool.online} online</div><div className="mt-1 text-[11px] text-muted-foreground">Provisioned capacity</div></TableCell>
                  <TableCell><div className="text-xs">{pool.busy} busy · {pool.queue} queued</div><div className="mt-1 text-[11px] text-muted-foreground">Current workload</div></TableCell>
                  <TableCell><StatusBadge status={pool.status} /></TableCell>
                  <TableCell><Link aria-label={`Open ${pool.name}`} className={buttonVariants({ size: "icon", variant: "ghost" })} params={{ poolId: pool.id }} title={`Open ${pool.name}`} to="/runner-pools/$poolId"><ArrowRight /></Link></TableCell>
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
              <Link className="group grid gap-3 px-5 py-3.5 transition-colors hover:bg-muted/45 sm:grid-cols-[minmax(0,1fr)_auto]" key={run.id} params={{ runId: String(run.id) }} to="/workflow-runs/$runId">
                <div className="min-w-0">
                  <p className="flex items-center gap-2 truncate text-sm font-medium"><Workflow className="size-4 shrink-0 text-primary" />{run.workflow}</p>
                  <p className="mt-1 truncate text-xs text-muted-foreground">{run.repository} · {run.branch ?? "detached"}</p>
                </div>
                <div className="flex items-center justify-between gap-4 sm:justify-end">
                  <span className="text-xs text-muted-foreground">{formatDuration(run.startedAt, run.completedAt)}</span>
                  <StatusBadge status={run.conclusion ?? run.status} />
                  <ArrowRight className="size-3.5 text-muted-foreground/35 transition-transform group-hover:translate-x-0.5 group-hover:text-primary" />
                </div>
              </Link>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
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
