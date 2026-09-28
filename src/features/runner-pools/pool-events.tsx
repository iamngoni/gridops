import { useEffect, useState } from "react";

import { type RunnerPoolEvent, getRunnerPoolEvents } from "./runner-pools.functions";
import { StatusDot } from "~/components/status-icon";
import { Tooltip } from "~/components/ui/tooltip";
import { formatAge } from "~/lib/utils";

type EventPage = { items: RunnerPoolEvent[]; total: number; page: number; perPage: number };

export type PoolEventsState =
  | { status: "loading"; data: null; error: null }
  | { status: "ready"; data: EventPage; error: null }
  | { status: "error"; data: null; error: string };

/** Pool events for one page, refreshed every five seconds while mounted. */
export function usePoolEvents(poolId: string, page: number) {
  const [state, setState] = useState<PoolEventsState>({ status: "loading", data: null, error: null });
  useEffect(() => {
    const controller = new AbortController();
    let cancelled = false;
    const load = async () => {
      try {
        const data = await getRunnerPoolEvents(poolId, page, controller.signal);
        if (!cancelled) setState({ status: "ready", data, error: null });
      } catch (cause) {
        if (!cancelled && !(cause instanceof DOMException && cause.name === "AbortError")) {
          setState({ status: "error", data: null, error: cause instanceof Error ? cause.message : "Pool activity could not be loaded." });
        }
      }
    };
    void load();
    const interval = window.setInterval(() => void load(), 5_000);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearInterval(interval);
    };
  }, [page, poolId]);
  return state;
}

/** Linear-style activity timeline: a thin rail with one dot per event. */
export function PoolEventTimeline({ events, detailed = false }: { events: RunnerPoolEvent[]; detailed?: boolean }) {
  return (
    <ol className="relative before:absolute before:bottom-4 before:left-[7px] before:top-4 before:w-px before:bg-border">
      {events.map((event) => <PoolEventItem detailed={detailed} event={event} key={event.id} />)}
    </ol>
  );
}

function PoolEventItem({ event, detailed }: { event: RunnerPoolEvent; detailed: boolean }) {
  const tone = event.level === "error" ? "danger" : event.level === "warning" ? "warning" : "success";
  let metadata = event.metadata;
  try {
    metadata = JSON.stringify(JSON.parse(event.metadata), null, 2);
  } catch {
    // Keep non-JSON metadata readable as-is.
  }
  const capacity = event.capacitySnapshot;
  return (
    <li className="relative flex gap-3 py-2">
      <span className="relative z-[1] mt-1 grid size-[15px] shrink-0 place-items-center rounded-full bg-panel"><StatusDot pulse={false} tone={tone} /></span>
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-sm font-medium text-foreground">{event.event}</span>
          <Tooltip content={new Date(event.createdAt).toLocaleString()}>
            <time className="tabular ml-auto shrink-0 text-2xs text-faint" dateTime={event.createdAt}>{formatAge(event.createdAt)}</time>
          </Tooltip>
        </div>
        <p className="mt-0.5 text-sm leading-5 text-muted-foreground">{event.message}</p>
        {detailed && capacity ? (
          <div className="mt-2 grid gap-x-4 gap-y-1 rounded-md border border-border bg-panel-subtle px-3 py-2 text-xs text-muted-foreground sm:grid-cols-3">
            <span><span className="tabular text-foreground">{capacity.active.cpu.toLocaleString()}</span> / {capacity.cpuBudget.toLocaleString()} cores</span>
            <span><span className="tabular text-foreground">{capacity.active.memoryMb.toLocaleString()}</span> / {capacity.memoryBudgetMb.toLocaleString()} MB</span>
            <span><span className="tabular text-foreground">{capacity.active.activeRunners}</span> / {capacity.maxRunners} runners</span>
          </div>
        ) : null}
        {detailed && event.runnerId ? <p className="mt-1 font-mono text-2xs text-faint">Runner {event.runnerId}</p> : null}
        {detailed && event.metadata && event.metadata !== "{}" ? (
          <details className="group mt-1.5">
            <summary className="w-fit list-none rounded px-1 text-xs text-muted-foreground hover:bg-hover hover:text-foreground">Details</summary>
            <pre className="mt-1.5 max-h-56 overflow-auto rounded-md border border-border bg-panel-subtle p-3 font-mono text-2xs leading-5 text-secondary-foreground">{metadata}</pre>
          </details>
        ) : null}
      </div>
    </li>
  );
}
