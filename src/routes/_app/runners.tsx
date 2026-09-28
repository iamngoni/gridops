import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { Activity, Boxes } from "lucide-react";

import { ListPagination } from "~/components/list-pagination";
import { EmptyState, ListGroup, PageBody, PageHeader } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusBadge } from "~/components/status-icon";
import { buttonVariants } from "~/components/ui/button";
import { getRunnersPage } from "~/features/operations/operations.functions";
import { RunnerRow, groupRunners } from "~/features/runners/runner-row";
import { validatePageSearch } from "~/lib/pagination";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";

export const Route = createFileRoute("/_app/runners")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getRunnersPage({ page: deps.page }),
  pendingComponent: () => <ResourcePageLoading icon={Activity} title="Runners" />,
  component: RunnersPage,
});

function RunnersPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(5_000, data.authenticated);

  return (
    <>
      <PageHeader
        actions={<Link className={buttonVariants({ size: "sm", variant: "outline" })} to="/runner-pools"><Boxes />Runner pools</Link>}
        count={data.total || undefined}
        icon={Activity}
        title="Runners"
      />
      <PageBody>
        {data.items.length === 0 ? (
          <EmptyState description="Runners appear here as soon as a pool provisions its first execution environment." icon={Activity} title="No managed runners">
            <Link className={buttonVariants({ variant: "outline" })} to="/runner-pools"><Boxes />Manage runner pools</Link>
          </EmptyState>
        ) : (
          <>
            {groupRunners(data.items).map((group) => (
              <ListGroup count={group.runners.length} icon={<StatusBadge iconOnly status={group.status} />} key={group.key} label={group.label}>
                {group.runners.map((runner) => <RunnerRow key={runner.id} runner={runner} />)}
              </ListGroup>
            ))}
            <ListPagination itemCount={data.items.length} noun="runners" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} />
          </>
        )}
      </PageBody>
    </>
  );
}
