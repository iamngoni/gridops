import { createFileRoute, useNavigate } from "@tanstack/react-router";
import * as Dialog from "@radix-ui/react-dialog";
import { Braces, CheckCircle2, Copy, LoaderCircle, RefreshCw, Settings, ShieldAlert, Webhook, X } from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";

import { AsyncActionButton } from "~/components/async-action-button";
import { ListPagination } from "~/components/list-pagination";
import { ResourcePage } from "~/components/resource-page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getWebhookPayloadAction, getWebhooksPage, retryWebhookAction } from "~/features/operations/operations.functions";
import type { WebhookDelivery, WebhookPayload } from "~/features/operations/operations.functions";
import { validatePageSearch } from "~/lib/pagination";
import { formatRelativeTime } from "~/lib/utils";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/webhooks")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getWebhooksPage({ page: deps.page }),
  pendingComponent: () => (
    <ResourcePageLoading
      title="Webhooks"
      description="Inspect GitHub deliveries, signature checks, processing state, and failures."
      icon={Webhook}
    />
  ),
  component: WebhooksPage,
});

function WebhooksPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(10_000, data.authenticated);
  const retry = retryWebhookAction;
  const [payloadDelivery, setPayloadDelivery] = useState<WebhookDelivery | null>(null);
  const [payload, setPayload] = useState<WebhookPayload | null>(null);
  const [payloadLoading, setPayloadLoading] = useState(false);
  const [payloadError, setPayloadError] = useState<string | null>(null);
  const payloadTriggerRef = useRef<HTMLButtonElement | null>(null);

  async function viewPayload(delivery: WebhookDelivery) {
    setPayloadDelivery(delivery);
    setPayload(null);
    setPayloadError(null);
    setPayloadLoading(true);
    try {
      setPayload(await getWebhookPayloadAction({ data: { deliveryId: delivery.id } }));
    } catch (error) {
      setPayloadError(error instanceof Error ? error.message : "The webhook payload could not be loaded.");
    } finally {
      setPayloadLoading(false);
    }
  }
  return (
    <ResourcePage
      title="Webhooks"
      description="Inspect GitHub deliveries, signature checks, processing state, and failures."
      icon={Webhook}
      emptyTitle="No webhook deliveries"
      emptyDescription="Verified GitHub App deliveries will appear here with their processing history."
      action="Open settings"
      actionHref="/settings"
      actionIcon={Settings}
    >
      {data.items.length > 0 ? (
        <Card><CardContent className="px-0 py-0"><Table>
          <TableHeader><TableRow><TableHead>Delivery</TableHead><TableHead>Event</TableHead><TableHead>Destination</TableHead><TableHead>Signature</TableHead><TableHead>Status</TableHead><TableHead>Received</TableHead><TableHead className="text-right">Actions</TableHead></TableRow></TableHeader>
          <TableBody>{data.items.map((delivery) => (
            <TableRow key={delivery.id}>
              <TableCell><button className="group inline-flex max-w-52 items-center gap-1.5 font-mono text-xs text-muted-foreground hover:text-foreground" onClick={() => void navigator.clipboard.writeText(delivery.id).then(() => toast.success("Delivery ID copied."))} title="Copy delivery ID" type="button"><span className="truncate">{delivery.id}</span><Copy className="size-3 shrink-0 opacity-40 group-hover:opacity-100" /></button>{delivery.error ? <details className="mt-1 max-w-72 text-[11px] text-red-400"><summary className="cursor-pointer truncate">{String(delivery.error)}</summary><p className="mt-1 whitespace-pre-wrap break-words rounded-md bg-red-500/5 p-2 leading-5">{String(delivery.error)}</p></details> : null}</TableCell>
              <TableCell><Badge variant="outline">{delivery.event}</Badge>{delivery.action ? <div className="mt-1 text-[11px] text-muted-foreground">{delivery.action}</div> : null}</TableCell>
              <TableCell><div className="text-xs">{delivery.repository ?? delivery.accountLogin ?? "GitHub App"}</div><div className="mt-1 font-mono text-[11px] text-muted-foreground">{delivery.installationId ? `installation ${delivery.installationId}` : "global"}</div></TableCell>
              <TableCell>{delivery.signatureValid ? <span className="inline-flex items-center gap-1.5 text-xs text-emerald-400"><CheckCircle2 className="size-3.5" />Verified</span> : <span className="inline-flex items-center gap-1.5 text-xs text-red-400"><ShieldAlert className="size-3.5" />Invalid</span>}</TableCell>
              <TableCell><StatusBadge status={delivery.status} /></TableCell>
              <TableCell className="text-xs text-muted-foreground">{formatRelativeTime(delivery.receivedAt)}</TableCell>
              <TableCell><div className="flex justify-end gap-1">{delivery.hasPayload ? <Button aria-label="View request payload" onClick={(event) => { payloadTriggerRef.current = event.currentTarget; void viewPayload(delivery); }} size="icon" title="View request payload" variant="ghost"><Braces /></Button> : null}{delivery.canRetry && delivery.status === "failed" && delivery.signatureValid ? <AsyncActionButton action={() => retry({ data: { deliveryId: delivery.id } })} icon={<RefreshCw />} size="icon" success="Webhook delivery reprocessed." title="Retry delivery"><span className="sr-only">Retry delivery</span></AsyncActionButton> : null}</div></TableCell>
            </TableRow>
          ))}</TableBody>
        </Table><ListPagination itemCount={data.items.length} noun="webhook deliveries" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} /></CardContent></Card>
      ) : undefined}
      {payloadDelivery ? <WebhookPayloadDialog delivery={payloadDelivery} error={payloadError} loading={payloadLoading} onClose={() => {
        const trigger = payloadTriggerRef.current;
        setPayloadDelivery(null);
        window.setTimeout(() => trigger?.focus());
      }} payload={payload} /> : null}
    </ResourcePage>
  );
}

function WebhookPayloadDialog({ delivery, error, loading, onClose, payload }: { delivery: WebhookDelivery; error: string | null; loading: boolean; onClose: () => void; payload: WebhookPayload | null }) {
  const formatted = payload?.payload == null ? "" : JSON.stringify(payload.payload, null, 2);
  return (
    <Dialog.Root onOpenChange={(open) => { if (!open) onClose(); }} open>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-[80] bg-black/75 backdrop-blur-sm" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-[81] flex max-h-[88vh] w-[calc(100%_-_2rem)] max-w-5xl -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border/80 bg-card shadow-2xl">
          <header className="flex items-start gap-4 border-b border-border/70 p-5">
            <div className="grid size-10 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary"><Braces className="size-5" /></div>
            <div className="min-w-0 flex-1">
              <Dialog.Title className="font-semibold">Webhook request payload</Dialog.Title>
              <Dialog.Description className="mt-1 truncate font-mono text-[11px] text-muted-foreground">{delivery.event} · {delivery.id}{payload ? ` · ${formatBytes(payload.payloadBytes)}` : ""}</Dialog.Description>
            </div>
            {formatted ? <Button onClick={() => void navigator.clipboard.writeText(formatted).then(() => toast.success("Webhook payload copied."))} size="sm" variant="outline"><Copy />Copy JSON</Button> : null}
            <Dialog.Close asChild><Button size="icon" title="Close payload viewer" variant="ghost"><X /><span className="sr-only">Close payload viewer</span></Button></Dialog.Close>
          </header>
          <div className="min-h-72 flex-1 overflow-auto bg-[hsl(162_28%_4%)] p-5">
            {loading ? <div className="grid min-h-64 place-items-center text-sm text-muted-foreground"><span className="inline-flex items-center gap-2"><LoaderCircle className="size-4 animate-spin text-primary" />Loading stored payload…</span></div> : error ? <div className="rounded-lg border border-red-500/20 bg-red-500/5 p-4 text-sm text-red-300">{error}</div> : formatted ? <pre className="whitespace-pre-wrap break-words font-mono text-xs leading-5 text-emerald-50/85"><code>{formatted}</code></pre> : <div className="grid min-h-64 place-items-center text-sm text-muted-foreground">No request payload was retained for this delivery.</div>}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}
