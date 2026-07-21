import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { ExternalLink, LoaderCircle, Lock, PackageSearch, RefreshCw, Search, X } from "lucide-react";
import { type FormEvent, useState } from "react";

import { ListPagination } from "~/components/list-pagination";
import { ResourcePage } from "~/components/resource-page";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Input } from "~/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getRepositoriesPage } from "~/features/operations/operations.functions";
import { parsePage } from "~/lib/pagination";
import { cn, formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/repositories")({
  validateSearch: (search: Record<string, unknown>) => {
    return {
      q: typeof search.q === "string" ? search.q.slice(0, 100) : "",
      page: parsePage(search.page),
    };
  },
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

  function searchRepositories(query: string) {
    void navigate({ search: { q: query, page: 1 } });
  }

  function clearSearch() {
    searchRepositories("");
  }

  function goToPage(page: number) {
    void navigate({ search: { q: search.q, page } });
  }

  return (
    <ResourcePage
      title="Repositories"
      description="A live view of repositories available to your GitHub App installations."
      icon={PackageSearch}
      emptyTitle={data?.authenticated ? "No repositories in this installation" : "No repositories connected"}
      emptyDescription="Authorize GridOps and install the GitHub App on the repositories or organizations you want to operate."
      action="Create runner pool"
      actionHref="/runner-pools/new"
    >
      {repositories.isError && !data ? (
        <Card>
          <CardContent className="grid min-h-72 place-items-center p-6 text-center">
            <div className="max-w-sm">
              <PackageSearch className="mx-auto size-7 text-destructive" />
              <h2 className="mt-3 text-sm font-medium">Repositories could not be loaded</h2>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">
                {repositories.error instanceof Error ? repositories.error.message : "The live GitHub repository request failed."}
              </p>
              <Button className="mt-4" onClick={() => void repositories.refetch()} size="sm" variant="outline">
                <RefreshCw />Try again
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : !data ? (
        <RepositoryLoadingCard initialQuery={search.q} onSearch={searchRepositories} />
      ) : data.authenticated ? (
        <Card>
          <CardHeader className="flex-col md:flex-row md:items-center">
            <div>
              <CardTitle>Connected repositories</CardTitle>
              <p className="mt-1 text-xs text-muted-foreground">
                {search.q
                  ? `${data.total} repositories match “${search.q}”`
                  : `${data.total} repositories available across your installations`}
              </p>
            </div>
            <Button disabled={repositories.isFetching} onClick={() => void repositories.refetch()} size="sm" variant="outline">
              <RefreshCw className={cn(repositories.isFetching && "animate-spin")} />
              {repositories.isFetching ? "Refreshing…" : "Refresh from GitHub"}
            </Button>
          </CardHeader>
          <CardContent aria-busy={repositories.isFetching} className={cn("px-0 pb-0 transition-opacity", repositories.isPlaceholderData && "opacity-60")}>
            <RepositorySearchForm initialQuery={search.q} key={search.q} onSearch={searchRepositories} />

            {data.items.length > 0 ? (
              <Table>
                <TableHeader><TableRow>
                  <TableHead>Repository</TableHead><TableHead>Installation</TableHead><TableHead>Default branch</TableHead>
                  <TableHead>Runner pools</TableHead><TableHead>Runs</TableHead><TableHead>Source</TableHead><TableHead />
                </TableRow></TableHeader>
                <TableBody>{data.items.map((repository) => (
                  <TableRow key={repository.id}>
                    <TableCell>
                      <div className="flex items-center gap-2 font-medium">{repository.fullName}{repository.private ? <Lock className="size-3 text-muted-foreground" /> : null}</div>
                      <div className="mt-1 flex gap-1">
                        {repository.archived ? <Badge variant="warning">archived</Badge> : null}
                        <Badge variant={repository.connected ? "success" : "outline"}>{repository.connected ? "connected" : "available"}</Badge>
                        <Badge variant="outline">{repository.permission ?? "installed"}</Badge>
                      </div>
                    </TableCell>
                    <TableCell><div className="text-xs">{repository.accountLogin}</div><div className="mt-1 text-[11px] text-muted-foreground">{repository.accountType} · {repository.repositorySelection}</div></TableCell>
                    <TableCell className="font-mono text-xs">{repository.defaultBranch}</TableCell>
                    <TableCell>{repository.poolCount}</TableCell>
                    <TableCell><div>{repository.runCount}</div><div className="mt-1 text-[11px] text-muted-foreground">{repository.lastRunAt ? formatRelativeTime(String(repository.lastRunAt)) : "No runs"}</div></TableCell>
                    <TableCell className="text-xs text-muted-foreground">Live from GitHub</TableCell>
                    <TableCell><a aria-label={`Open ${repository.fullName} on GitHub`} className="text-muted-foreground hover:text-foreground" href={String(repository.htmlUrl)} rel="noreferrer" target="_blank"><ExternalLink className="size-4" /></a></TableCell>
                  </TableRow>
                ))}</TableBody>
              </Table>
            ) : (
              <div className="grid min-h-64 place-items-center px-6 py-12 text-center">
                <div>
                  <PackageSearch className="mx-auto size-7 text-muted-foreground" />
                  <h3 className="mt-3 text-sm font-medium">{search.q ? "No matching repositories" : "No repositories synchronized"}</h3>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {search.q ? "Try a different owner or repository name." : "Sync GitHub to refresh this installation."}
                  </p>
                  {search.q ? <Button className="mt-4" onClick={clearSearch} size="sm" variant="outline">Clear search</Button> : null}
                </div>
              </div>
            )}

            <ListPagination itemCount={data.items.length} noun="repositories" onPageChange={goToPage} page={data.page} perPage={data.perPage} total={data.total} />
          </CardContent>
        </Card>
      ) : undefined}
    </ResourcePage>
  );
}

function RepositoryLoadingCard({ initialQuery, onSearch }: { initialQuery: string; onSearch: (query: string) => void }) {
  return (
    <Card aria-busy="true" aria-live="polite">
      <CardHeader className="flex-col md:flex-row md:items-center">
        <div>
          <CardTitle>Connected repositories</CardTitle>
          <p className="mt-1 text-xs text-muted-foreground">Loading repositories from GitHub…</p>
        </div>
        <div className="inline-flex items-center gap-2 text-xs text-muted-foreground" role="status">
          <LoaderCircle className="size-4 animate-spin text-primary" />
          Fetching live data
        </div>
      </CardHeader>
      <CardContent className="px-0 pb-0">
        <RepositorySearchForm initialQuery={initialQuery} onSearch={onSearch} />
        <Table>
          <TableHeader><TableRow>
            <TableHead>Repository</TableHead><TableHead>Installation</TableHead><TableHead>Default branch</TableHead>
            <TableHead>Runner pools</TableHead><TableHead>Runs</TableHead><TableHead>Source</TableHead><TableHead />
          </TableRow></TableHeader>
          <TableBody>
            {Array.from({ length: 7 }, (_, index) => (
              <TableRow key={index}>
                {Array.from({ length: 7 }, (_, cell) => (
                  <TableCell key={cell}><div className="h-7 animate-pulse rounded bg-muted" /></TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}

function RepositorySearchForm({ initialQuery, onSearch }: { initialQuery: string; onSearch: (query: string) => void }) {
  const [query, setQuery] = useState(initialQuery);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSearch(query.trim());
  }

  function clear() {
    setQuery("");
    onSearch("");
  }

  return (
    <form className="flex flex-col gap-2 border-y border-border px-4 py-3 sm:flex-row" onSubmit={submit}>
      <div className="relative flex-1">
        <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          aria-label="Search repositories"
          className="pl-9 pr-9"
          maxLength={100}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search by owner or repository name…"
          value={query}
        />
        {query ? (
          <button
            aria-label="Clear repository search"
            className="absolute right-2 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
            onClick={clear}
            type="button"
          >
            <X className="size-3.5" />
          </button>
        ) : null}
      </div>
      <Button type="submit" variant="outline"><Search />Search</Button>
    </form>
  );
}
