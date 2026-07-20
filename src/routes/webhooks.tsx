import { createFileRoute } from "@tanstack/react-router";
import { CheckCircle2, ShieldAlert, Webhook } from "lucide-react";

import { ResourcePage } from "~/components/resource-page";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { Card, CardContent } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getWebhooksPage } from "~/features/operations/operations.functions";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/webhooks")({
  loader: () => getWebhooksPage(),
  component: WebhooksPage,
});

function WebhooksPage() {
  const data = Route.useLoaderData();
  return (
    <ResourcePage
      title="Webhooks"
      description="Inspect GitHub deliveries, signature checks, processing state, and failures."
      icon={Webhook}
      emptyTitle="No webhook deliveries"
      emptyDescription="Verified GitHub App deliveries will appear here with their processing history."
      action="Open settings"
      actionHref="/settings"
    >
      {data.items.length > 0 ? (
        <Card><CardContent className="px-0 py-0"><Table>
          <TableHeader><TableRow><TableHead>Delivery</TableHead><TableHead>Event</TableHead><TableHead>Destination</TableHead><TableHead>Signature</TableHead><TableHead>Status</TableHead><TableHead>Received</TableHead></TableRow></TableHeader>
          <TableBody>{data.items.map((delivery) => (
            <TableRow key={delivery.id}>
              <TableCell><div className="max-w-44 truncate font-mono text-xs" title={delivery.id}>{delivery.id}</div>{delivery.error ? <div className="mt-1 max-w-64 truncate text-[11px] text-red-400" title={String(delivery.error)}>{String(delivery.error)}</div> : null}</TableCell>
              <TableCell><Badge variant="outline">{delivery.event}</Badge>{delivery.action ? <div className="mt-1 text-[11px] text-muted-foreground">{delivery.action}</div> : null}</TableCell>
              <TableCell><div className="text-xs">{delivery.repository ?? delivery.accountLogin ?? "GitHub App"}</div><div className="mt-1 font-mono text-[11px] text-muted-foreground">{delivery.installationId ? `installation ${delivery.installationId}` : "global"}</div></TableCell>
              <TableCell>{delivery.signatureValid ? <span className="inline-flex items-center gap-1.5 text-xs text-emerald-400"><CheckCircle2 className="size-3.5" />Verified</span> : <span className="inline-flex items-center gap-1.5 text-xs text-red-400"><ShieldAlert className="size-3.5" />Invalid</span>}</TableCell>
              <TableCell><StatusBadge status={delivery.status} /></TableCell>
              <TableCell className="text-xs text-muted-foreground">{formatRelativeTime(delivery.receivedAt)}</TableCell>
            </TableRow>
          ))}</TableBody>
        </Table></CardContent></Card>
      ) : undefined}
    </ResourcePage>
  );
}
