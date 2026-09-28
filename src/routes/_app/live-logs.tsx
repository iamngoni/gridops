import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowDown, ChevronRight, CircleX, GitPullRequestArrow, LoaderCircle, Radio, RefreshCw, Search, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { EmptyState, PageHeader } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { RunStatusIcon } from "~/components/status-icon";
import { Button, buttonVariants } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";
import {
  type StructuredJobLog,
  getLiveLogsPage,
  getWorkflowJobLogAction,
} from "~/features/operations/operations.functions";
import { advanceFollowedSteps, isNearLogEnd } from "~/lib/log-follow";
import { parsePage } from "~/lib/pagination";
import { cn, formatAge, formatDuration } from "~/lib/utils";

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
  const [jobLog, setJobLog] = useState<StructuredJobLog | null>(null);
  const [expandedSteps, setExpandedSteps] = useState<Set<number>>(new Set());
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [following, setFollowing] = useState(true);
  const followingRef = useRef(true);
  const updateFollowing = useCallback((value: boolean) => {
    followingRef.current = value;
    setFollowing(value);
  }, []);
  const selectedJob = useRef<number | null>(null);
  const logViewport = useRef<HTMLDivElement>(null);
  const targets = targetPage.items;
  const selected = targets.find((item) => item.id === targetId) ?? targets[0];
  const selectedId = selected?.id;
  const selectedJobId = selected?.jobId;
  const active = [selected?.jobStatus, jobLog?.status].some((status) => status === "queued" || status === "in_progress");

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
          setTargetId((current) => page.items.some((item) => item.id === current)
            ? current
            : (page.items[0]?.id ?? ""));
        }
      } catch {
        // Keep the last useful job list while the selected job has its own error state.
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

  const refreshLog = useCallback(async (jobId: number, showLoading = false) => {
    if (showLoading) setLoading(true);
    try {
      const response = await getWorkflowJobLogAction({ data: { jobId } });
      setJobLog(response);
      setError(null);
      if (selectedJob.current !== response.id) {
        selectedJob.current = response.id;
        const failed = response.steps
          .filter((step) => step.conclusion === "failure" || step.status === "in_progress")
          .map((step) => step.number);
        const recommended = advanceFollowedSteps(response.steps, new Set(failed));
        const firstStep = response.steps[0]?.number;
        setExpandedSteps(recommended.size ? recommended : new Set(firstStep === undefined ? [] : [firstStep]));
      } else if (followingRef.current && (response.status === "queued" || response.status === "in_progress")) {
        setExpandedSteps((current) => {
          const next = advanceFollowedSteps(response.steps, current);
          if (next.size === current.size && [...next].every((number) => current.has(number))) return current;
          return next;
        });
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not load workflow job logs.");
    } finally {
      if (showLoading) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!selectedJobId) return undefined;
    const jobId = selectedJobId;
    const initial = window.setTimeout(() => {
      selectedJob.current = null;
      setJobLog(null);
      setError(null);
      setQuery("");
      updateFollowing(true);
      void refreshLog(jobId, true);
    }, 0);
    return () => window.clearTimeout(initial);
  }, [refreshLog, selectedJobId, updateFollowing]);

  useEffect(() => {
    if (!selectedJobId || !active) return undefined;
    const jobId = selectedJobId;
    const interval = active
      ? window.setInterval(() => {
        if (document.visibilityState === "visible") void refreshLog(jobId, false);
      }, 4_000)
      : undefined;
    return () => {
      if (interval) window.clearInterval(interval);
    };
  }, [active, refreshLog, selectedJobId]);

  useLayoutEffect(() => {
    const viewport = logViewport.current;
    if (!viewport || !following || !active) return;
    viewport.scrollTop = viewport.scrollHeight;
  }, [active, expandedSteps, following, jobLog?.lineCount]);

  const visibleSteps = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!jobLog || !normalized) return jobLog?.steps ?? [];
    return jobLog.steps
      .map((step) => ({
        ...step,
        lines: step.lines.filter((line) => line.text.toLowerCase().includes(normalized)),
      }))
      .filter((step) => step.name.toLowerCase().includes(normalized) || step.lines.length > 0);
  }, [jobLog, query]);

  function selectTarget(nextTargetId: string) {
    setTargetId(nextTargetId);
    void navigate({ replace: true, search: { page: search.page, target: nextTargetId } });
  }

  function toggleStep(number: number) {
    if (active) updateFollowing(false);
    setExpandedSteps((current) => {
      const next = new Set(current);
      if (next.has(number)) next.delete(number);
      else next.add(number);
      return next;
    });
  }

  function revealStep(number: number) {
    if (active) updateFollowing(false);
    setExpandedSteps((current) => new Set(current).add(number));
  }

  function handleLogScroll() {
    const viewport = logViewport.current;
    if (!viewport || !active) return;
    updateFollowing(isNearLogEnd(viewport));
  }

  function jumpToLatest() {
    const viewport = logViewport.current;
    if (jobLog) setExpandedSteps((current) => advanceFollowedSteps(jobLog.steps, current));
    updateFollowing(true);
    viewport?.scrollTo({ behavior: "smooth", top: viewport.scrollHeight });
  }

  const status = jobLog?.conclusion ?? jobLog?.status ?? selected?.jobConclusion ?? selected?.jobStatus ?? "queued";

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
      <PageHeader count={targetPage.total || undefined} icon={Radio} title="Live logs" />
      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <aside className="flex max-h-72 shrink-0 flex-col border-b border-border md:max-h-none md:w-[320px] md:border-b-0 md:border-r">
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

        <section className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-2">
            <RunStatusIcon status={status} />
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-medium">{jobLog?.name ?? selected?.jobName ?? "Job log"}</div>
              <div className="truncate text-xs text-muted-foreground">
                {jobLog?.repository ?? selected?.repository} · {jobLog?.workflowName ?? selected?.workflowName} #{jobLog?.runNumber ?? selected?.runNumber}
                {jobLog ? <> · {formatDuration(jobLog.startedAt, jobLog.completedAt)} · {jobLog.source === "github" ? "GitHub log" : jobLog.source === "runner" ? "live runner" : "waiting for output"}</> : null}
              </div>
            </div>
            {active ? (
              <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <span className={cn("size-1.5 rounded-full", following ? "bg-success live-pulse" : "bg-warning")} />{following ? "Following" : "Paused"}
              </span>
            ) : null}
            <label className="relative hidden h-7 w-56 items-center sm:flex">
              <Search className="pointer-events-none absolute left-2 size-3.5 text-muted-foreground" />
              <input
                aria-label="Search job logs"
                className="h-7 w-full rounded-md border border-border-strong bg-panel pl-7 pr-2 text-xs outline-none placeholder:text-faint focus:border-primary/70"
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search output…"
                value={query}
              />
            </label>
            {selected?.runId ? <Link className={buttonVariants({ size: "sm", variant: "ghost" })} params={{ runId: String(selected.runId) }} to="/workflow-runs/$runId">View run</Link> : null}
            <Tooltip content="Refresh log">
              <Button aria-label="Refresh job log" disabled={loading} onClick={() => selected?.jobId && void refreshLog(selected.jobId, true)} size="icon-sm" variant="ghost">
                {loading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
              </Button>
            </Tooltip>
          </div>

          {error || jobLog?.metadataWarning || jobLog?.truncated ? (
            <div className="shrink-0 space-y-1 border-b border-border px-4 py-2 text-xs">
              {error ? <p className="text-danger">{error}</p> : null}
              {jobLog?.metadataWarning ? <p className="text-warning">{jobLog.metadataWarning}</p> : null}
              {jobLog?.truncated ? <p className="text-warning">This very large job log is showing its final 25 MB.</p> : null}
            </div>
          ) : null}

          {jobLog?.annotations.length ? (
            <div className="shrink-0 border-b border-border">
              <div className="flex h-8 items-center gap-2 px-4 text-xs font-medium text-muted-foreground">Annotations<span className="font-normal text-faint">{annotationSummary(jobLog)}</span></div>
              <div className="max-h-40 overflow-y-auto pb-1">
                {jobLog.annotations.map((annotation, index) => (
                  <button className="flex w-full items-start gap-2.5 px-4 py-1.5 text-left hover:bg-hover" key={`${annotation.stepNumber}:${annotation.message}:${index}`} onClick={() => revealStep(annotation.stepNumber)} type="button">
                    {annotation.level === "error" ? <CircleX className="mt-0.5 size-3.5 shrink-0 text-danger" /> : <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-warning" />}
                    <span className="min-w-0 text-xs"><span className="font-medium text-foreground">{annotation.stepName}</span><span className="ml-2 break-words font-mono text-muted-foreground">{annotation.message}</span></span>
                  </button>
                ))}
              </div>
            </div>
          ) : null}

          <div className="relative min-h-0 flex-1">
            <div className="absolute inset-0 overflow-auto bg-[var(--log-background)]" onScroll={handleLogScroll} ref={logViewport}>
              {loading && !jobLog ? <div className="flex items-center gap-2 p-6 text-sm text-muted-foreground"><LoaderCircle className="size-4 animate-spin" />Loading job output…</div> : null}
              {!loading && jobLog && visibleSteps.length === 0 ? <div className="p-6 text-sm text-muted-foreground">No step output matches “{query}”.</div> : null}
              {visibleSteps.map((step) => {
                const expanded = expandedSteps.has(step.number) || Boolean(query);
                return (
                  <div className="border-b border-white/[0.06]" key={step.number}>
                    <button className={cn("sticky top-0 z-[1] flex w-full items-center gap-2.5 bg-[var(--log-background)] px-4 py-2 text-left hover:bg-white/[0.03]", step.conclusion === "failure" && "text-[#ff8a8a]")} onClick={() => toggleStep(step.number)} type="button">
                      <ChevronRight className={cn("size-3.5 shrink-0 text-[#62666d] transition-transform", expanded && "rotate-90")} />
                      <RunStatusIcon status={step.conclusion ?? step.status} />
                      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-[#d0d6e0]">{step.name}</span>
                      <span className="tabular shrink-0 text-xs text-[#62666d]">{formatDuration(step.startedAt, step.completedAt)}</span>
                    </button>
                    {expanded ? (
                      step.lines.length ? (
                        <pre className="overflow-x-auto pb-2 font-mono text-xs leading-5 text-[#c9ced6]"><code>{step.lines.map((line, index) => <LogLine index={index + 1} key={`${line.timestamp}:${index}`} level={line.level} text={line.text} />)}</code></pre>
                      ) : <div className="px-11 pb-3 text-xs text-[#62666d]">{active && step.status === "in_progress" ? "Waiting for output from this step…" : "No console output for this step."}</div>
                    ) : null}
                  </div>
                );
              })}
            </div>
            {active && !following ? <Button className="absolute bottom-4 right-4 shadow-popover" onClick={jumpToLatest} size="sm" variant="secondary"><ArrowDown />Jump to latest</Button> : null}
          </div>
        </section>
      </div>
    </>
  );
}

function LogLine({ index, level, text }: { index: number; level: string; text: string }) {
  return (
    <span className={cn(
      "grid min-w-max grid-cols-[3.25rem_minmax(0,1fr)] pr-4",
      level === "error" && "bg-[#eb5757]/15 text-[#ffb4b4]",
      level === "warning" && "bg-[#f2994a]/12 text-[#ffd8b0]",
      level === "command" && "text-[#8fc1ff]",
      level === "group" && "mt-1 font-semibold text-[#f7f8f8]",
      level === "notice" && "text-[#8fe0b5]",
    )}>
      <span className="select-none pr-4 text-right text-[#4a4d54]">{index}</span>
      <span className="whitespace-pre-wrap break-words">{level === "group" ? `▸ ${text}` : text || " "}</span>
    </span>
  );
}

function annotationSummary(job: StructuredJobLog) {
  const errors = job.annotations.filter((annotation) => annotation.level === "error").length;
  const warnings = job.annotations.filter((annotation) => annotation.level === "warning").length;
  return `${errors} ${errors === 1 ? "error" : "errors"} · ${warnings} ${warnings === 1 ? "warning" : "warnings"}`;
}
