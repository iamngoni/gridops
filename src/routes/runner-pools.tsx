import { Link, createFileRoute } from "@tanstack/react-router";
import { Boxes, Minus, Pause, Play, Plus, RefreshCw, Settings2, Trash2 } from "lucide-react";

import { AsyncActionButton } from "~/components/async-action-button";
import { ResourcePage } from "~/components/resource-page";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { Card, CardContent } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getRunnerPoolsPage } from "~/features/operations/operations.functions";
import { runnerPoolAction } from "~/features/runner-pools/runner-pools.functions";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/runner-pools")({
  loader: () => getRunnerPoolsPage(),
  component: RunnerPoolsPage,
});

function RunnerPoolsPage() {
  const data = Route.useLoaderData();
  useLiveRouteRefresh(5_000, data.authenticated);
  const control = runnerPoolAction;

  return (
    <ResourcePage
      title="Runner pools"
      description="Define capacity, labels, images, limits, and lifecycle policy for groups of runners."
      icon={Boxes}
      emptyTitle={data.authenticated ? "No runner pools" : "Connect GitHub to manage runners"}
      emptyDescription={data.authenticated
        ? "Create a repository or organization-scoped pool to start provisioning containers."
        : "Authorize the GitHub App, then choose a repository or organization for your first pool."}
      action="Create runner pool"
      actionHref="/runner-pools/new"
    >
      {data.items.length > 0 ? (
        <Card>
          <CardContent className="px-0 py-0">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Pool</TableHead>
                  <TableHead>Destination</TableHead>
                  <TableHead>Capacity</TableHead>
                  <TableHead>Runners</TableHead>
                  <TableHead>Resources</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead className="text-right">Controls</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.items.map((pool) => (
                  <TableRow key={pool.id}>
                    <TableCell>
                      <Link className="font-medium hover:text-primary" params={{ poolId: pool.id }} to="/runner-pools/$poolId">{pool.name}</Link>
                      <div className="mt-1 flex flex-wrap gap-1">
                        {pool.labels.slice(0, 3).map((label) => <Badge key={label} variant="outline">{label}</Badge>)}
                      </div>
                    </TableCell>
                    <TableCell>
                      <div className="text-xs">{pool.repository ?? pool.accountLogin}</div>
                      <div className="mt-1 text-[11px] capitalize text-muted-foreground">{pool.scope} · {pool.mode}</div>
                    </TableCell>
                    <TableCell>
                      <div className="font-mono text-xs">{pool.desiredCount}</div>
                      <div className="mt-1 text-[11px] text-muted-foreground">{pool.minCount} min · {pool.maxCount} max</div>
                    </TableCell>
                    <TableCell>
                      <div className="text-xs">{pool.onlineRunners} online · {pool.busyRunners} busy</div>
                      <div className="mt-1 text-[11px] text-muted-foreground">{pool.totalRunners} managed · {pool.failedRunners} failed</div>
                      {pool.outdatedRunners > 0 ? <div className="mt-1 text-[11px] text-amber-300">{pool.outdatedRunners} awaiting update</div> : null}
                    </TableCell>
                    <TableCell className="text-xs">
                      {pool.cpuLimit} CPU · {pool.memoryLimitMb} MB
                      <div className="mt-1 max-w-40 truncate text-[11px] text-muted-foreground" title={pool.image}>{pool.image}</div>
                    </TableCell>
                    <TableCell><StatusBadge status={pool.paused ? "paused" : pool.state} /></TableCell>
                    <TableCell>
                      {pool.canManage ? <div className="flex justify-end gap-1">
                        <Link aria-label={`Edit ${pool.name}`} className="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground" params={{ poolId: pool.id }} to="/runner-pools/$poolId"><Settings2 className="size-4" /></Link>
                        <AsyncActionButton
                          action={() => control({ data: { action: "scale", poolId: pool.id, desiredCount: pool.desiredCount - 1 } })}
                          disabled={pool.desiredCount <= pool.minCount || pool.paused}
                          icon={<Minus />}
                          size="icon"
                          success="Pool capacity decreased."
                        ><span className="sr-only">Scale down {pool.name}</span></AsyncActionButton>
                        <AsyncActionButton
                          action={() => control({ data: { action: "scale", poolId: pool.id, desiredCount: pool.desiredCount + 1 } })}
                          disabled={pool.desiredCount >= pool.maxCount || pool.paused}
                          icon={<Plus />}
                          size="icon"
                          success="Pool capacity increased."
                        ><span className="sr-only">Scale up {pool.name}</span></AsyncActionButton>
                        <AsyncActionButton
                          action={() => control({ data: { action: pool.paused ? "resume" : "pause", poolId: pool.id } })}
                          confirm={pool.paused ? undefined : `Pause ${pool.name}? Idle runners will be drained immediately.`}
                          icon={pool.paused ? <Play /> : <Pause />}
                          size="icon"
                          success={pool.paused ? "Pool resumed." : "Pool is draining."}
                        ><span className="sr-only">{pool.paused ? "Resume" : "Pause"} {pool.name}</span></AsyncActionButton>
                        <AsyncActionButton
                          action={() => control({ data: { action: "reconcile", poolId: pool.id } })}
                          icon={<RefreshCw />}
                          size="icon"
                          success="Pool reconciled with Docker."
                        ><span className="sr-only">Reconcile {pool.name}</span></AsyncActionButton>
                        <AsyncActionButton
                          action={() => control({ data: { action: "delete", poolId: pool.id } })}
                          confirm={`Delete ${pool.name} and every runner it manages? This cannot be undone.`}
                          icon={<Trash2 />}
                          size="icon"
                          success="Runner pool deleted."
                          variant="destructive"
                        ><span className="sr-only">Delete {pool.name}</span></AsyncActionButton>
                      </div> : <div className="flex justify-end"><Badge variant="outline">read only</Badge></div>}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      ) : undefined}
    </ResourcePage>
  );
}
