import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  Activity,
  ArrowDown,
  ChevronDown,
  ChevronRight,
  Circle,
  CircleCheck,
  CircleX,
  Clock3,
  LoaderCircle,
  Radio,
  RefreshCw,
  Search,
  Terminal,
  TriangleAlert,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { ResourcePage } from "~/components/resource-page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import {
  type StructuredJobLog,
  getLiveLogsPage,
  getWorkflowJobLogAction,
} from "~/features/operations/operations.functions";
import { isNearLogEnd } from "~/lib/log-follow";
import { parsePage } from "~/lib/pagination";
import { cn, formatDuration } from "~/lib/utils";

export const Route = createFileRoute("/live-logs")({
  validateSearch: (search: Record<string, unknown>): { target?: string; page?: number } => {
    const target = typeof search.target === "string" ? search.target : undefined;
    const page = parsePage(search.page);
    return { ...(target ? { target } : {}), ...(page > 1 ? { page } : {}) };
  },
  loaderDeps: ({ search }) => ({ target: search.target, page: search.page ?? 1 }),
  loader: ({ deps }) => getLiveLogsPage({ page: deps.page, target: deps.target }),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Live logs"
      description="Loading workflow jobs, steps, and annotations."
      icon={Radio}
    />
  ),
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
        const recommended = response.steps
          .filter((step) => step.conclusion === "failure" || step.status === "in_progress")
          .map((step) => step.number);
        const firstStep = response.steps[0]?.number;
        setExpandedSteps(new Set(recommended.length ? recommended : firstStep === undefined ? [] : [firstStep]));
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
      setFollowing(true);
      void refreshLog(jobId, true);
    }, 0);
    return () => window.clearTimeout(initial);
  }, [refreshLog, selectedJobId]);

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
  }, [active, following, jobLog?.lineCount]);

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
    setExpandedSteps((current) => {
      const next = new Set(current);
      if (next.has(number)) next.delete(number);
      else next.add(number);
      return next;
    });
  }

  function revealStep(number: number) {
    setExpandedSteps((current) => new Set(current).add(number));
  }

  function handleLogScroll() {
    const viewport = logViewport.current;
    if (!viewport || !active) return;
    setFollowing(isNearLogEnd(viewport));
  }

  function jumpToLatest() {
    const viewport = logViewport.current;
    setFollowing(true);
    viewport?.scrollTo({ behavior: "smooth", top: viewport.scrollHeight });
  }

  return (
    <ResourcePage
      title="Live logs"
      description="Inspect clean workflow output by job and step, with failures and annotations surfaced first."
      icon={Radio}
      emptyTitle="No workflow job logs"
      emptyDescription="Logs appear when GitHub assigns a workflow job to a managed runner."
      action="Workflow runs"
      actionHref="/workflow-runs"
      actionIcon={Activity}
    >
      {targets.length > 0 ? (
        <div className="grid items-start gap-5 xl:grid-cols-[340px_minmax(0,1fr)]">
          <Card>
            <CardHeader>
              <div>
                <CardTitle>Workflow jobs</CardTitle>
                <p className="mt-1 text-xs text-muted-foreground">Choose a current or retained job log.</p>
              </div>
            </CardHeader>
            <CardContent className="p-0">
              <div className="max-h-[30rem] space-y-2 overflow-y-auto p-3 xl:max-h-[44rem]">
                {targets.map((target) => (
                  <button
                    aria-pressed={target.id === selectedId}
                    className={cn(
                      "w-full rounded-lg p-3 text-left transition-colors",
                      target.id === selectedId
                        ? "bg-primary/[0.09] shadow-[inset_3px_0_0_hsl(153_64%_52%)]"
                        : "hover:bg-muted/50",
                    )}
                    key={target.id}
                    onClick={() => selectTarget(target.id)}
                    type="button"
                  >
                    <div className="flex items-start justify-between gap-2">
                      <span className="line-clamp-2 text-sm font-medium leading-5">{target.jobName}</span>
                      <StatusBadge status={target.jobConclusion ?? target.jobStatus} />
                    </div>
                    <div className="mt-1.5 truncate text-[11px] text-muted-foreground">
                      {target.workflowName} #{target.runNumber}
                    </div>
                    <div className="mt-1 flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
                      <span className="truncate">{target.repository}</span>
                      <span className="shrink-0 uppercase tracking-wide">{target.kind}</span>
                    </div>
                  </button>
                ))}
              </div>
              <ListPagination
                itemCount={targets.length}
                noun="job logs"
                onPageChange={(page) => void navigate({ search: { page, target: undefined } })}
                page={targetPage.page}
                perPage={targetPage.perPage}
                total={targetPage.total}
              />
            </CardContent>
          </Card>

          <div className="min-w-0 space-y-4">
            <Card>
              <CardHeader>
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <Terminal className="size-4 text-primary" />
                    <CardTitle className="truncate">{jobLog?.name ?? selected?.jobName ?? "Job log"}</CardTitle>
                    <StatusBadge status={jobLog?.conclusion ?? jobLog?.status ?? selected?.jobConclusion ?? selected?.jobStatus ?? "queued"} />
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {jobLog?.repository ?? selected?.repository} · {jobLog?.workflowName ?? selected?.workflowName} #{jobLog?.runNumber ?? selected?.runNumber}
                  </p>
                </div>
                <div className="flex flex-wrap items-center justify-end gap-2">
                  {active ? <span className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground"><span className={cn("size-1.5 rounded-full", following ? "bg-emerald-400" : "bg-amber-400")} />{following ? "Following" : "Follow paused"}</span> : null}
                  {selected?.runId ? <Link className="text-xs font-medium text-muted-foreground hover:text-foreground" params={{ runId: String(selected.runId) }} to="/workflow-runs/$runId">View run</Link> : null}
                  <Button disabled={loading} onClick={() => selected?.jobId && void refreshLog(selected.jobId, true)} size="icon" title="Refresh job log" variant="outline">
                    {loading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
                    <span className="sr-only">Refresh job log</span>
                  </Button>
                </div>
              </CardHeader>
              {jobLog ? <CardContent className="border-t border-border/60 py-3">
                <div className="grid gap-3 text-xs sm:grid-cols-3">
                  <Meta label="Duration" value={formatDuration(jobLog.startedAt, jobLog.completedAt)} />
                  <Meta label="Steps" value={`${jobLog.steps.length}`} />
                  <Meta label="Log source" value={jobLog.source === "github" ? "GitHub job log" : jobLog.source === "runner" ? "Live runner" : "Waiting for output"} />
                </div>
              </CardContent> : null}
            </Card>

            {error ? <div className="rounded-lg border border-red-500/25 bg-red-500/8 px-4 py-3 text-sm text-red-200">{error}</div> : null}
            {jobLog?.metadataWarning ? <div className="rounded-lg border border-amber-500/25 bg-amber-500/8 px-4 py-3 text-sm text-amber-100">{jobLog.metadataWarning}</div> : null}
            {jobLog?.truncated ? <div className="rounded-lg border border-amber-500/25 bg-amber-500/8 px-4 py-3 text-sm text-amber-100">This exceptionally large job log is showing its final 25 MB.</div> : null}

            {jobLog?.annotations.length ? <Card>
              <CardHeader>
                <div>
                  <CardTitle>Annotations</CardTitle>
                  <p className="mt-1 text-xs text-muted-foreground">{annotationSummary(jobLog)}</p>
                </div>
                <TriangleAlert className="size-5 text-red-400" />
              </CardHeader>
              <CardContent className="space-y-2 pt-0">
                {jobLog.annotations.map((annotation, index) => (
                  <button
                    className={cn(
                      "flex w-full items-start gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors",
                      annotation.level === "error"
                        ? "border-red-500/20 bg-red-500/6 hover:bg-red-500/10"
                        : "border-amber-500/20 bg-amber-500/6 hover:bg-amber-500/10",
                    )}
                    key={`${annotation.stepNumber}:${annotation.message}:${index}`}
                    onClick={() => revealStep(annotation.stepNumber)}
                    type="button"
                  >
                    {annotation.level === "error" ? <CircleX className="mt-0.5 size-4 shrink-0 text-red-400" /> : <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-400" />}
                    <span className="min-w-0"><span className="block text-xs font-medium">{annotation.stepName}</span><span className="mt-1 block break-words font-mono text-[11px] leading-5 text-muted-foreground">{annotation.message}</span></span>
                  </button>
                ))}
              </CardContent>
            </Card> : null}

            <Card className="min-w-0 overflow-hidden">
              <CardHeader>
                <div>
                  <CardTitle>Steps</CardTitle>
                  <p className="mt-1 text-xs text-muted-foreground">Failed and running steps open automatically.</p>
                </div>
                <div className="relative w-full sm:w-64">
                  <Search className="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
                  <Input aria-label="Search job logs" className="h-9 pl-9" onChange={(event) => setQuery(event.target.value)} placeholder="Search logs…" value={query} />
                </div>
              </CardHeader>
              <CardContent className="relative p-0">
                <div className="max-h-[68vh] min-h-[360px] overflow-auto border-t border-border/60" onScroll={handleLogScroll} ref={logViewport}>
                  {loading && !jobLog ? <div className="grid min-h-80 place-items-center text-sm text-muted-foreground"><LoaderCircle className="mr-2 inline size-4 animate-spin" />Loading structured job output…</div> : null}
                  {!loading && jobLog && visibleSteps.length === 0 ? <div className="grid min-h-64 place-items-center text-sm text-muted-foreground">No step output matches “{query}”.</div> : null}
                  {visibleSteps.map((step) => {
                    const expanded = expandedSteps.has(step.number) || Boolean(query);
                    return <div className="border-b border-border/50 last:border-b-0" key={step.number}>
                      <button className={cn("flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-muted/30", step.conclusion === "failure" && "bg-red-500/[0.035]")} onClick={() => toggleStep(step.number)} type="button">
                        {expanded ? <ChevronDown className="size-4 shrink-0 text-muted-foreground" /> : <ChevronRight className="size-4 shrink-0 text-muted-foreground" />}
                        <StepIcon conclusion={step.conclusion} status={step.status} />
                        <span className="min-w-0 flex-1 truncate text-sm font-medium">{step.name}</span>
                        <span className="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground"><Clock3 className="size-3" />{formatDuration(step.startedAt, step.completedAt)}</span>
                      </button>
                      {expanded ? <div className="border-t border-border/40 bg-[hsl(222_35%_5%)]">
                        {step.lines.length ? <pre className="overflow-x-auto py-3 font-mono text-[12px] leading-5 text-slate-200"><code>{step.lines.map((line, index) => <LogLine index={index + 1} key={`${line.timestamp}:${index}`} level={line.level} text={line.text} />)}</code></pre> : <div className="px-12 py-5 text-xs text-muted-foreground">{active && step.status === "in_progress" ? "Waiting for output from this step…" : "No console output for this step."}</div>}
                      </div> : null}
                    </div>;
                  })}
                </div>
                {active && !following ? <Button className="absolute bottom-4 right-4 shadow-lg" onClick={jumpToLatest} size="sm" variant="secondary"><ArrowDown />Jump to latest</Button> : null}
              </CardContent>
            </Card>
          </div>
        </div>
      ) : undefined}
    </ResourcePage>
  );
}

function Meta({ label, value }: { label: string; value: string }) {
  return <div><div className="text-[10px] uppercase tracking-[0.12em] text-muted-foreground">{label}</div><div className="mt-1 font-medium">{value}</div></div>;
}

function StepIcon({ conclusion, status }: { conclusion: string | null; status: string }) {
  if (conclusion === "failure") return <CircleX className="size-4 shrink-0 text-red-400" />;
  if (conclusion === "success") return <CircleCheck className="size-4 shrink-0 text-emerald-400" />;
  if (status === "in_progress") return <LoaderCircle className="size-4 shrink-0 animate-spin text-sky-400" />;
  return <Circle className="size-4 shrink-0 text-muted-foreground" />;
}

function LogLine({ index, level, text }: { index: number; level: string; text: string }) {
  return <span className={cn(
    "grid min-w-max grid-cols-[3.5rem_minmax(0,1fr)] px-4",
    level === "error" && "bg-red-500/15 text-red-200",
    level === "warning" && "bg-amber-500/12 text-amber-100",
    level === "command" && "text-sky-200",
    level === "group" && "mt-1 font-semibold text-slate-100",
    level === "notice" && "text-emerald-200",
  )}><span className="select-none pr-4 text-right text-slate-600">{index}</span><span className="whitespace-pre-wrap break-words pr-4">{level === "group" ? `› ${text}` : text || " "}</span></span>;
}

function annotationSummary(job: StructuredJobLog) {
  const errors = job.annotations.filter((annotation) => annotation.level === "error").length;
  const warnings = job.annotations.filter((annotation) => annotation.level === "warning").length;
  return `${errors} ${errors === 1 ? "error" : "errors"} and ${warnings} ${warnings === 1 ? "warning" : "warnings"}`;
}
