import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { GitPullRequestArrow } from "lucide-react";

import { ListPagination } from "~/components/list-pagination";
import { EmptyState, ListGroup, PageBody, PageHeader } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { RunStatusIcon } from "~/components/status-icon";
import { type WorkflowRun, getWorkflowRunsPage } from "~/features/operations/operations.functions";
import { RunActionsMenu, isActiveRun } from "~/features/workflow-runs/run-actions";
import { RunRow } from "~/features/workflow-runs/run-row";
import { validatePageSearch } from "~/lib/pagination";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/_app/workflow-runs/")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getWorkflowRunsPage({ page: deps.page }),
  pendingComponent: () => <ResourcePageLoading icon={GitPullRequestArrow} title="Workflow runs" />,
  component: WorkflowRunsPage,
});

const GROUPS = [
  { key: "in_progress", label: "In progress", icon: "in_progress" },
  { key: "queued", label: "Queued", icon: "queued" },
  { key: "failure", label: "Failed", icon: "failure" },
  { key: "done", label: "Completed", icon: "success" },
] as const;

function runGroup(run: WorkflowRun): (typeof GROUPS)[number]["key"] {
  if (run.status === "in_progress") return "in_progress";
  if (isActiveRun(run)) return "queued";
  if (run.conclusion === "failure" || run.conclusion === "timed_out" || run.conclusion === "startup_failure") return "failure";
  return "done";
}

function WorkflowRunsPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(5_000, data.authenticated);
  const groups = GROUPS.map((group) => ({ ...group, runs: data.items.filter((run) => runGroup(run) === group.key) })).filter((group) => group.runs.length);

  return (
    <>
      <PageHeader count={data.total || undefined} icon={GitPullRequestArrow} title="Workflow runs" />
      <PageBody>
        {data.items.length === 0 ? (
          <EmptyState description="Run history fills in from connected repositories through GitHub webhooks and polling." icon={GitPullRequestArrow} title="No workflow runs yet" />
        ) : (
          <>
            {groups.map((group) => (
              <ListGroup count={group.runs.length} icon={<RunStatusIcon status={group.icon} />} key={group.key} label={group.label}>
                {group.runs.map((run) => (
                  <RunRow
                    key={run.id}
                    meta={<JobSummary run={run} />}
                    run={{ id: run.id, workflow: run.workflowName, repository: run.repository, runNumber: run.runNumber, branch: run.headBranch, status: run.status, conclusion: run.conclusion, actor: run.actorLogin, startedAt: run.startedAt, completedAt: run.completedAt, createdAt: run.createdAt }}
                    trailing={<RunActionsMenu run={run} />}
                  />
                ))}
              </ListGroup>
            ))}
            <ListPagination itemCount={data.items.length} noun="workflow runs" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} />
          </>
        )}
      </PageBody>
    </>
  );
}

function JobSummary({ run }: { run: WorkflowRun }) {
  if (!run.jobCount) return null;
  return (
    <span className="tabular hidden shrink-0 items-center gap-2 text-xs text-muted-foreground lg:inline-flex">
      {run.failedJobs ? <span className="text-danger">{run.failedJobs} failed</span> : null}
      {run.activeJobs ? <span className="text-progress">{run.activeJobs} active</span> : null}
      <span className="text-faint">{run.jobCount} {run.jobCount === 1 ? "job" : "jobs"}</span>
    </span>
  );
}
