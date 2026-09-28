import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { Boxes, Plus } from "lucide-react";

import { CapacityMeter } from "~/components/capacity-meter";
import { ListPagination } from "~/components/list-pagination";
import { EmptyState, ListGroup, PageBody, PageHeader, listRowClassName } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { StatusDot, statusLabel } from "~/components/status-icon";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Badge } from "~/components/ui/badge";
import { buttonVariants } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";
import { type RunnerPool, getRunnerPoolsPage } from "~/features/operations/operations.functions";
import { PoolActionsMenu, providerLabel } from "~/features/runner-pools/pool-actions";
import { validatePageSearch } from "~/lib/pagination";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/_app/runner-pools/")({
  validateSearch: validatePageSearch,
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ deps }) => getRunnerPoolsPage({ page: deps.page }),
  pendingComponent: () => <ResourcePageLoading icon={Boxes} title="Runner pools" />,
  component: RunnerPoolsPage,
});

const GROUPS = [
  { key: "attention", label: "Needs attention", tone: "danger" },
  { key: "waiting", label: "Waiting for capacity", tone: "warning" },
  { key: "changing", label: "Scaling or updating", tone: "progress" },
  { key: "active", label: "Active", tone: "success" },
  { key: "paused", label: "Paused", tone: "neutral" },
] as const;

function poolGroup(pool: RunnerPool): (typeof GROUPS)[number]["key"] {
  if (pool.paused || pool.state === "paused" || pool.state === "provisioning-paused") return "paused";
  if (pool.provisionCircuitOpen || pool.failedRunners > 0 || pool.state === "blocked") return "attention";
  if (pool.state === "waiting" || pool.state === "backoff") return "waiting";
  if (["scaling", "updating", "draining"].includes(pool.state) || pool.outdatedRunners > 0) return "changing";
  return "active";
}

function RunnerPoolsPage() {
  const data = Route.useLoaderData();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(5_000, data.authenticated);
  const grouped = GROUPS.map((group) => ({ ...group, pools: data.items.filter((pool) => poolGroup(pool) === group.key) })).filter((group) => group.pools.length);

  return (
    <>
      <PageHeader
        actions={<Link className={buttonVariants({ size: "sm" })} to="/runner-pools/new"><Plus />New pool</Link>}
        count={data.total || undefined}
        icon={Boxes}
        title="Runner pools"
      />
      <PageBody>
        {data.items.length === 0 ? (
          <EmptyState
            description={data.authenticated
              ? "A pool defines where runners register, what they run on, and how far they may scale."
              : "Authorize the GitHub App, then choose repositories or an organization for your first pool."}
            icon={Boxes}
            title={data.authenticated ? "Create your first runner pool" : "Connect GitHub to manage runners"}
          >
            <Link className={buttonVariants()} to="/runner-pools/new"><Plus />New pool</Link>
          </EmptyState>
        ) : (
          <>
            {grouped.map((group) => (
              <ListGroup count={group.pools.length} icon={<StatusDot pulse={false} tone={group.tone} />} key={group.key} label={group.label}>
                {group.pools.map((pool) => <PoolRow key={pool.id} pool={pool} />)}
              </ListGroup>
            ))}
            <ListPagination itemCount={data.items.length} noun="runner pools" onPageChange={(page) => void navigate({ search: { page } })} page={data.page} perPage={data.perPage} total={data.total} />
          </>
        )}
      </PageBody>
    </>
  );
}

function PoolRow({ pool }: { pool: RunnerPool }) {
  const providers = pool.providers?.length ? pool.providers : [pool.provider];
  const destination = pool.scope === "repository" && pool.repositoryCount > 1 ? `${pool.repositoryCount} repositories` : pool.repository ?? pool.accountLogin;
  const state = pool.paused ? "paused" : pool.state;
  return (
    <div className={cn(listRowClassName, "relative pr-2")}>
      <Link aria-label={`Open ${pool.name}`} className="absolute inset-0" params={{ poolId: pool.id }} to="/runner-pools/$poolId" />
      <Tooltip content={statusLabel(state)}><span className="relative inline-flex size-3.5 items-center justify-center"><StatusDot status={state} /></span></Tooltip>
      <span className="min-w-0 shrink truncate font-medium">{pool.name}</span>
      <span className="hidden min-w-0 items-center gap-1 overflow-hidden md:flex">
        {providers.map((provider) => <Badge dot={provider === "tart" ? "bg-[#bb87fc]" : "bg-info"} key={provider}>{providerLabel(provider, pool.macosRuntime)}</Badge>)}
        {pool.labels.slice(0, 2).map((label) => <Badge key={label} variant="outline">{label}</Badge>)}
        {pool.labels.length > 2 ? <span className="text-2xs text-faint">+{pool.labels.length - 2}</span> : null}
      </span>
      {pool.provisionCircuitOpen ? <Badge className="hidden lg:inline-flex" variant="destructive">Paused after {pool.provisionFailureCount} failures</Badge> : null}
      {pool.outdatedRunners > 0 ? <Badge className="hidden lg:inline-flex" dot="bg-warning">{pool.outdatedRunners} updating</Badge> : null}
      <span className="flex-1" />
      <span className="hidden w-40 shrink-0 items-center gap-1.5 truncate text-xs text-muted-foreground xl:flex" title={destination}>
        <Avatar name={pool.accountLogin} size={16} square src={githubAvatar(pool.accountLogin)} />
        <span className="truncate">{destination}</span>
      </span>
      <span className="tabular hidden w-24 shrink-0 text-right text-xs text-muted-foreground lg:block">{pool.cpuLimit} CPU · {pool.memoryLimitMb >= 1024 ? `${Math.round(pool.memoryLimitMb / 102.4) / 10} GB` : `${pool.memoryLimitMb} MB`}</span>
      <CapacityMeter busy={pool.busyRunners} desired={pool.desiredCount} online={pool.onlineRunners} />
      <span className="relative flex w-7 shrink-0 justify-end">{pool.canManage ? <PoolActionsMenu pool={pool} /> : <Tooltip content="Read only"><span className="size-1.5 rounded-full bg-faint" /></Tooltip>}</span>
    </div>
  );
}
