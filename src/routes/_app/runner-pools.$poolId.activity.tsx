import { createFileRoute, getRouteApi } from "@tanstack/react-router";
import { Activity } from "lucide-react";
import { useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { Callout, EmptyState, LoadingRows } from "~/components/page";
import { PoolEventTable, usePoolEvents } from "~/features/runner-pools/pool-events";

export const Route = createFileRoute("/_app/runner-pools/$poolId/activity")({
  component: PoolActivityTab,
});

const poolRoute = getRouteApi("/_app/runner-pools/$poolId");

function PoolActivityTab() {
  const pool = poolRoute.useLoaderData();
  const [page, setPage] = useState(1);
  const state = usePoolEvents(pool.id, page);

  if (state.status === "loading") return <LoadingRows />;
  if (state.status === "error") return <div className="p-4"><Callout title="Activity could not be loaded" tone="danger">{state.error}</Callout></div>;
  if (state.data.items.length === 0) {
    return <EmptyState className="min-h-[50vh]" description="Provisioning, capacity, autoscaling, and routing events for this pool appear here." icon={Activity} title="No activity yet" />;
  }
  return (
    <>
      <PoolEventTable events={state.data.items} />
      <ListPagination itemCount={state.data.items.length} noun="events" onPageChange={setPage} page={state.data.page} perPage={state.data.perPage} total={state.data.total} />
    </>
  );
}
