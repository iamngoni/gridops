import { createFileRoute } from "@tanstack/react-router";
import { ExternalLink, Lock, PackageSearch, RefreshCw } from "lucide-react";

import { AsyncActionButton } from "~/components/async-action-button";
import { ResourcePage } from "~/components/resource-page";
import { Badge } from "~/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getRepositoriesPage, syncGitHubAction } from "~/features/operations/operations.functions";
import { formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/repositories")({
  loader: () => getRepositoriesPage(),
  component: RepositoriesPage,
});

function RepositoriesPage() {
  const data = Route.useLoaderData();
  const sync = syncGitHubAction;

  return (
    <ResourcePage
      title="Repositories"
      description="Repositories available through your GitHub App installations."
      icon={PackageSearch}
      emptyTitle={data.authenticated ? "No repositories in this installation" : "No repositories connected"}
      emptyDescription="Authorize GridOps and install the GitHub App on the repositories or organizations you want to operate."
      action="Create runner pool"
      actionHref="/runner-pools/new"
    >
      {data.items.length > 0 ? (
        <Card>
          <CardHeader>
            <div><CardTitle>Connected repositories</CardTitle><p className="mt-1 text-xs text-muted-foreground">{data.items.length} repositories across your installations</p></div>
            <AsyncActionButton action={() => sync()} icon={<RefreshCw />} success="GitHub installations and repositories synced.">Sync GitHub</AsyncActionButton>
          </CardHeader>
          <CardContent className="px-0 pb-0">
            <Table>
              <TableHeader><TableRow>
                <TableHead>Repository</TableHead><TableHead>Installation</TableHead><TableHead>Default branch</TableHead>
                <TableHead>Runner pools</TableHead><TableHead>Runs</TableHead><TableHead>Last sync</TableHead><TableHead />
              </TableRow></TableHeader>
              <TableBody>{data.items.map((repository) => (
                <TableRow key={repository.id}>
                  <TableCell>
                    <div className="flex items-center gap-2 font-medium">{repository.fullName}{repository.private ? <Lock className="size-3 text-muted-foreground" /> : null}</div>
                    <div className="mt-1 flex gap-1">{repository.archived ? <Badge variant="warning">archived</Badge> : <Badge variant="success">active</Badge>}<Badge variant="outline">{repository.permission ?? "installed"}</Badge></div>
                  </TableCell>
                  <TableCell><div className="text-xs">{repository.accountLogin}</div><div className="mt-1 text-[11px] text-muted-foreground">{repository.accountType} · {repository.repositorySelection}</div></TableCell>
                  <TableCell className="font-mono text-xs">{repository.defaultBranch}</TableCell>
                  <TableCell>{repository.poolCount}</TableCell>
                  <TableCell><div>{repository.runCount}</div><div className="mt-1 text-[11px] text-muted-foreground">{repository.lastRunAt ? formatRelativeTime(String(repository.lastRunAt)) : "No runs"}</div></TableCell>
                  <TableCell className="text-xs text-muted-foreground">{formatRelativeTime(String(repository.lastSyncedAt))}</TableCell>
                  <TableCell><a aria-label={`Open ${repository.fullName} on GitHub`} className="text-muted-foreground hover:text-foreground" href={String(repository.htmlUrl)} rel="noreferrer" target="_blank"><ExternalLink className="size-4" /></a></TableCell>
                </TableRow>
              ))}</TableBody>
            </Table>
          </CardContent>
        </Card>
      ) : undefined}
    </ResourcePage>
  );
}
