import { Outlet, createFileRoute, useRouterState } from "@tanstack/react-router";
import { Boxes } from "lucide-react";

import { PageBody, PageHeader, PageToolbar, ViewTabLink } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusDot } from "~/components/status-icon";
import { PoolActionsMenu } from "~/features/runner-pools/pool-actions";
import { getRunnerPoolAction } from "~/features/runner-pools/runner-pools.functions";

export const Route = createFileRoute("/_app/runner-pools/$poolId")({
  loader: ({ params }) => getRunnerPoolAction({ data: { poolId: params.poolId } }),
  pendingComponent: () => <ResourcePageLoading icon={Boxes} title="Runner pool" />,
  component: PoolLayout,
});

function PoolLayout() {
  const pool = Route.useLoaderData();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const tab = pathname.endsWith("/activity") ? "activity" : pathname.endsWith("/settings") ? "settings" : "overview";
  const params = { poolId: pool.id };

  return (
    <>
      <PageHeader
        actions={pool.canManage ? <PoolActionsMenu pool={pool} showOpen={false} triggerVariant="outline" /> : null}
        breadcrumbs={[{ label: "Runner pools", to: "/runner-pools" }]}
        documentTitle={pool.name}
        title={<span className="flex items-center gap-2"><StatusDot status={pool.paused ? "paused" : pool.state} />{pool.name}</span>}
      />
      <PageToolbar>
        <ViewTabLink active={tab === "overview"} params={params} to="/runner-pools/$poolId">Overview</ViewTabLink>
        <ViewTabLink active={tab === "activity"} params={params} to="/runner-pools/$poolId/activity">Activity</ViewTabLink>
        <ViewTabLink active={tab === "settings"} params={params} to="/runner-pools/$poolId/settings">Configuration</ViewTabLink>
      </PageToolbar>
      <PageBody>
        <Outlet />
      </PageBody>
    </>
  );
}
