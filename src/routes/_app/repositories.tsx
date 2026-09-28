import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ExternalLink, GitBranch, Lock, PackageSearch, Plus, RefreshCw, Search, X } from "lucide-react";
import { useEffect, useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { EmptyState, ListGroup, LoadingRows, PageBody, PageHeader, PageToolbar, listRowClassName } from "~/components/page";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";
import { type Repository, getRepositoriesPage } from "~/features/operations/operations.functions";
import { parsePage } from "~/lib/pagination";
import { cn, formatAge } from "~/lib/utils";

export const Route = createFileRoute("/_app/repositories")({
  validateSearch: (search: Record<string, unknown>) => ({
    q: typeof search.q === "string" ? search.q.slice(0, 100) : "",
    page: parsePage(search.page),
  }),
  component: RepositoriesPage,
});

function RepositoriesPage() {
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const repositories = useQuery({
    queryKey: ["repositories", search.q, search.page],
    queryFn: () => getRepositoriesPage({ query: search.q, page: search.page }),
    placeholderData: keepPreviousData,
  });
  const data = repositories.data;
  const groups = groupByAccount(data?.items ?? []);
  const [draft, setDraft] = useState(search.q);

  useEffect(() => {
    const trimmed = draft.trim();
    if (trimmed === search.q) return undefined;
    const timeout = window.setTimeout(() => void navigate({ search: { q: trimmed, page: 1 } }), 280);
    return () => window.clearTimeout(timeout);
  }, [draft, navigate, search.q]);

  return (
    <>
      <PageHeader
        actions={
          <>
            <Button disabled={repositories.isFetching} onClick={() => void repositories.refetch()} size="sm" variant="ghost">
              <RefreshCw className={cn(repositories.isFetching && "animate-spin")} /><span className="hidden sm:inline">Refresh</span>
            </Button>
            <Link className={buttonVariants({ size: "sm" })} to="/runner-pools/new"><Plus />New pool</Link>
          </>
        }
        count={data?.total || undefined}
        icon={PackageSearch}
        title="Repositories"
      />
      <PageToolbar>
        <RepositoryFilter onChange={setDraft} value={draft} />
      </PageToolbar>
      <PageBody className={cn("transition-opacity", repositories.isPlaceholderData && "opacity-60")}>
        {repositories.isError && !data ? (
          <EmptyState description={repositories.error instanceof Error ? repositories.error.message : "The live GitHub request failed."} icon={PackageSearch} title="Repositories could not be loaded">
            <Button onClick={() => void repositories.refetch()} variant="outline"><RefreshCw />Try again</Button>
          </EmptyState>
        ) : !data ? <LoadingRows /> : data.items.length === 0 ? (
          <EmptyState
            description={search.q ? "Try a different owner or repository name." : "Install the GitHub App on the repositories or organizations you want GridOps to operate."}
            icon={PackageSearch}
            title={search.q ? `No repositories match “${search.q}”` : "No repositories yet"}
          >
            {search.q ? <Button onClick={() => setDraft("")} variant="outline">Clear filter</Button> : null}
          </EmptyState>
        ) : (
          <>
            {groups.map((group) => (
              <ListGroup count={group.items.length} icon={<Avatar name={group.account} size={16} square src={githubAvatar(group.account)} />} key={group.account} label={group.account}>
                {group.items.map((repository) => <RepositoryRow key={repository.id} repository={repository} />)}
              </ListGroup>
            ))}
            <ListPagination itemCount={data.items.length} noun="repositories" onPageChange={(page) => void navigate({ search: { q: search.q, page } })} page={data.page} perPage={data.perPage} total={data.total} />
          </>
        )}
      </PageBody>
    </>
  );
}

function groupByAccount(items: Repository[]) {
  const groups = new Map<string, Repository[]>();
  for (const repository of items) groups.set(repository.accountLogin, [...(groups.get(repository.accountLogin) ?? []), repository]);
  return [...groups.entries()].map(([account, groupItems]) => ({ account, items: groupItems }));
}

function RepositoryFilter({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  return (
    <label className="relative flex h-7 w-full max-w-sm items-center">
      <Search className="pointer-events-none absolute left-2 size-3.5 text-muted-foreground" />
      <input
        aria-label="Filter repositories"
        className="h-7 w-full rounded-md border border-transparent bg-transparent pl-7 pr-7 text-sm text-foreground outline-none placeholder:text-faint hover:bg-hover focus:border-border-strong focus:bg-panel"
        maxLength={100}
        onChange={(event) => onChange(event.target.value)}
        placeholder="Filter by owner or name…"
        value={value}
      />
      {value ? (
        <button aria-label="Clear filter" className="absolute right-1.5 grid size-5 place-items-center rounded text-muted-foreground hover:bg-selected hover:text-foreground" onClick={() => onChange("")} type="button"><X className="size-3" /></button>
      ) : null}
    </label>
  );
}

function RepositoryRow({ repository }: { repository: Repository }) {
  const [owner, name] = repository.fullName.split("/");
  return (
    <div className={cn(listRowClassName, "pr-2")}>
      <span className="min-w-0 shrink truncate">
        <span className="text-muted-foreground">{owner}/</span><span className="font-medium text-foreground">{name}</span>
      </span>
      {repository.private ? <Tooltip content="Private repository"><Lock className="size-3.5 shrink-0 text-faint" /></Tooltip> : null}
      {repository.archived ? <Badge dot="bg-warning">Archived</Badge> : null}
      {repository.connected ? <Badge dot="bg-primary">In pool</Badge> : null}
      <span className="flex-1" />
      <span className="hidden max-w-36 items-center gap-1 truncate font-mono text-2xs text-muted-foreground md:flex"><GitBranch className="size-3 shrink-0" />{repository.defaultBranch}</span>
      <Link className="hidden w-20 shrink-0 rounded-md px-1.5 py-0.5 text-right text-xs text-muted-foreground hover:bg-selected hover:text-foreground lg:block" to="/runner-pools">{repository.poolCount} {repository.poolCount === 1 ? "pool" : "pools"}</Link>
      <Link className="tabular hidden w-28 shrink-0 rounded-md px-1.5 py-0.5 text-right text-xs text-muted-foreground hover:bg-selected hover:text-foreground sm:block" to="/workflow-runs">
        {repository.runCount} {repository.runCount === 1 ? "run" : "runs"}{repository.lastRunAt ? <span className="text-faint"> · {formatAge(repository.lastRunAt)}</span> : null}
      </Link>
      <Tooltip content="Open on GitHub">
        <a aria-label={`Open ${repository.fullName} on GitHub`} className={buttonVariants({ size: "icon-xs", variant: "ghost" })} href={repository.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink /></a>
      </Tooltip>
    </div>
  );
}
