import { createFileRoute, getRouteApi } from "@tanstack/react-router";
import { Activity } from "lucide-react";
import { useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { Callout, EmptyState, InlineLoading } from "~/components/page";
import { PoolEventTimeline, usePoolEvents } from "~/features/runner-pools/pool-events";

export const Route = createFileRoute("/_app/runner-pools/$poolId/activity")({
  component: PoolActivityTab,
});

const poolRoute = getRouteApi("/_app/runner-pools/$poolId");

function PoolActivityTab() {
  const pool = poolRoute.useLoaderData();
  const [page, setPage] = useState(1);
  const state = usePoolEvents(pool.id, page);

  return (
    <div className="mx-auto w-full max-w-[760px] px-5 py-8 sm:px-8">
      {state.status === "loading" ? <InlineLoading label="Loading pool activity…" /> : null}
      {state.status === "error" ? <Callout title="Activity could not be loaded" tone="danger">{state.error}</Callout> : null}
      {state.status === "ready" && state.data.items.length === 0 ? (
        <EmptyState className="min-h-[40vh]" description="Provisioning, capacity, autoscaling, and routing events for this pool will appear here." icon={Activity} title="No activity yet" />
      ) : null}
      {state.status === "ready" && state.data.items.length > 0 ? (
        <>
          <div className="mb-4 flex items-baseline justify-between">
            <h2 className="text-sm font-medium">Activity</h2>
            <span className="tabular text-xs text-faint">{state.data.total} events</span>
          </div>
          <PoolEventTimeline detailed events={state.data.items} />
          <div className="-mx-4 mt-4 border-t border-border">
            <ListPagination itemCount={state.data.items.length} noun="events" onPageChange={setPage} page={state.data.page} perPage={state.data.perPage} total={state.data.total} />
          </div>
        </>
      ) : null}
    </div>
  );
}
