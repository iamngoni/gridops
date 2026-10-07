import {
  ChevronRight,
  CircleCheck,
  CircleDot,
  CircleX,
  ExternalLink,
  GitBranch,
  GitPullRequestArrow,
  LoaderCircle,
  MessageSquareText,
  Sparkles,
  Square,
  Wrench,
} from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { agentRunIconStatus, agentRunLabel, formatElapsed, isTerminalAgentStatus, triggerLabel } from "./agent-state";
import { type AgentRun, type AgentRunEvent, type AgentRunSummary, cancelAgentRun } from "./agent.functions";
import { confirmAction } from "~/components/confirm-dialog";
import { Callout, InlineError } from "~/components/page";
import { RunStatusIcon } from "~/components/status-icon";
import { Button, buttonVariants } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";
import { isNearLogEnd } from "~/lib/log-follow";
import { cn, formatDateTime } from "~/lib/utils";

/**
 * The latest "Fix with agent" run for a job: status, outcome, and a timeline of
 * what the agent did. Collapsible, and capped in height so the log stays in view.
 * Mount with `key={agentRunId}` so each run opens expanded.
 */
export function AgentRunPanel({
  run,
  summary,
  error,
  onRetry,
  onUpdate,
}: {
  run: AgentRun | null;
  summary: AgentRunSummary | null;
  error: string | null;
  onRetry: () => void;
  onUpdate: (run: AgentRun) => void;
}) {
  const [open, setOpen] = useState(true);
  const [cancelling, setCancelling] = useState(false);
  const status = run?.status ?? summary?.status ?? "queued";
  const terminal = isTerminalAgentStatus(status);
  const pullRequestUrl = run?.pullRequestUrl ?? summary?.pullRequestUrl ?? null;
  const pullRequestNumber = run?.pullRequestNumber ?? summary?.pullRequestNumber ?? null;
  const label = agentRunLabel({ status, outcome: run?.outcome ?? summary?.outcome ?? null, pullRequestNumber });
  const stage = !terminal ? run?.stage : null;

  async function cancel() {
    if (!run) return;
    if (!(await confirmAction("Cancel this agent run? It stops working on the fix; nothing it hasn’t pushed is kept.", { confirmLabel: "Cancel run" }))) return;
    setCancelling(true);
    try {
      onUpdate(await cancelAgentRun(run.id));
      toast.success("Cancellation requested.");
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : "Could not cancel the agent run.");
    } finally {
      setCancelling(false);
    }
  }

  return (
    <section aria-label="Fix with agent" className="shrink-0 border-b border-border">
      <div className="flex min-h-9 items-center gap-2 px-4 py-1">
        <button
          aria-expanded={open}
          className="-ml-1 flex min-w-0 flex-1 items-center gap-2 rounded-md py-1 pl-1 pr-1.5 text-left text-xs hover:bg-hover"
          onClick={() => setOpen((current) => !current)}
          type="button"
        >
          <ChevronRight className={cn("size-3.5 shrink-0 text-faint transition-transform", open && "rotate-90")} />
          <RunStatusIcon status={agentRunIconStatus(status)} />
          <span className="shrink-0 font-medium text-foreground">Agent</span>
          <span className="min-w-0 truncate text-muted-foreground">
            {label}{stage ? <span className="text-faint"> · {stage}</span> : null}
          </span>
        </button>
        {run ? <Elapsed run={run} /> : null}
        {/* Expanded, the outcome below carries the link. */}
        {pullRequestUrl && (!open || !run) ? (
          <a className={buttonVariants({ size: "xs", variant: "ghost" })} href={pullRequestUrl} rel="noreferrer" target="_blank">
            <GitPullRequestArrow />{pullRequestNumber ? `#${pullRequestNumber}` : "Pull request"}<ExternalLink />
          </a>
        ) : null}
        {run?.canCancel ? (
          <Button disabled={cancelling} onClick={() => void cancel()} size="xs" variant="ghost">
            {cancelling ? <LoaderCircle className="animate-spin" /> : <Square />}Cancel
          </Button>
        ) : null}
      </div>
      {open ? (
        <AgentRunBody error={error} onRetry={onRetry} run={run} />
      ) : null}
    </section>
  );
}

function AgentRunBody({ run, error, onRetry }: { run: AgentRun | null; error: string | null; onRetry: () => void }) {
  const viewport = useRef<HTMLDivElement>(null);
  const stickToEnd = useRef(true);
  const terminal = run ? isTerminalAgentStatus(run.status) : false;
  const [showActivity, setShowActivity] = useState<boolean | null>(null);
  const activityOpen = showActivity ?? !terminal;
  const eventCount = run?.events.length ?? 0;

  // While the agent works, keep the newest step in view unless the reader scrolled up.
  useLayoutEffect(() => {
    const element = viewport.current;
    if (!element || terminal || !stickToEnd.current) return;
    element.scrollTop = element.scrollHeight;
  }, [eventCount, terminal]);

  if (!run) {
    return (
      <div className="px-4 pb-3">
        {error ? <InlineError className="mt-0" onRetry={onRetry} title="Couldn’t load the agent run">{error}</InlineError> : (
          <div className="flex items-center gap-2 text-xs text-muted-foreground"><LoaderCircle className="size-3.5 animate-spin" />Loading the agent run…</div>
        )}
      </div>
    );
  }

  return (
    <div
      className="max-h-[min(22rem,38dvh)] overflow-y-auto overscroll-contain px-4 pb-3"
      onScroll={() => {
        if (viewport.current) stickToEnd.current = isNearLogEnd(viewport.current);
      }}
      ref={viewport}
    >
      <RunMeta run={run} />
      {error ? <InlineError onRetry={onRetry} title="Couldn’t refresh the agent run">{error}</InlineError> : null}
      <RunOutcome run={run} />
      {terminal && eventCount ? (
        <button
          aria-expanded={activityOpen}
          className="mt-3 inline-flex items-center gap-1.5 rounded-md text-xs font-medium text-muted-foreground hover:text-foreground"
          onClick={() => setShowActivity(!activityOpen)}
          type="button"
        >
          <ChevronRight className={cn("size-3.5 transition-transform", activityOpen && "rotate-90")} />
          Activity<span className="tabular font-normal text-faint">{eventCount}</span>
        </button>
      ) : null}
      {activityOpen ? (
        eventCount ? (
          <ol className={cn("space-y-px", terminal ? "mt-1.5" : "mt-2.5")}>
            {run.events.map((event) => (
              // Reading a step's detail holds the timeline still.
              <AgentEventRow event={event} key={event.id} onExpand={() => { stickToEnd.current = false; }} start={run.startedAt ?? run.createdAt} />
            ))}
          </ol>
        ) : !terminal ? (
          <div className="mt-2.5 flex items-center gap-2 text-xs text-muted-foreground">
            <LoaderCircle className="size-3.5 animate-spin" />{run.status === "queued" ? "Waiting for a sandbox…" : "Waiting for the agent’s first step…"}
          </div>
        ) : null
      ) : null}
    </div>
  );
}

function RunMeta({ run }: { run: AgentRun }) {
  const model = [run.provider, run.model].filter(Boolean).join(" · ");
  const who = run.trigger === "automatic" ? "Started automatically" : run.requestedBy ? `Requested by @${run.requestedBy}` : triggerLabel(run.trigger);
  return (
    <p className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-2xs text-muted-foreground">
      <span>{who}</span>
      {model ? <><span aria-hidden="true" className="text-faint">·</span><span className="font-mono">{model}</span></> : null}
      <span aria-hidden="true" className="text-faint">·</span>
      <time dateTime={run.createdAt}>{formatDateTime(run.createdAt)}</time>
    </p>
  );
}

function RunOutcome({ run }: { run: AgentRun }) {
  if (run.status === "failed") {
    return <Callout className="mt-2.5" title="The agent couldn’t finish" tone="danger"><span className="whitespace-pre-wrap break-words">{run.error ?? "It stopped without reporting an error."}</span></Callout>;
  }
  if (run.status !== "succeeded") return null;

  if (run.outcome === "pull_request") {
    return (
      <div className="mt-2.5 rounded-md border border-border-strong bg-panel-subtle p-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          {run.pullRequestUrl ? (
            <a className={buttonVariants({ size: "sm" })} href={run.pullRequestUrl} rel="noreferrer" target="_blank">
              <GitPullRequestArrow />{run.pullRequestNumber ? `Open pull request #${run.pullRequestNumber}` : "Open pull request"}<ExternalLink />
            </a>
          ) : null}
          {run.branch ? (
            <span className="inline-flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
              <GitBranch className="size-3.5 shrink-0" /><code className="truncate font-mono text-2xs text-secondary-foreground">{run.branch}</code>
            </span>
          ) : null}
        </div>
        <p className="mt-2 text-2xs text-muted-foreground">A draft pull request from the GridOps GitHub App. Review it before merging.</p>
        {run.summary ? <p className="mt-2 whitespace-pre-wrap break-words text-xs leading-5 text-secondary-foreground">{run.summary}</p> : null}
      </div>
    );
  }

  const diagnosis = run.diagnosis ?? run.summary;
  return (
    <div className="mt-2.5 rounded-md border border-border-strong bg-panel-subtle p-3">
      <div className="flex items-center gap-1.5 text-xs font-medium text-foreground"><Sparkles className="size-3.5 text-primary" />{run.outcome === "diagnosis" ? "Diagnosis" : "Summary"}</div>
      {run.outcome === "diagnosis" && run.summary && run.diagnosis ? <p className="mt-1.5 whitespace-pre-wrap break-words text-xs font-medium leading-5 text-secondary-foreground">{run.summary}</p> : null}
      {diagnosis ? <p className="mt-1.5 whitespace-pre-wrap break-words text-xs leading-5 text-secondary-foreground">{diagnosis}</p> : <p className="mt-1.5 text-xs text-muted-foreground">The agent finished without a write-up.</p>}
      {run.outcome === "diagnosis" ? <p className="mt-2 text-2xs text-muted-foreground">This doesn’t look like a code problem, so no pull request was opened.</p> : null}
    </div>
  );
}

const eventIcons = {
  status: { icon: CircleDot, className: "text-muted-foreground" },
  message: { icon: MessageSquareText, className: "text-muted-foreground" },
  tool: { icon: Wrench, className: "text-muted-foreground" },
  result: { icon: CircleCheck, className: "text-success" },
  error: { icon: CircleX, className: "text-danger" },
} as const;

function AgentEventRow({ event, start, onExpand }: { event: AgentRunEvent; start: string; onExpand: () => void }) {
  const [open, setOpen] = useState(false);
  const { icon: Icon, className } = eventIcons[event.kind] ?? eventIcons.status;
  const offset = formatElapsed(start, event.createdAt);
  const title = (
    <>
      <Icon className={cn("mt-[3px] size-3.5 shrink-0", className)} />
      <span className={cn("min-w-0 flex-1 break-words", event.kind === "error" ? "text-danger" : "text-secondary-foreground")}>{event.title}</span>
      {event.detail ? <ChevronRight className={cn("mt-[3px] size-3.5 shrink-0 text-faint transition-transform", open && "rotate-90")} /> : null}
      {offset ? (
        <Tooltip content={new Date(event.createdAt).toLocaleString()}>
          <time className="tabular mt-px w-12 shrink-0 text-right text-2xs text-faint" dateTime={event.createdAt}>+{offset}</time>
        </Tooltip>
      ) : null}
    </>
  );
  return (
    <li>
      {event.detail ? (
        <button aria-expanded={open} className="-mx-1.5 flex w-[calc(100%+0.75rem)] items-start gap-2 rounded-md px-1.5 py-1 text-left text-xs hover:bg-hover" onClick={() => { if (!open) onExpand(); setOpen(!open); }} type="button">
          {title}
        </button>
      ) : <div className="flex items-start gap-2 py-1 text-xs">{title}</div>}
      {open && event.detail ? (
        <pre className="mb-1 ml-[1.375rem] mt-0.5 max-h-56 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border bg-panel-subtle p-2.5 font-mono text-2xs leading-4 text-secondary-foreground">{event.detail}</pre>
      ) : null}
    </li>
  );
}

/** Time the run has taken, ticking each second until it finishes. */
function Elapsed({ run }: { run: AgentRun }) {
  const terminal = isTerminalAgentStatus(run.status);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (terminal) return undefined;
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") setNow(Date.now());
    }, 1_000);
    return () => window.clearInterval(interval);
  }, [terminal]);

  const elapsed = formatElapsed(run.startedAt ?? run.createdAt, run.completedAt, now);
  if (!elapsed) return null;
  return (
    <Tooltip content={terminal ? "How long the agent ran" : "Time since the agent started"}>
      <span className="tabular shrink-0 text-xs text-faint">{elapsed}</span>
    </Tooltip>
  );
}
