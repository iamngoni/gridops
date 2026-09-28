import { Link } from "@tanstack/react-router";
import { ArrowDown, ChevronRight, CircleX, LoaderCircle, RefreshCw, Search, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type * as React from "react";

import { InlineError } from "~/components/page";
import { RunStatusIcon } from "~/components/status-icon";
import { Button, buttonVariants } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";
import { type StructuredJobLog, getWorkflowJobLogAction } from "~/features/operations/operations.functions";
import { advanceFollowedSteps, isNearLogEnd } from "~/lib/log-follow";
import { cn, formatDuration } from "~/lib/utils";

export type JobLogFallback = {
  name: string;
  status: string;
  repository?: string | null;
  workflowName?: string;
  runNumber?: number;
  runId?: number;
};

const isActive = (status: string | null | undefined) => status === "queued" || status === "in_progress" || status === "waiting";

/**
 * Structured log reader for one workflow job: steps, annotations, search, and
 * live follow while the job runs. Mount with `key={jobId}` to switch jobs.
 */
export function JobLogViewer({
  jobId,
  fallback,
  showRunLink = true,
  actions,
}: {
  jobId: number;
  fallback?: JobLogFallback;
  showRunLink?: boolean;
  actions?: React.ReactNode;
}) {
  const [jobLog, setJobLog] = useState<StructuredJobLog | null>(null);
  const [expandedSteps, setExpandedSteps] = useState<Set<number>>(new Set());
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [following, setFollowing] = useState(true);
  const followingRef = useRef(true);
  const loadedOnce = useRef(false);
  const logViewport = useRef<HTMLDivElement>(null);
  const active = isActive(jobLog?.status ?? fallback?.status);

  const updateFollowing = useCallback((value: boolean) => {
    followingRef.current = value;
    setFollowing(value);
  }, []);

  const refreshLog = useCallback(async (showLoading = false) => {
    if (showLoading) setLoading(true);
    try {
      const response = await getWorkflowJobLogAction({ data: { jobId } });
      setJobLog(response);
      setError(null);
      if (!loadedOnce.current) {
        loadedOnce.current = true;
        const failed = response.steps.filter((step) => step.conclusion === "failure" || step.status === "in_progress").map((step) => step.number);
        const recommended = advanceFollowedSteps(response.steps, new Set(failed));
        const firstStep = response.steps[0]?.number;
        setExpandedSteps(recommended.size ? recommended : new Set(firstStep === undefined ? [] : [firstStep]));
      } else if (followingRef.current && isActive(response.status)) {
        setExpandedSteps((current) => {
          const next = advanceFollowedSteps(response.steps, current);
          if (next.size === current.size && [...next].every((number) => current.has(number))) return current;
          return next;
        });
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not load the job log.");
    } finally {
      setLoading(false);
    }
  }, [jobId]);

  useEffect(() => {
    const initial = window.setTimeout(() => void refreshLog(), 0);
    return () => window.clearTimeout(initial);
  }, [refreshLog]);

  useEffect(() => {
    if (!active) return undefined;
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") void refreshLog();
    }, 4_000);
    return () => window.clearInterval(interval);
  }, [active, refreshLog]);

  useLayoutEffect(() => {
    const viewport = logViewport.current;
    if (!viewport || !following || !active) return;
    viewport.scrollTop = viewport.scrollHeight;
  }, [active, expandedSteps, following, jobLog?.lineCount]);

  const visibleSteps = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!jobLog || !normalized) return jobLog?.steps ?? [];
    return jobLog.steps
      .map((step) => ({ ...step, lines: step.lines.filter((line) => line.text.toLowerCase().includes(normalized)) }))
      .filter((step) => step.name.toLowerCase().includes(normalized) || step.lines.length > 0);
  }, [jobLog, query]);

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

  const status = jobLog?.conclusion ?? jobLog?.status ?? fallback?.status ?? "queued";
  const runId = jobLog?.runId ?? fallback?.runId;
  const waitingForRunner = jobLog?.source === "pending" || (!jobLog?.steps.length && status === "queued");

  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-2">
        <RunStatusIcon status={status} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{jobLog?.name ?? fallback?.name ?? "Job log"}</div>
          <div className="truncate text-xs text-muted-foreground">
            {jobLog?.repository ?? fallback?.repository} · {jobLog?.workflowName ?? fallback?.workflowName} #{jobLog?.runNumber ?? fallback?.runNumber}
            {jobLog && !waitingForRunner ? <> · {formatDuration(jobLog.startedAt, jobLog.completedAt)} · {jobLog.source === "github" ? "GitHub log" : "live runner"}</> : null}
          </div>
        </div>
        {active && !waitingForRunner ? (
          <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className={cn("size-1.5 rounded-full", following ? "bg-success live-pulse" : "bg-warning")} />{following ? "Following" : "Paused"}
          </span>
        ) : null}
        <label className="relative hidden h-7 w-52 items-center sm:flex">
          <Search className="pointer-events-none absolute left-2 size-3.5 text-muted-foreground" />
          <input
            aria-label="Search job output"
            className="h-7 w-full rounded-md border border-border-strong bg-panel pl-7 pr-2 text-xs outline-none placeholder:text-faint focus:border-primary/70"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search output…"
            value={query}
          />
        </label>
        {showRunLink && runId ? <Link className={buttonVariants({ size: "sm", variant: "ghost" })} params={{ runId: String(runId) }} to="/workflow-runs/$runId">View run</Link> : null}
        <Tooltip content="Refresh log">
          <Button aria-label="Refresh job log" disabled={loading} onClick={() => void refreshLog(true)} size="icon-sm" variant="ghost">
            {loading ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
          </Button>
        </Tooltip>
        {actions}
      </div>

      {error || jobLog?.metadataWarning || jobLog?.truncated ? (
        <div className="shrink-0 space-y-1 border-b border-border px-4 py-2 text-xs">
          {error ? <InlineError className="mt-0" onRetry={() => void refreshLog(true)} title="Couldn’t load this log">{error}</InlineError> : null}
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
          {jobLog && waitingForRunner ? (
            <div className="grid h-full place-items-center p-6 text-center">
              <div>
                <RunStatusIcon className="mx-auto" size={20} status="queued" />
                <p className="mt-3 text-sm font-medium text-[#d0d6e0]">Waiting for a runner</p>
                <p className="mt-1 max-w-xs text-xs text-[#8a8f98]">Output appears here as soon as a runner picks this job up.</p>
              </div>
            </div>
          ) : null}
          {jobLog && !waitingForRunner && visibleSteps.length === 0 ? <div className="p-6 text-sm text-muted-foreground">{query ? `No step output matches “${query}”.` : "This job has no step output."}</div> : null}
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
        {active && !following && !waitingForRunner ? <Button className="absolute bottom-4 right-4 shadow-popover" onClick={jumpToLatest} size="sm" variant="secondary"><ArrowDown />Jump to latest</Button> : null}
      </div>
    </section>
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
