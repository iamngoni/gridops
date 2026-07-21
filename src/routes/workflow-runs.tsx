import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ExternalLink, GitPullRequestArrow, OctagonX, RefreshCw, RotateCcw, Square } from "lucide-react";

import { AsyncActionButton } from "~/components/async-action-button";
import { ListPagination } from "~/components/list-pagination";
import { ResourcePage } from "~/components/resource-page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-badge";
import { Card, CardContent } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getWorkflowRunsPage, workflowRunAction } from "~/features/operations/operations.functions";
import { formatDuration, formatRelativeTime } from "~/lib/utils";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";
import { validatePageSearch } from "~/lib/pagination";

export const Route = createFileRoute("/workflow-runs")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getWorkflowRunsPage({ page: deps.page }),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Workflow runs"
      description="Follow GitHub Actions runs, jobs, conclusions, reruns, and cancellations."
      icon={GitPullRequestArrow}
    />
  ),
  component: WorkflowRunsPage,
});

function WorkflowRunsPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(5_000, data.authenticated);
  const control = workflowRunAction;

  return (
    <ResourcePage
      title="Workflow runs"
      description="Follow GitHub Actions runs, jobs, conclusions, reruns, and cancellations."
      icon={GitPullRequestArrow}
      emptyTitle="No workflow runs synced"
      emptyDescription="Run history will populate from connected repositories and verified GitHub webhook events."
    >
      {data.items.length > 0 ? (
        <Card><CardContent className="px-0 py-0">
          <Table>
            <TableHeader><TableRow>
              <TableHead>Workflow</TableHead><TableHead>Repository</TableHead><TableHead>Ref</TableHead>
              <TableHead>Jobs</TableHead><TableHead>Duration</TableHead><TableHead>Status</TableHead>
              <TableHead className="text-right">Controls</TableHead>
            </TableRow></TableHeader>
            <TableBody>{data.items.map((run) => {
              const active = run.status === "queued" || run.status === "in_progress";
              return (
                <TableRow key={run.id}>
                  <TableCell><Link className="font-medium hover:text-primary" params={{ runId: String(run.id) }} to="/workflow-runs/$runId">{run.workflowName}</Link><div className="mt-1 text-[11px] text-muted-foreground">#{run.runNumber} · attempt {run.runAttempt} · {run.event}</div></TableCell>
                  <TableCell><div className="text-xs">{run.repository}</div><div className="mt-1 text-[11px] text-muted-foreground">{run.actorLogin ? `by ${run.actorLogin}` : "GitHub Actions"}</div></TableCell>
                  <TableCell><div className="max-w-40 truncate font-mono text-xs">{run.headBranch ?? "detached"}</div><div className="mt-1 font-mono text-[11px] text-muted-foreground">{String(run.headSha).slice(0, 7)}</div></TableCell>
                  <TableCell><div className="text-xs">{run.jobCount} total · {run.activeJobs} active</div>{run.failedJobs ? <div className="mt-1 text-[11px] text-red-400">{run.failedJobs} failed</div> : null}</TableCell>
                  <TableCell><div className="text-xs">{formatDuration(run.startedAt ? String(run.startedAt) : null, run.completedAt ? String(run.completedAt) : null)}</div><div className="mt-1 text-[11px] text-muted-foreground">{formatRelativeTime(String(run.createdAt))}</div></TableCell>
                  <TableCell><StatusBadge status={run.conclusion ?? run.status} /></TableCell>
                  <TableCell><div className="flex justify-end gap-1">
                    {run.canManage && active ? <AsyncActionButton action={() => control({ data: { runId: run.id, action: "cancel" } })} confirm={`Cancel ${run.workflowName} #${run.runNumber}?`} icon={<Square />} size="icon" success="Cancellation requested."><span className="sr-only">Cancel run</span></AsyncActionButton> : null}
                    {run.canManage && active ? <AsyncActionButton action={() => control({ data: { runId: run.id, action: "force-cancel" } })} confirm={`Force-cancel ${run.workflowName} #${run.runNumber}? Use this when normal cancellation is blocked by a running condition.`} icon={<OctagonX />} size="icon" success="Force cancellation requested." variant="destructive"><span className="sr-only">Force-cancel run</span></AsyncActionButton> : null}
                    {run.canManage && !active ? <AsyncActionButton action={() => control({ data: { runId: run.id, action: "rerun" } })} icon={<RotateCcw />} size="icon" success="Workflow rerun requested."><span className="sr-only">Rerun workflow</span></AsyncActionButton> : null}
                    {run.canManage && run.conclusion === "failure" ? <AsyncActionButton action={() => control({ data: { runId: run.id, action: "rerun-failed" } })} icon={<RefreshCw />} size="icon" success="Failed jobs rerun requested."><span className="sr-only">Rerun failed jobs</span></AsyncActionButton> : null}
                    <a aria-label="Open run on GitHub" className="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground" href={String(run.htmlUrl)} rel="noreferrer" target="_blank"><ExternalLink className="size-4" /></a>
                  </div></TableCell>
                </TableRow>
              );
            })}</TableBody>
          </Table>
          <ListPagination itemCount={data.items.length} noun="workflow runs" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} />
        </CardContent></Card>
      ) : undefined}
    </ResourcePage>
  );
}
