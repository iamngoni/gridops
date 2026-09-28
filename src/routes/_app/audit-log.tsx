import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { Bot, FileClock } from "lucide-react";
import { useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { EmptyState, ListGroup, PageBody, PageHeader, listRowClassName } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Tooltip } from "~/components/ui/tooltip";
import { type AuditEvent, getAuditLogPage } from "~/features/operations/operations.functions";
import { validatePageSearch } from "~/lib/pagination";
import { cn, formatDateTime } from "~/lib/utils";

export const Route = createFileRoute("/_app/audit-log")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getAuditLogPage({ page: deps.page }),
  pendingComponent: () => <ResourcePageLoading icon={FileClock} title="Audit log" />,
  component: AuditLogPage,
});

function dayLabel(value: string) {
  const date = new Date(value);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return "Today";
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString([], { weekday: "long", month: "short", day: "numeric", year: date.getFullYear() === today.getFullYear() ? undefined : "numeric" });
}

function AuditLogPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  const days = new Map<string, AuditEvent[]>();
  for (const event of data.items) {
    const label = dayLabel(event.createdAt);
    days.set(label, [...(days.get(label) ?? []), event]);
  }

  return (
    <>
      <PageHeader count={data.total || undefined} icon={FileClock} title="Audit log" />
      <PageBody>
        {data.items.length === 0 ? (
          <EmptyState description="User actions and automated reconciliation decisions are recorded here." icon={FileClock} title="No audit events yet" />
        ) : (
          <>
            {[...days.entries()].map(([label, events]) => (
              <ListGroup count={events.length} key={label} label={label}>
                {events.map((event) => <AuditRow event={event} key={event.id} />)}
              </ListGroup>
            ))}
            <ListPagination itemCount={data.items.length} noun="events" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} />
          </>
        )}
      </PageBody>
    </>
  );
}

function AuditRow({ event }: { event: AuditEvent }) {
  const [open, setOpen] = useState(false);
  const hasMetadata = Boolean(event.metadata && event.metadata !== "{}");
  const system = event.actorLabel === "system";
  let metadata = event.metadata;
  try {
    metadata = JSON.stringify(JSON.parse(event.metadata), null, 2);
  } catch {
    // Show non-JSON detail verbatim.
  }
  return (
    <div className="border-b border-border">
      <div className={cn(listRowClassName, "relative border-b-0 pr-3", hasMetadata && "cursor-pointer")}>
        {hasMetadata ? <button aria-expanded={open} aria-label="Toggle event details" className="absolute inset-0 outline-none" data-list-row="" onClick={() => setOpen((current) => !current)} type="button" /> : null}
        {system ? <span className="grid size-[18px] shrink-0 place-items-center rounded-full bg-selected text-muted-foreground"><Bot className="size-3" /></span> : <Avatar name={event.actorLabel} size={18} src={githubAvatar(event.actorLabel.replace(/^@/, ""))} />}
        <span className="hidden w-28 shrink-0 truncate text-xs text-muted-foreground sm:block">{system ? "GridOps" : event.actorLabel}</span>
        <span className="min-w-0 shrink truncate font-mono text-xs font-medium text-foreground">{event.action}</span>
        <span className="flex-1" />
        <AuditTarget id={event.targetId} type={event.targetType} />
        <Tooltip content={new Date(event.createdAt).toLocaleString()}>
          <span className="tabular relative w-24 shrink-0 text-right text-xs text-faint">{formatDateTime(event.createdAt).split(", ").pop()}</span>
        </Tooltip>
      </div>
      {open ? <pre className="mx-4 mb-3 ml-[42px] max-h-64 overflow-auto rounded-md border border-border bg-panel-subtle p-3 font-mono text-2xs leading-5 text-secondary-foreground">{metadata}</pre> : null}
    </div>
  );
}

function AuditTarget({ id, type }: { id: string | null; type: string }) {
  const label = <><span className="text-muted-foreground">{type.replaceAll("_", " ")}</span>{id ? <span className="ml-1.5 font-mono text-faint">{id.slice(0, 8)}</span> : null}</>;
  const className = "relative hidden max-w-56 truncate rounded-md px-1.5 py-0.5 text-xs hover:bg-selected md:block";
  if (id && type === "runner_pool") return <Link className={className} params={{ poolId: id }} to="/runner-pools/$poolId">{label}</Link>;
  if (id && type === "workflow_run") return <Link className={className} params={{ runId: id }} to="/workflow-runs/$runId">{label}</Link>;
  if (id && type === "runner") return <Link className={className} search={{ target: id }} to="/live-logs">{label}</Link>;
  return <span className="hidden max-w-56 truncate px-1.5 text-xs md:block">{label}</span>;
}
