import { Link, createFileRoute } from "@tanstack/react-router";
import { ExternalLink, FileArchive, GitBranch, GitPullRequestArrow } from "lucide-react";

import { AppShell } from "~/components/app-shell";
import { StatusBadge } from "~/components/status-badge";
import { Badge } from "~/components/ui/badge";
import { buttonVariants } from "~/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "~/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "~/components/ui/table";
import { getWorkflowRunDetailAction } from "~/features/operations/operations.functions";
import { formatDuration, formatRelativeTime } from "~/lib/utils";

export const Route = createFileRoute("/workflow-runs_/$runId")({
  loader: ({ params }) => getWorkflowRunDetailAction({ data: { runId: Number(params.runId) } }),
  component: WorkflowRunDetailPage,
});

function WorkflowRunDetailPage() {
  const run = Route.useLoaderData();
  return (
    <AppShell>
      <div className="space-y-5">
        <Link className="text-sm text-muted-foreground hover:text-foreground" to="/workflow-runs">← Workflow runs</Link>
        <div className="flex flex-col justify-between gap-4 lg:flex-row lg:items-end">
          <div><div className="flex items-center gap-2"><GitPullRequestArrow className="size-5 text-primary" /><h1 className="text-2xl font-semibold tracking-tight">{run.workflowName}</h1><StatusBadge status={run.conclusion ?? run.status} /></div><p className="mt-2 text-sm text-muted-foreground">{run.repository} · run #{run.runNumber} · attempt {run.runAttempt}</p></div>
          <div className="flex gap-2"><a className={buttonVariants({ variant: "outline" })} href={`/api/workflow-runs/${run.id}/logs`}><FileArchive />Download logs</a><a className={buttonVariants()} href={run.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink />Open on GitHub</a></div>
        </div>

        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <Summary label="Trigger" value={run.event} />
          <Summary label="Branch" value={run.headBranch ?? "detached"} icon={<GitBranch />} />
          <Summary label="Duration" value={formatDuration(run.startedAt, run.completedAt)} />
          <Summary label="Started" value={formatRelativeTime(run.createdAt)} />
        </div>

        <Card><CardHeader><CardTitle>Jobs</CardTitle></CardHeader><CardContent className="px-0 pb-0">
          {run.jobs.length ? <Table><TableHeader><TableRow><TableHead>Job</TableHead><TableHead>Runner</TableHead><TableHead>Labels</TableHead><TableHead>Duration</TableHead><TableHead>Status</TableHead><TableHead /></TableRow></TableHeader><TableBody>
            {run.jobs.map((job) => <TableRow key={job.id}>
              <TableCell className="font-medium">{job.name}</TableCell>
              <TableCell><div className="font-mono text-xs">{job.runnerName ?? "Unassigned"}</div><div className="mt-1 text-[11px] text-muted-foreground">{job.runnerGroupName ?? "—"}</div></TableCell>
              <TableCell><div className="flex max-w-80 flex-wrap gap-1">{job.labels.map((label) => <Badge key={label} variant="outline">{label}</Badge>)}</div></TableCell>
              <TableCell className="text-xs">{formatDuration(job.startedAt, job.completedAt)}</TableCell>
              <TableCell><StatusBadge status={job.conclusion ?? job.status} /></TableCell>
              <TableCell><a aria-label={`Open ${job.name} on GitHub`} className="text-muted-foreground hover:text-foreground" href={job.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink className="size-4" /></a></TableCell>
            </TableRow>)}
          </TableBody></Table> : <div className="grid min-h-56 place-items-center border-t border-border text-sm text-muted-foreground">Job details arrive through workflow job webhooks.</div>}
        </CardContent></Card>
      </div>
    </AppShell>
  );
}

function Summary({ label, value, icon }: { label: string; value: string; icon?: React.ReactNode }) {
  return <Card><CardContent className="p-4"><div className="flex items-center gap-2 text-xs text-muted-foreground">{icon}{label}</div><div className="mt-2 truncate font-medium">{value}</div></CardContent></Card>;
}
