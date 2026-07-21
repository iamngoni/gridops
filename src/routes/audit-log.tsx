import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { FileClock } from "lucide-react";

import { ResourcePage } from "~/components/resource-page";
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
              <TableCell><div className="text-xs">{event.targetType}</div><div className="mt-1 max-w-48 truncate font-mono text-[11px] text-muted-foreground" title={event.targetId ?? ""}>{event.targetId ?? "—"}</div></TableCell>
              <TableCell><code className="block max-w-96 truncate text-[11px] text-muted-foreground" title={event.metadata}>{event.metadata === "{}" ? "—" : event.metadata}</code></TableCell>
            </TableRow>
          ))}</TableBody>
        </Table><ListPagination itemCount={data.items.length} noun="audit events" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} /></CardContent></Card>
      ) : undefined}
    </ResourcePage>
  );
}
