import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute, getRouteApi } from "@tanstack/react-router";
import { Activity, ArrowRight, Server } from "lucide-react";

import { CapacityMeter } from "~/components/capacity-meter";
import { Callout, InlineLoading, ListGroup, PropertiesPanel, PropertyGroup, PropertyRow, SectionHeading } from "~/components/page";
import { StatusBadge, StatusDot, statusLabel } from "~/components/status-icon";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Badge } from "~/components/ui/badge";
import { buttonVariants } from "~/components/ui/button";
import { getRunnersPage } from "~/features/operations/operations.functions";
import type { RunnerPoolDetail } from "~/features/runner-pools/runner-pools.functions";
import { providerLabel } from "~/features/runner-pools/pool-actions";
import { PoolEventTable, usePoolEvents } from "~/features/runner-pools/pool-events";
import { RunnerRow, groupRunners } from "~/features/runners/runner-row";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/_app/runner-pools/$poolId/")({
  component: PoolOverviewTab,
});

const poolRoute = getRouteApi("/_app/runner-pools/$poolId");

/** Explains why the pool is not provisioning, distinguishing failures from a full host. */
function ProvisioningCallout({ pool }: { pool: RunnerPoolDetail }) {
  const retry = pool.provisionRetryAt ? new Date(pool.provisionRetryAt) : null;
  const when = retry ? `at ${retry.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : "shortly";
  let content: { title: string; body: string; tone: "danger" | "warning" | "neutral" } | null = null;
  if (pool.provisionCircuitOpen) {
    content = { title: `Provisioning paused after ${pool.provisionFailureCount} consecutive failures`, body: `GridOps tries again ${when}. The Activity tab shows the failing step; use the pool menu to retry now.`, tone: "danger" };
  } else if (pool.provisionFailureCount > 0) {
    content = { title: "Provisioning is backing off", body: `The last ${pool.provisionFailureCount === 1 ? "attempt" : `${pool.provisionFailureCount} attempts`} failed. GridOps retries ${when}.`, tone: "warning" };
  } else if (pool.state === "waiting") {
    content = { title: "Waiting for host capacity", body: `The runner host is at its safe limit. GridOps starts more runners as capacity frees up, next check ${when}.`, tone: "neutral" };
  }
  if (!content) return null;
  return (
    <div className="border-b border-border px-4 py-3">
      <Callout title={content.title} tone={content.tone}>{content.body}</Callout>
    </div>
  );
}

function PoolOverviewTab() {
  const pool = poolRoute.useLoaderData();
  useLiveRouteRefresh(5_000);
  const runners = useQuery({
    queryKey: ["pool-runners", pool.id],
    queryFn: () => getRunnersPage({ pool: pool.id, page: 1 }),
    refetchInterval: 5_000,
  });
  const events = usePoolEvents(pool.id, 1);
  const providers = pool.providers?.length ? pool.providers : [pool.provider];
  const items = runners.data?.items ?? [];
  const online = items.filter((runner) => !["failed", "stopped", "deleted", "paused"].includes(runner.status)).length;
  const busy = items.filter((runner) => runner.busy).length;

  return (
    <div className="flex min-h-full flex-col lg:flex-row">
      <div className="min-w-0 flex-1">
        <ProvisioningCallout pool={pool} />

        {runners.isPending ? <InlineLoading label="Loading runners…" /> : items.length === 0 ? (
          <div className="border-b border-border px-4 py-10 text-center">
            <Server className="mx-auto size-5 text-faint" />
            <p className="mt-3 text-sm font-medium">No runners right now</p>
            <p className="mx-auto mt-1 max-w-sm text-xs leading-5 text-muted-foreground">
              {pool.autoscalingEnabled ? `The pool scales up from ${pool.minCount} when matching jobs are queued.` : `Autoscaling is off; the pool keeps ${pool.desiredCount} runners.`}
            </p>
          </div>
        ) : (
          groupRunners(items).map((group) => (
            <ListGroup count={group.runners.length} icon={<StatusBadge iconOnly status={group.status} />} key={group.key} label={group.label}>
              {group.runners.map((runner) => <RunnerRow key={runner.id} runner={runner} showPool={false} />)}
            </ListGroup>
          ))
        )}

        <section className="pt-6">
          <SectionHeading
            actions={<Link className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-hover hover:text-foreground" params={{ poolId: pool.id }} to="/runner-pools/$poolId/activity">All activity<ArrowRight className="size-3" /></Link>}
            className="mb-2 px-4"
          >
            Recent activity
          </SectionHeading>
          {events.status === "loading" ? <InlineLoading label="Loading activity…" /> : null}
          {events.status === "error" ? <p className="px-4 text-sm text-danger">{events.error}</p> : null}
          {events.status === "ready" ? (
            events.data.items.length ? <div className="border-t border-border"><PoolEventTable events={events.data.items.slice(0, 10)} stickyHeader={false} /></div> : (
              <p className="flex items-center gap-2 px-4 text-sm text-muted-foreground"><Activity className="size-4" />No lifecycle events recorded yet.</p>
            )
          ) : null}
        </section>
      </div>

      <PropertiesPanel>
        <PropertyGroup title="Status">
          <PropertyRow label="State"><StatusDot status={pool.paused ? "paused" : pool.state} />{statusLabel(pool.paused ? "paused" : pool.state)}</PropertyRow>
          <PropertyRow label="Runners"><CapacityMeter busy={busy} className="flex w-full" desired={pool.desiredCount} online={online} /></PropertyRow>
          <PropertyRow label="Range"><span className="tabular">{pool.minCount}–{pool.maxCount} runners</span></PropertyRow>
          <PropertyRow label="Autoscaling">{pool.autoscalingEnabled ? `On · idle after ${pool.idleTimeoutMinutes}m` : "Off"}</PropertyRow>
        </PropertyGroup>

        <PropertyGroup title="Runtime">
          <PropertyRow label="Providers">
            <span className="flex flex-wrap gap-1">{providers.map((provider) => <Badge dot={provider === "tart" ? "bg-[#bb87fc]" : "bg-info"} key={provider}>{providerLabel(provider, pool.macosRuntime)}</Badge>)}</span>
          </PropertyRow>
          <PropertyRow label="Mode"><span className="capitalize">{pool.mode}</span></PropertyRow>
          {providers.includes("docker") ? <PropertyRow label="Image" title={pool.dockerImage}><span className="truncate font-mono text-xs">{pool.dockerImage}</span></PropertyRow> : null}
          {providers.includes("tart") && pool.macosRuntime !== "native" ? <PropertyRow label="Base VM" title={pool.tartImage}><span className="truncate font-mono text-xs">{pool.tartImage}</span></PropertyRow> : null}
          <PropertyRow label="Per runner"><span className="tabular">{pool.cpuLimit} cores · {pool.memoryLimitMb.toLocaleString()} MB</span></PropertyRow>
          <PropertyRow label="Labels">
            {pool.labels.length ? <span className="flex flex-wrap gap-1">{pool.labels.map((label) => <Badge key={label} variant="outline">{label}</Badge>)}</span> : <span className="text-faint">None</span>}
          </PropertyRow>
        </PropertyGroup>

        <PropertyGroup title="Destination">
          <PropertyRow label="Scope"><span className="capitalize">{pool.scope}</span></PropertyRow>
          <PropertyRow label="Account"><Avatar name={pool.accountLogin} size={16} square src={githubAvatar(pool.accountLogin)} />{pool.accountLogin}</PropertyRow>
          {pool.scope === "repository" ? (
            <div className="space-y-1 pt-1">
              {pool.repositories.slice(0, 6).map((repository) => (
                <div className="truncate rounded-md bg-panel-subtle px-2 py-1 text-xs text-secondary-foreground" key={repository.id} title={repository.fullName}>{repository.fullName}</div>
              ))}
              {pool.repositories.length > 6 ? <div className="px-2 text-xs text-faint">+{pool.repositories.length - 6} more</div> : null}
            </div>
          ) : <PropertyRow label="Runner group"><span className="tabular">#{pool.runnerGroupId}</span></PropertyRow>}
        </PropertyGroup>

        <PropertyGroup title="Configuration">
          <PropertyRow label="Generation"><span className="tabular">{pool.configurationVersion}</span></PropertyRow>
          {pool.canManage ? (
            <Link className={buttonVariants({ className: "mt-2 w-full", size: "sm", variant: "outline" })} params={{ poolId: pool.id }} to="/runner-pools/$poolId/settings">Edit configuration</Link>
          ) : <PropertyRow label="Access">Read only</PropertyRow>}
        </PropertyGroup>
      </PropertiesPanel>
    </div>
  );
}
