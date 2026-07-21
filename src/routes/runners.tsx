import { createFileRoute } from "@tanstack/react-router";
import { Activity, Pause, Play, RefreshCw, RotateCcw, Square, Trash2 } from "lucide-react";

import { AsyncActionButton } from "~/components/async-action-button";
import { ResourcePage } from "~/components/resource-page";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { Card, CardContent } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getRunnersPage } from "~/features/operations/operations.functions";
import { runnerAction } from "~/features/runner-pools/runner-pools.functions";
import { formatRelativeTime } from "~/lib/utils";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/runners")({
  loader: () => getRunnersPage(),
  component: RunnersPage,
});

function RunnersPage() {
  const data = Route.useLoaderData();
  useLiveRouteRefresh(5_000, data.authenticated);
  const control = runnerAction;

  return (
    <ResourcePage
      title="Runners"
      description="Inspect every managed container and its GitHub registration state."
      icon={Activity}
      emptyTitle="No managed runners"
      emptyDescription="Runners appear here when a pool provisions its first container."
      action="Manage runner pools"
      actionHref="/runner-pools"
    >
      {data.items.length > 0 ? (
        <Card><CardContent className="px-0 py-0">
          <Table>
            <TableHeader><TableRow>
              <TableHead>Runner</TableHead><TableHead>Pool</TableHead><TableHead>GitHub</TableHead>
              <TableHead>Runtime</TableHead><TableHead>Heartbeat</TableHead><TableHead>Status</TableHead>
              <TableHead className="text-right">Controls</TableHead>
            </TableRow></TableHeader>
            <TableBody>{data.items.map((runner) => (
              <TableRow key={runner.id}>
                <TableCell>
                  <div className="font-mono text-xs font-medium">{runner.name}</div>
                  <div className="mt-1 text-[11px] text-muted-foreground">{runner.os}/{runner.architecture} · {runner.ephemeral ? "ephemeral" : "persistent"}</div>
                </TableCell>
                <TableCell>
                  <div className="text-xs">{runner.poolName}</div>
                  <div className="mt-1 text-[11px] text-muted-foreground">{runner.repository ?? runner.accountLogin}</div>
                </TableCell>
                <TableCell className="font-mono text-xs">{runner.githubRunnerId ?? "pending"}</TableCell>
                <TableCell>
                  <div className="max-w-36 truncate font-mono text-[11px]" title={String(runner.containerId ?? "")}>{runner.containerId ? String(runner.containerId).slice(0, 12) : "—"}</div>
                  {runner.currentJobName ? <Badge className="mt-1" variant="info">{String(runner.currentJobName)}</Badge> : null}
                </TableCell>
                <TableCell className="text-xs text-muted-foreground">
                  {runner.lastHeartbeatAt ? formatRelativeTime(String(runner.lastHeartbeatAt)) : "Never"}
                </TableCell>
                <TableCell>
                  <StatusBadge status={runner.busy ? "busy" : String(runner.status)} />
                  {runner.failureReason ? <div className="mt-1 max-w-52 truncate text-[11px] text-red-400" title={String(runner.failureReason)}>{String(runner.failureReason)}</div> : null}
                </TableCell>
                <TableCell><div className="flex justify-end gap-1">
                  {runner.status === "paused" ? (
                    <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "resume" } })} icon={<Play />} size="icon" success="Runner resumed."><span className="sr-only">Resume {runner.name}</span></AsyncActionButton>
                  ) : runner.status === "stopped" && !runner.ephemeral ? (
                    <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "start" } })} icon={<Play />} size="icon" success="Runner started."><span className="sr-only">Start {runner.name}</span></AsyncActionButton>
                  ) : runner.status !== "stopped" ? (
                    <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "pause" } })} disabled={runner.busy || !runner.containerId || runner.status === "stopped"} icon={<Pause />} size="icon" success="Runner paused."><span className="sr-only">Pause {runner.name}</span></AsyncActionButton>
                  ) : null}
                  <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "stop" } })} confirm={`Stop ${runner.name}?`} disabled={!runner.containerId || runner.status === "stopped"} icon={<Square />} size="icon" success="Runner stopped."><span className="sr-only">Stop {runner.name}</span></AsyncActionButton>
                  <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "restart" } })} confirm={runner.busy ? `${runner.name} is busy. Restart it and interrupt the current job?` : undefined} disabled={runner.ephemeral || !runner.containerId || runner.status === "stopped"} icon={<RotateCcw />} size="icon" success="Runner restarted."><span className="sr-only">Restart {runner.name}</span></AsyncActionButton>
                  <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "rebuild" } })} confirm={`Rebuild ${runner.name}? GridOps will replace it with a newly registered container.`} disabled={runner.busy} icon={<RefreshCw />} size="icon" success="Runner rebuilt."><span className="sr-only">Rebuild {runner.name}</span></AsyncActionButton>
                  <AsyncActionButton action={() => control({ data: { runnerId: runner.id, action: "delete" } })} confirm={`Delete ${runner.name} from Docker and GitHub?`} icon={<Trash2 />} size="icon" success="Runner deleted." variant="destructive"><span className="sr-only">Delete {runner.name}</span></AsyncActionButton>
                </div></TableCell>
              </TableRow>
            ))}</TableBody>
          </Table>
        </CardContent></Card>
      ) : undefined}
    </ResourcePage>
  );
}
