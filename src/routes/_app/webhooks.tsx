import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { Copy, LoaderCircle, RefreshCw, Settings, ShieldAlert, Webhook } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { ListPagination } from "~/components/list-pagination";
import { Callout, EmptyState, ListGroup, PageBody, PageHeader, listRowClassName } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge, statusLabel } from "~/components/status-icon";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Dialog, SheetContent } from "~/components/ui/dialog";
import { Tooltip } from "~/components/ui/tooltip";
import { type WebhookDelivery, type WebhookPayload, getWebhookPayloadAction, getWebhooksPage, retryWebhookAction } from "~/features/operations/operations.functions";
import { validatePageSearch } from "~/lib/pagination";
import { useAction } from "~/lib/use-action";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";
import { cn, formatAge, formatDateTime } from "~/lib/utils";

export const Route = createFileRoute("/_app/webhooks")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getWebhooksPage({ page: deps.page }),
  pendingComponent: () => <ResourcePageLoading icon={Webhook} title="Webhooks" />,
  component: WebhooksPage,
});

const GROUPS = [
  { key: "failed", label: "Failed", status: "failed" },
  { key: "pending", label: "Received", status: "received" },
  { key: "processed", label: "Processed", status: "processed" },
] as const;

function deliveryGroup(delivery: WebhookDelivery): (typeof GROUPS)[number]["key"] {
  if (delivery.status === "failed" || delivery.status === "rejected" || !delivery.signatureValid) return "failed";
  if (delivery.status === "processed") return "processed";
  return "pending";
}

type PayloadState = { delivery: WebhookDelivery; payload: WebhookPayload | null; loading: boolean; error: string | null };

function WebhooksPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(10_000, data.authenticated);
  const [peek, setPeek] = useState<PayloadState | null>(null);
  const groups = GROUPS.map((group) => ({ ...group, deliveries: data.items.filter((delivery) => deliveryGroup(delivery) === group.key) })).filter((group) => group.deliveries.length);

  async function openPayload(delivery: WebhookDelivery) {
    setPeek({ delivery, payload: null, loading: true, error: null });
    try {
      const payload = await getWebhookPayloadAction({ data: { deliveryId: delivery.id } });
      setPeek((current) => current?.delivery.id === delivery.id ? { ...current, payload, loading: false } : current);
    } catch (error) {
      setPeek((current) => current?.delivery.id === delivery.id ? { ...current, loading: false, error: error instanceof Error ? error.message : "The payload could not be loaded." } : current);
    }
  }

  return (
    <>
      <PageHeader count={data.total || undefined} icon={Webhook} title="Webhooks" />
      <PageBody>
        {data.items.length === 0 ? (
          <EmptyState description="Verified GitHub App deliveries appear here with their processing history. Private deployments poll GitHub instead." icon={Webhook} title="No webhook deliveries">
            <Link className={buttonVariants({ variant: "outline" })} to="/settings/github"><Settings />GitHub settings</Link>
          </EmptyState>
        ) : (
          <>
            {groups.map((group) => (
              <ListGroup count={group.deliveries.length} icon={<StatusBadge iconOnly status={group.status} />} key={group.key} label={group.label}>
                {group.deliveries.map((delivery) => <DeliveryRow delivery={delivery} key={delivery.id} onOpen={() => void openPayload(delivery)} />)}
              </ListGroup>
            ))}
            <ListPagination itemCount={data.items.length} noun="deliveries" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} />
          </>
        )}
      </PageBody>
      <Dialog onOpenChange={(open) => { if (!open) setPeek(null); }} open={Boolean(peek)}>
        {peek ? <PayloadSheet state={peek} /> : null}
      </Dialog>
    </>
  );
}

function DeliveryRow({ delivery, onOpen }: { delivery: WebhookDelivery; onOpen: () => void }) {
  const run = useAction();
  return (
    <div className={cn(listRowClassName, "relative pr-2")}>
      <button aria-label={`Inspect delivery ${delivery.id}`} className="absolute inset-0" disabled={!delivery.hasPayload} onClick={onOpen} type="button" />
      <Tooltip content={statusLabel(delivery.status)}><span className="relative inline-flex"><StatusBadge iconOnly status={delivery.status} /></span></Tooltip>
      <span className="shrink-0 font-mono text-xs font-medium text-foreground">{delivery.event}</span>
      {delivery.action ? <Badge variant="outline">{delivery.action}</Badge> : null}
      {!delivery.signatureValid ? <Badge dot={false} variant="destructive"><ShieldAlert className="size-3" />Invalid signature</Badge> : null}
      {delivery.error ? <span className="hidden min-w-0 truncate text-xs text-danger md:block" title={delivery.error}>{delivery.error}</span> : null}
      <span className="flex-1" />
      <span className="hidden max-w-52 truncate text-xs text-muted-foreground lg:block">{delivery.repository ?? delivery.accountLogin ?? "GitHub App"}</span>
      <Tooltip content={formatDateTime(delivery.receivedAt)}><span className="tabular relative w-10 text-right text-xs text-faint">{formatAge(delivery.receivedAt)}</span></Tooltip>
      <span className="relative flex w-7 justify-end">
        {delivery.canRetry && delivery.status === "failed" && delivery.signatureValid ? (
          <Tooltip content="Retry delivery">
            <Button aria-label="Retry delivery" onClick={() => void run({ action: () => retryWebhookAction({ data: { deliveryId: delivery.id } }), success: "Delivery reprocessed." })} size="icon-xs" variant="ghost"><RefreshCw /></Button>
          </Tooltip>
        ) : null}
      </span>
    </div>
  );
}

function PayloadSheet({ state }: { state: PayloadState }) {
  const { delivery, payload, loading, error } = state;
  const formatted = payload?.payload == null ? "" : JSON.stringify(payload.payload, null, 2);
  return (
    <SheetContent
      actions={formatted ? <Button onClick={() => void navigator.clipboard.writeText(formatted).then(() => toast.success("Payload copied."))} size="sm" variant="outline"><Copy />Copy JSON</Button> : null}
      aria-describedby={undefined}
      description={`${delivery.id}${payload ? ` · ${formatBytes(payload.payloadBytes)}` : ""}`}
      heading={<span className="font-mono">{delivery.event}{delivery.action ? ` · ${delivery.action}` : ""}</span>}
    >
      <div className="space-y-4 p-4">
        <dl className="grid grid-cols-2 gap-x-6 gap-y-2 text-sm">
          <dt className="text-muted-foreground">Status</dt><dd><StatusBadge status={delivery.status} /></dd>
          <dt className="text-muted-foreground">Signature</dt><dd>{delivery.signatureValid ? "Verified" : <span className="text-danger">Invalid</span>}</dd>
          <dt className="text-muted-foreground">Destination</dt><dd className="truncate">{delivery.repository ?? delivery.accountLogin ?? "GitHub App"}</dd>
          <dt className="text-muted-foreground">Received</dt><dd>{formatDateTime(delivery.receivedAt)}</dd>
          {delivery.processedAt ? <><dt className="text-muted-foreground">Processed</dt><dd>{formatDateTime(delivery.processedAt)}</dd></> : null}
        </dl>
        {delivery.error ? <Callout title="Processing error" tone="danger"><span className="whitespace-pre-wrap break-words font-mono">{delivery.error}</span></Callout> : null}
        <div className="overflow-hidden rounded-lg border border-border bg-[var(--log-background)]">
          {loading ? <div className="flex items-center gap-2 p-6 text-sm text-muted-foreground"><LoaderCircle className="size-4 animate-spin" />Loading stored payload…</div>
            : error ? <div className="p-4 text-sm text-danger">{error}</div>
              : formatted ? <pre className="max-h-[60vh] overflow-auto p-4 font-mono text-xs leading-5 text-[#d0d6e0]"><code>{formatted}</code></pre>
                : <div className="p-6 text-sm text-muted-foreground">No request payload was retained for this delivery.</div>}
        </div>
      </div>
    </SheetContent>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  return `${(bytes / 1_048_576).toFixed(1)} MB`;
}
