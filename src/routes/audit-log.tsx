import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowUpRight, FileClock } from "lucide-react";

import { ResourcePage } from "~/components/resource-page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { ListPagination } from "~/components/list-pagination";
import { Badge } from "~/components/ui/badge";
import { Card, CardContent } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getAuditLogPage } from "~/features/operations/operations.functions";
import { validatePageSearch } from "~/lib/pagination";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/audit-log")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getAuditLogPage({ page: deps.page }),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Audit log"
      description="Trace configuration and runner lifecycle actions across the control plane."
      icon={FileClock}
    />
  ),
  component: AuditLogPage,
});

function AuditLogPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  return (
    <ResourcePage
      title="Audit log"
      description="Trace configuration and runner lifecycle actions across the control plane."
      icon={FileClock}
      emptyTitle="No audit events"
      emptyDescription="User actions and automated reconciliation decisions will be recorded here."
    >
      {data.items.length > 0 ? (
        <Card><CardContent className="px-0 py-0"><Table>
          <TableHeader><TableRow><TableHead>Time</TableHead><TableHead>Actor</TableHead><TableHead>Action</TableHead><TableHead>Target</TableHead><TableHead>Details</TableHead></TableRow></TableHeader>
          <TableBody>{data.items.map((event) => (
            <TableRow key={event.id}>
              <TableCell><div className="text-xs">{formatRelativeTime(event.createdAt)}</div><div className="mt-1 text-[11px] text-muted-foreground">{new Date(event.createdAt).toLocaleString()}</div></TableCell>
              <TableCell><Badge variant={event.actorLabel === "system" ? "secondary" : "outline"}>{event.actorLabel}</Badge></TableCell>
              <TableCell className="font-mono text-xs">{event.action}</TableCell>
              <TableCell><AuditTarget id={event.targetId} type={event.targetType} /></TableCell>
              <TableCell><AuditMetadata value={event.metadata} /></TableCell>
            </TableRow>
          ))}</TableBody>
        </Table><ListPagination itemCount={data.items.length} noun="audit events" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} /></CardContent></Card>
      ) : undefined}
    </ResourcePage>
  );
}

function AuditTarget({ id, type }: { id: string | null; type: string }) {
  const label = <><span className="max-w-48 truncate font-mono text-[11px]">{id ?? "—"}</span>{id ? <ArrowUpRight className="size-3" /> : null}</>;
  const className = "mt-1 inline-flex items-center gap-1 text-muted-foreground hover:text-primary";
  return <div><div className="text-xs capitalize">{type.replaceAll("_", " ")}</div>{id && type === "runner_pool" ? <Link className={className} params={{ poolId: id }} to="/runner-pools/$poolId">{label}</Link> : id && type === "workflow_run" ? <Link className={className} params={{ runId: id }} to="/workflow-runs/$runId">{label}</Link> : id && type === "runner" ? <Link className={className} search={{ target: id }} to="/live-logs">{label}</Link> : <div className="mt-1 flex items-center gap-1 text-muted-foreground">{label}</div>}</div>;
}

function AuditMetadata({ value }: { value: string }) {
  if (!value || value === "{}") return <span className="text-muted-foreground">—</span>;
  let formatted = value;
  try { formatted = JSON.stringify(JSON.parse(value), null, 2); } catch { /* Preserve non-JSON audit detail verbatim. */ }
  return <details className="group max-w-96"><summary className="cursor-pointer list-none truncate text-[11px] text-muted-foreground hover:text-foreground">{value}<span className="ml-2 text-primary/70 group-open:hidden">View</span></summary><pre className="mt-2 max-h-52 overflow-auto rounded-lg bg-background/70 p-3 text-[11px] leading-5 text-foreground/80">{formatted}</pre></details>;
}
