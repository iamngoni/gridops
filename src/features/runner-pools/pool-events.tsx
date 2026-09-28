import { Link } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { type RunnerPoolEvent, getRunnerPoolEvents } from "./runner-pools.functions";
import { StatusDot } from "~/components/status-icon";
import { Tooltip } from "~/components/ui/tooltip";
import { cn, formatAge } from "~/lib/utils";

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

/** Event names embed GitHub states such as "in_progress"; show them as words. */
export function humanizeEvent(value: string) {
  return value.replaceAll("_", " ");
}

const columns = "grid grid-cols-[14px_minmax(0,1fr)_64px] items-center gap-x-3 md:grid-cols-[14px_minmax(160px,220px)_minmax(0,1fr)_104px_64px]";

/** Pool events as an aligned table: event, details, runner, and age, with expandable metadata. */
export function PoolEventTable({ events, stickyHeader = true }: { events: RunnerPoolEvent[]; stickyHeader?: boolean }) {
  return (
    <div role="table">
      <div className={cn(columns, "h-8 border-b border-border bg-panel-subtle px-4 text-xs font-medium text-muted-foreground", stickyHeader && "sticky top-0 z-10")} role="row">
        <span />
        <span role="columnheader">Event</span>
        <span className="hidden md:block" role="columnheader">Details</span>
        <span className="hidden md:block" role="columnheader">Runner</span>
        <span className="text-right" role="columnheader">When</span>
      </div>
      {events.map((event) => <PoolEventRow event={event} key={event.id} />)}
    </div>
  );
}

function PoolEventRow({ event }: { event: RunnerPoolEvent }) {
  const [open, setOpen] = useState(false);
  const tone = event.level === "error" ? "danger" : event.level === "warning" ? "warning" : "success";
  const capacity = event.capacitySnapshot;
  const hasMetadata = Boolean(event.metadata && event.metadata !== "{}");
  const expandable = hasMetadata || Boolean(capacity);
  let metadata = event.metadata;
  try {
    metadata = JSON.stringify(JSON.parse(event.metadata), null, 2);
  } catch {
    // Keep non-JSON metadata readable as-is.
  }
  return (
    <div className="border-b border-border" role="rowgroup">
      <div
        aria-expanded={expandable ? open : undefined}
        className={cn(columns, "min-h-10 px-4 py-2 text-sm", expandable && "cursor-pointer hover:bg-hover")}
        onClick={expandable ? () => setOpen((current) => !current) : undefined}
        role="row"
      >
        <StatusDot pulse={false} tone={tone} />
        <span className="truncate font-medium text-foreground" role="cell">{humanizeEvent(event.event)}</span>
        <span className="hidden truncate text-muted-foreground md:block" role="cell" title={event.message}>{event.message}</span>
        <span className="hidden truncate font-mono text-2xs text-faint md:block" role="cell">
          {event.runnerId ? <Link className="hover:text-foreground" onClick={(click) => click.stopPropagation()} search={{ target: event.runnerId }} to="/live-logs">{event.runnerId.slice(0, 8)}</Link> : "—"}
        </span>
        <Tooltip content={new Date(event.createdAt).toLocaleString()}>
          <time className="tabular text-right text-xs text-faint" dateTime={event.createdAt} role="cell">{formatAge(event.createdAt)}</time>
        </Tooltip>
      </div>
      {open ? (
        <div className="space-y-2 px-4 pb-3 md:pl-[calc(1rem+14px+0.75rem)]">
          <p className="text-sm text-muted-foreground md:hidden">{event.message}</p>
          {capacity ? (
            <div className="grid gap-x-6 gap-y-1 rounded-md border border-border bg-panel-subtle px-3 py-2 text-xs text-muted-foreground sm:grid-cols-3">
              <span><span className="tabular text-foreground">{capacity.active.cpu.toLocaleString()}</span> / {capacity.cpuBudget.toLocaleString()} cores in use</span>
              <span><span className="tabular text-foreground">{capacity.active.memoryMb.toLocaleString()}</span> / {capacity.memoryBudgetMb.toLocaleString()} MB in use</span>
              <span><span className="tabular text-foreground">{capacity.active.activeRunners}</span> / {capacity.maxRunners} runners</span>
            </div>
          ) : null}
          {hasMetadata ? <pre className="max-h-56 overflow-auto rounded-md border border-border bg-panel-subtle p-3 font-mono text-2xs leading-5 text-secondary-foreground">{metadata}</pre> : null}
        </div>
      ) : null}
    </div>
  );
}
