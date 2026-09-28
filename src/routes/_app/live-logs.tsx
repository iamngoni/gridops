import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ChevronLeft, GitPullRequestArrow, Radio } from "lucide-react";
import { useEffect, useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { EmptyState, PageHeader } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { RunStatusIcon } from "~/components/status-icon";
import { buttonVariants } from "~/components/ui/button";
import { getLiveLogsPage } from "~/features/operations/operations.functions";
import { JobLogViewer } from "~/features/workflow-runs/job-log-viewer";
import { parsePage } from "~/lib/pagination";
import { useMediaQuery } from "~/lib/use-media-query";
import { cn, formatAge } from "~/lib/utils";

export const Route = createFileRoute("/_app/live-logs")({
  validateSearch: (search: Record<string, unknown>): { target?: string; page?: number } => {
    const target = typeof search.target === "string" ? search.target : undefined;
    const page = parsePage(search.page);
    return { ...(target ? { target } : {}), ...(page > 1 ? { page } : {}) };
  },
  loaderDeps: ({ search }) => ({ target: search.target, page: search.page ?? 1 }),
  loader: ({ deps }) => getLiveLogsPage({ page: deps.page, target: deps.target }),
  pendingComponent: () => <ResourcePageLoading icon={Radio} title="Live logs" />,
  component: LiveLogsRoutePage,
});

function LiveLogsRoutePage() {
  const search = Route.useSearch();
  return <LiveLogsPage key={`${search.page ?? 1}:${search.target ?? ""}`} />;
}

function LiveLogsPage() {
  const data = Route.useLoaderData();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const [targetPage, setTargetPage] = useState(data);
  const initialTarget = data.items.find((item) => item.id === search.target || item.runnerId === search.target);
  const [targetId, setTargetId] = useState(initialTarget?.id ?? data.items[0]?.id ?? "");
  const targets = targetPage.items;
  const selected = targets.find((item) => item.id === targetId) ?? targets[0];
  const selectedId = selected?.id;
  // Wide screens show the list beside the log. Narrow ones show one at a time:
  // the list until a job is chosen, then that job's log.
  const wide = useMediaQuery("(min-width: 48rem)");
  const showList = wide || !search.target;
  const showLog = wide || Boolean(search.target);

  useEffect(() => {
    if (!data.authenticated) return undefined;
    let cancelled = false;
    let refreshing = false;
    async function refreshTargets() {
      if (cancelled || refreshing || document.visibilityState === "hidden") return;
      refreshing = true;
      try {
        const page = await getLiveLogsPage({ page: search.page, target: search.target });
        if (!cancelled) {
          setTargetPage(page);
          setTargetId((current) => page.items.some((item) => item.id === current) ? current : (page.items[0]?.id ?? ""));
        }
      } catch {
        // Keep the last useful job list; the log viewer reports its own errors.
      } finally {
        refreshing = false;
      }
    }
    const interval = window.setInterval(() => void refreshTargets(), 5_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [data.authenticated, search.page, search.target]);

  function selectTarget(nextTargetId: string) {
    setTargetId(nextTargetId);
    // Side by side, switching jobs shouldn't fill the history; on a phone,
    // opening a log is a step the back gesture should undo.
    void navigate({ replace: wide, search: { page: search.page, target: nextTargetId } });
  }

  if (targets.length === 0) {
    return (
      <>
        <PageHeader icon={Radio} title="Live logs" />
        <EmptyState description="Logs appear once GitHub assigns a workflow job to a managed runner." icon={Radio} title="No job logs yet">
          <Link className={buttonVariants({ variant: "outline" })} to="/workflow-runs"><GitPullRequestArrow />Workflow runs</Link>
        </EmptyState>
      </>
    );
  }

  return (
    <>
      {showList ? (
        <PageHeader count={targetPage.total || undefined} icon={Radio} title="Live logs" />
      ) : (
        // The log's own header names the job, so the bar above is just the way back.
        <PageHeader
          documentTitle={selected?.jobName ?? "Live logs"}
          title={
            <Link className="-ml-1.5 inline-flex items-center gap-0.5 rounded-md py-0.5 pl-0.5 pr-1.5 text-muted-foreground transition-colors hover:bg-hover hover:text-foreground" search={search.page ? { page: search.page } : {}} to="/live-logs">
              <ChevronLeft className="size-4" />Live logs
            </Link>
          }
        />
      )}
      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <aside className={cn("min-h-0 flex-1 flex-col md:w-[320px] md:flex-none md:border-r md:border-border", showList ? "flex" : "hidden")}>
          <div className="min-h-0 flex-1 overflow-y-auto">
            {targets.map((target) => {
              const targetState = target.jobConclusion ?? target.jobStatus;
              const current = target.id === selectedId;
              return (
                <button
                  aria-pressed={current}
                  className={cn("flex w-full items-start gap-2.5 border-b border-border px-3 py-2 text-left outline-none transition-colors focus:shadow-[inset_2px_0_0_var(--primary)]", current ? "bg-selected" : "hover:bg-hover focus:bg-hover")}
                  data-list-row=""
                  key={target.id}
                  onClick={() => selectTarget(target.id)}
                  type="button"
                >
                  <RunStatusIcon className="mt-[3px]" status={targetState} />
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-2">
                      <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">{target.jobName}</span>
                      {target.kind === "live" ? <span className="shrink-0 rounded-full bg-info/15 px-1.5 text-2xs font-medium text-info">Live</span> : null}
                      <span className="tabular shrink-0 text-2xs text-faint">{formatAge(target.updatedAt)}</span>
                    </span>
                    <span className="mt-0.5 block truncate text-xs text-muted-foreground">
                      {target.workflowName} #{target.runNumber}<span className="text-faint"> · {target.repository?.split("/").pop()}</span>
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
          <div className="shrink-0 border-t border-border">
            <ListPagination compact itemCount={targets.length} noun="jobs" onPageChange={(page) => void navigate({ search: { page, target: undefined } })} page={targetPage.page} perPage={targetPage.perPage} total={targetPage.total} />
          </div>
        </aside>

        {selected && showLog ? (
          <JobLogViewer
            fallback={{ name: selected.jobName, status: selected.jobConclusion ?? selected.jobStatus, repository: selected.repository, workflowName: selected.workflowName, runNumber: selected.runNumber, runId: selected.runId }}
            jobId={selected.jobId}
            key={selected.jobId}
          />
        ) : null}
      </div>
    </>
  );
}
