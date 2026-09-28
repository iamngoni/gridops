import { Link, createFileRoute } from "@tanstack/react-router";
import { ArrowRight, ArrowUpRight, Boxes, CircleGauge, Github, Plus, Radio, RefreshCw, Settings2, Workflow } from "lucide-react";
import { useEffect, useState } from "react";
import { Area, AreaChart, CartesianGrid, ResponsiveContainer, Tooltip as ChartTooltip, XAxis, YAxis } from "recharts";

import { AsyncActionButton } from "~/components/async-action-button";
import { CapacityMeter } from "~/components/capacity-meter";
import { Callout, EmptyState, PageBody, PageHeader, SectionHeading, listRowClassName } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusDot } from "~/components/status-icon";
import { buttonVariants } from "~/components/ui/button";
import { getCapacityHistory, getDashboardOverview } from "~/features/dashboard/dashboard.functions";
import type { CapacityHistory, CapacityWindow, DashboardOverview } from "~/features/dashboard/types";
import { syncGitHubAction } from "~/features/operations/operations.functions";
import { humanizeEvent } from "~/features/runner-pools/pool-events";
import { RunRow } from "~/features/workflow-runs/run-row";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";
import { cn, formatAge } from "~/lib/utils";

export const Route = createFileRoute("/_app/")({
  loader: () => getDashboardOverview(),
  pendingComponent: () => <ResourcePageLoading icon={CircleGauge} title="Overview" />,
  component: OverviewPage,
});

function OverviewPage() {
  const data = Route.useLoaderData();
  useLiveRouteRefresh(10_000, data.authenticated);
  const configurationComplete =
    data.configuration.githubOAuth &&
    data.configuration.githubAppControl &&
    (!data.configuration.webhookActive || data.configuration.webhookVerification) &&
    data.configuration.secureStorage &&
    data.configuration.runnerManager;

  return (
    <>
      <PageHeader
        actions={
          <>
            {data.authenticated ? (
              <AsyncActionButton action={() => syncGitHubAction()} icon={<RefreshCw />} success="GitHub installation access refreshed." variant="ghost">
                <span className="hidden sm:inline">Refresh GitHub</span>
              </AsyncActionButton>
            ) : (
              <a className={buttonVariants({ size: "sm", variant: "outline" })} href="/auth/github"><Github />Connect GitHub</a>
            )}
            <Link className={buttonVariants({ size: "sm" })} to="/runner-pools/new"><Plus />New pool</Link>
          </>
        }
        icon={CircleGauge}
        title="Overview"
      />
      <PageBody>
        <div className="w-full space-y-8 px-4 py-6 sm:px-6 lg:px-8">
          {!configurationComplete ? <ConfigurationCallout data={data} /> : null}
          <MetricStrip data={data} />
          <div className="grid gap-8 xl:grid-cols-[minmax(0,1.55fr)_minmax(340px,1fr)]">
            <div className="min-w-0 space-y-8">
              <CapacitySection installations={data.installations} />
              <PoolsSection pools={data.pools} />
              <RunsSection runs={data.runs} />
            </div>
            <div className="min-w-0 space-y-8">
              {data.authenticated ? <AttentionSection data={data} /> : null}
              <ActivitySection activity={data.activity} />
            </div>
          </div>
        </div>
      </PageBody>
    </>
  );
}

function ConfigurationCallout({ data }: { data: DashboardOverview }) {
  const missing = [
    !data.configuration.githubOAuth && "OAuth credentials",
    !data.configuration.githubAppControl && "App ID and private key",
    data.configuration.webhookActive && !data.configuration.webhookVerification && "webhook secret",
    !data.configuration.secureStorage && "secure storage keys",
    !data.configuration.runnerManager && "runner manager token",
  ].filter(Boolean) as string[];
  return (
    <Callout
      action={<Link className={buttonVariants({ size: "sm", variant: "outline" })} to="/settings"><Settings2 />Open setup</Link>}
      title="Finish connecting GridOps"
      tone="warning"
    >
      Still required: {missing.join(", ")}. Operational controls stay disabled until credentials are complete.
    </Callout>
  );
}

function MetricStrip({ data }: { data: DashboardOverview }) {
  const metrics = [
    { label: "Runners", value: data.metrics.runners, hint: `${data.metrics.online} online`, to: "/runners", tone: data.metrics.online > 0 ? "success" : "neutral" },
    { label: "Busy", value: data.metrics.busy, hint: "Running jobs now", to: "/runners", tone: data.metrics.busy > 0 ? "progress" : "neutral" },
    { label: "Queued jobs", value: data.metrics.queuedJobs, hint: data.metrics.queuedJobs > 0 ? "Waiting for a runner" : "Queue is clear", to: "/workflow-runs", tone: data.metrics.queuedJobs > 0 ? "warning" : "success" },
    { label: "Success rate", value: data.metrics.successRate === null ? "—" : `${data.metrics.successRate}%`, hint: "Completed runs", to: "/workflow-runs", tone: "neutral" },
  ] as const;
  return (
    <section aria-label="Runner metrics" className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-border xl:grid-cols-4">
      {metrics.map((metric) => (
        <Link className="group flex flex-col gap-1 bg-panel px-4 py-3.5 transition-colors hover:bg-hover" key={metric.label} to={metric.to}>
          <span className="flex items-center gap-2 text-xs text-muted-foreground">
            <StatusDot pulse={false} tone={metric.tone} />
            <span className="truncate">{metric.label}</span>
            <ArrowUpRight className="ml-auto size-3.5 shrink-0 text-faint opacity-0 transition-opacity group-hover:opacity-100" />
          </span>
          <span className="tabular text-2xl font-semibold tracking-tight text-foreground">{metric.value}</span>
          <span className="truncate text-xs text-faint">{metric.hint}</span>
        </Link>
      ))}
    </section>
  );
}

function ViewAll({ to, label = "View all" }: { to: string; label?: string }) {
  return <Link className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-hover hover:text-foreground" to={to}>{label}<ArrowRight className="size-3" /></Link>;
}

function CapacitySection({ installations }: { installations: number }) {
  const [capacityWindow, setCapacityWindow] = useState<CapacityWindow>("24h");
  const [history, setHistory] = useState<CapacityHistory["points"]>([]);
  const [loading, setLoading] = useState(installations > 0);
  const [error, setError] = useState<string | null>(null);
  const current = history.at(-1);

  useEffect(() => {
    if (installations === 0) return undefined;
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
  }, [capacityWindow, installations]);

  return (
    <section className="space-y-3">
      <SectionHeading
        actions={
          <div className="flex rounded-md border border-border-strong p-0.5">
            {(["24h", "7d", "30d"] as const).map((period) => (
              <button
                aria-pressed={capacityWindow === period}
                className={cn("h-6 rounded-[4px] px-2 text-xs font-medium transition-colors", capacityWindow === period ? "bg-selected text-foreground" : "text-muted-foreground hover:text-foreground")}
                key={period}
                onClick={() => { setLoading(true); setCapacityWindow(period); }}
                type="button"
              >
                {period}
              </button>
            ))}
          </div>
        }
      >
        Capacity
      </SectionHeading>
      <div className="rounded-lg border border-border">
        <div className="flex flex-wrap items-center gap-x-5 gap-y-1 border-b border-border px-4 py-2.5 text-xs text-muted-foreground">
          <Legend color="var(--success)" label="Available" value={current?.available} />
          <Legend color="var(--info)" label="Busy" value={current?.busy} />
          <Legend color="var(--warning)" label="Queued" value={current?.queued} />
          {error ? <span className="ml-auto text-danger">{error}</span> : null}
        </div>
        {history.length > 0 ? (
          <div className="h-60 w-full px-2 pb-2 pt-4">
            <ResponsiveContainer height="100%" width="100%">
              <AreaChart data={history} margin={{ bottom: 0, left: 0, right: 12, top: 4 }}>
                <defs>
                  {(["success", "info", "warning"] as const).map((tone) => (
                    <linearGradient id={`capacity-${tone}`} key={tone} x1="0" x2="0" y1="0" y2="1">
                      <stop offset="0%" stopColor={`var(--${tone})`} stopOpacity={0.22} />
                      <stop offset="100%" stopColor={`var(--${tone})`} stopOpacity={0} />
                    </linearGradient>
                  ))}
                </defs>
                <CartesianGrid stroke="var(--border)" vertical={false} />
                <XAxis axisLine={false} dataKey="recordedAt" minTickGap={48} tick={{ fill: "var(--faint)", fontSize: 11 }} tickFormatter={(value: string) => formatCapacityTick(value, capacityWindow)} tickLine={false} />
                <YAxis allowDecimals={false} axisLine={false} tick={{ fill: "var(--faint)", fontSize: 11 }} tickLine={false} tickMargin={4} width={32} />
                <ChartTooltip
                  contentStyle={{ background: "var(--popover)", border: "1px solid var(--border-strong)", borderRadius: 8, boxShadow: "var(--popover-shadow)", color: "var(--foreground)", fontSize: 12 }}
                  cursor={{ stroke: "var(--border-strong)" }}
                  labelFormatter={(value) => new Date(String(value)).toLocaleString()}
                />
                <Area dataKey="available" isAnimationActive={false} fill="url(#capacity-success)" name="Available" stroke="var(--success)" strokeWidth={1.5} type="monotone" />
                <Area dataKey="busy" isAnimationActive={false} fill="url(#capacity-info)" name="Busy" stroke="var(--info)" strokeWidth={1.5} type="monotone" />
                <Area dataKey="queued" isAnimationActive={false} fill="url(#capacity-warning)" name="Queued" stroke="var(--warning)" strokeWidth={1.5} type="monotone" />
              </AreaChart>
            </ResponsiveContainer>
          </div>
        ) : (
          <div className="grid h-60 place-items-center px-6 text-center">
            <div>
              <Workflow className="mx-auto size-5 text-faint" />
              <p className="mt-3 text-sm font-medium">{installations === 0 ? "Connect GitHub to collect capacity data" : loading ? "Loading capacity history…" : "Waiting for the first capacity sample"}</p>
              <p className="mt-1 text-xs text-muted-foreground">Available, busy, and queued capacity is sampled every minute and kept for 31 days.</p>
            </div>
          </div>
        )}
      </div>
    </section>
  );
}

function formatCapacityTick(value: string, window: CapacityWindow) {
  const date = new Date(value);
  return window === "24h"
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString([], { day: "numeric", month: "short" });
}

function Legend({ color, label, value }: { color: string; label: string; value?: number }) {
  return <span className="flex items-center gap-1.5"><span className="size-2 rounded-full" style={{ backgroundColor: color }} />{label}<span className="tabular font-medium text-foreground">{value ?? "—"}</span></span>;
}

function PoolsSection({ pools }: { pools: DashboardOverview["pools"] }) {
  return (
    <section className="space-y-3">
      <SectionHeading actions={<ViewAll to="/runner-pools" />}>Runner pools</SectionHeading>
      <div className="overflow-hidden rounded-lg border border-border">
        {pools.length === 0 ? (
          <EmptyState className="min-h-0 py-10" description="Create a pool after connecting a GitHub installation." icon={Boxes} title="No runner pools">
            <Link className={buttonVariants({ size: "sm", variant: "outline" })} to="/runner-pools/new"><Plus />Create pool</Link>
          </EmptyState>
        ) : pools.map((pool) => (
          <Link className={cn(listRowClassName, "last:border-b-0")} key={pool.id} params={{ poolId: pool.id }} to="/runner-pools/$poolId">
            <span className="inline-flex size-3.5 items-center justify-center"><StatusDot status={pool.status} /></span>
            <span className="min-w-0 flex-1 truncate font-medium">{pool.name}</span>
            <span className="hidden text-xs capitalize text-muted-foreground sm:inline">{pool.scope} · {pool.mode}</span>
            <CapacityMeter busy={pool.busy} desired={pool.desired} online={pool.online} />
            <span className={cn("tabular w-20 text-right text-xs", pool.queue > 0 ? "text-warning" : "text-faint")}>{pool.queue} queued</span>
          </Link>
        ))}
      </div>
    </section>
  );
}

function RunsSection({ runs }: { runs: DashboardOverview["runs"] }) {
  return (
    <section className="space-y-3">
      <SectionHeading actions={<ViewAll to="/workflow-runs" />}>Recent runs</SectionHeading>
      <div className="overflow-hidden rounded-lg border border-border [&>*:last-child]:border-b-0">
        {runs.length === 0 ? (
          <EmptyState className="min-h-0 py-10" description="Runs appear once a repository installation is connected." icon={Workflow} title="No workflow runs synced" />
        ) : runs.map((run) => (
          <RunRow key={run.id} run={{ id: run.id, workflow: run.workflow, repository: run.repository, branch: run.branch, status: run.status, conclusion: run.conclusion, startedAt: run.startedAt, completedAt: run.completedAt }} />
        ))}
      </div>
    </section>
  );
}

function AttentionSection({ data }: { data: DashboardOverview }) {
  const { alerts, failures, queue, startLatency, windowHours } = data.slo;
  return (
    <section className="space-y-3">
      <SectionHeading actions={<span className="text-xs text-faint">Last {windowHours}h</span>}>Service level</SectionHeading>
      <div className="grid grid-cols-3 overflow-hidden rounded-lg border border-border">
        <SloCell hint={queue.p95Seconds === null ? "Queue is clear" : `p95 ${formatSeconds(queue.p95Seconds)}`} label="Oldest queued" value={formatSeconds(queue.oldestSeconds)} />
        <SloCell className="border-l" hint={startLatency.sampleSize ? `p95 ${formatSeconds(startLatency.p95Seconds)} · ${startLatency.sampleSize} starts` : "No starts"} label="Start latency" value={formatSeconds(startLatency.p50Seconds)} />
        <SloCell className="border-l" hint={failures.length ? failures.map((item) => `${item.count} ${item.reason}`).join(" · ") : "None"} label="Failures" value={String(failures.reduce((total, item) => total + item.count, 0))} />
      </div>
      <div className="overflow-hidden rounded-lg border border-border">
        <div className="flex h-9 items-center gap-2 border-b border-border bg-panel-subtle px-4 text-xs font-medium">
          Needs attention<span className="tabular text-faint">{alerts.length}</span>
        </div>
        {alerts.length ? alerts.map((alert) => (
          <Link className={cn(listRowClassName, "items-start py-3 last:border-b-0")} key={`${alert.href}-${alert.title}`} to={alert.href}>
            <StatusDot className="mt-1.5" tone={alert.level === "error" ? "danger" : "warning"} />
            <span className="min-w-0 flex-1">
              <span className="block font-medium">{alert.title}</span>
              <span className="mt-0.5 block text-xs leading-5 text-muted-foreground">{alert.detail}</span>
            </span>
            <ArrowRight className="mt-1 size-3.5 shrink-0 text-faint" />
          </Link>
        )) : (
          <div className="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground"><StatusDot tone="success" />No active runner-control incidents.</div>
        )}
      </div>
    </section>
  );
}

function SloCell({ label, value, hint, className }: { label: string; value: string; hint: string; className?: string }) {
  return (
    <div className={cn("min-w-0 px-3 py-3", className)}>
      <div className="truncate text-xs text-muted-foreground">{label}</div>
      <div className="tabular mt-1 text-lg font-semibold tracking-tight">{value}</div>
      <div className="mt-0.5 truncate text-2xs text-faint" title={hint}>{hint}</div>
    </div>
  );
}

function formatSeconds(value: number | null) {
  if (value === null) return "—";
  if (value >= 3_600) return `${Math.floor(value / 3_600)}h ${Math.floor((value % 3_600) / 60)}m`;
  if (value >= 60) return `${Math.floor(value / 60)}m ${value % 60}s`;
  return `${value}s`;
}

function ActivitySection({ activity }: { activity: DashboardOverview["activity"] }) {
  return (
    <section className="space-y-3">
      <SectionHeading actions={<ViewAll label="Live logs" to="/live-logs" />}>Activity</SectionHeading>
      <div className="overflow-hidden rounded-lg border border-border">
        {activity.length === 0 ? (
          <div className="flex items-center gap-2 px-4 py-6 text-sm text-muted-foreground"><Radio className="size-4 text-faint" />Runner lifecycle and assignment events stream here.</div>
        ) : activity.map((item) => <ActivityItem item={item} key={item.id} />)}
      </div>
    </section>
  );
}

function ActivityItem({ item }: { item: DashboardOverview["activity"][number] }) {
  const tone = item.level === "error" ? "danger" : item.level === "warning" ? "warning" : "success";
  const body = (
    <>
      <StatusDot pulse={false} tone={tone} />
      <span className="max-w-[45%] shrink-0 truncate font-medium text-foreground">{humanizeEvent(item.event)}</span>
      <span className="min-w-0 flex-1 truncate text-muted-foreground" title={item.message}>{item.message}</span>
      <time className="tabular shrink-0 text-xs text-faint" dateTime={item.createdAt}>{formatAge(item.createdAt)}</time>
    </>
  );
  const className = cn(listRowClassName, "min-h-10 last:border-b-0");
  if (item.runnerId) return <Link className={className} search={{ target: item.runnerId }} to="/live-logs">{body}</Link>;
  if (item.poolId) return <Link className={className} params={{ poolId: item.poolId }} to="/runner-pools/$poolId">{body}</Link>;
  return <div className={className}>{body}</div>;
}
