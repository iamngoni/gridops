import { Link, createFileRoute } from "@tanstack/react-router";
import { CircleCheck, CircleX, ExternalLink, GitBranch, GitCommitHorizontal, GitPullRequestArrow, Terminal } from "lucide-react";

import { PageBody, PageHeader, PropertiesPanel, PropertyGroup, PropertyRow, SectionHeading, listRowClassName } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { RunStatusIcon, StatusBadge, statusLabel } from "~/components/status-icon";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Badge } from "~/components/ui/badge";
import { buttonVariants } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";
import { type WorkflowRunDetail, getWorkflowRunDetailAction } from "~/features/operations/operations.functions";
import { RunActionsMenu, isActiveRun } from "~/features/workflow-runs/run-actions";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";
import { cn, formatDateTime, formatDuration } from "~/lib/utils";

export const Route = createFileRoute("/_app/workflow-runs/$runId")({
  loader: ({ params }) => getWorkflowRunDetailAction({ data: { runId: Number(params.runId) } }),
  pendingComponent: () => <ResourcePageLoading icon={GitPullRequestArrow} title="Workflow run" />,
  component: WorkflowRunDetailPage,
});

type Job = WorkflowRunDetail["jobs"][number];

function WorkflowRunDetailPage() {
  const run = Route.useLoaderData();
  useLiveRouteRefresh(3_000, isActiveRun(run));
  const state = run.conclusion ?? run.status;
  const failed = run.jobs.filter((job) => job.conclusion === "failure").length;

  return (
    <>
      <PageHeader
        actions={
          <>
            <a className={buttonVariants({ size: "sm", variant: "ghost" })} href={run.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink /><span className="hidden sm:inline">GitHub</span></a>
            <RunActionsMenu run={run} showDownload triggerVariant="outline" />
          </>
        }
        breadcrumbs={[{ label: "Workflow runs", to: "/workflow-runs" }]}
        title={`${run.workflowName} #${run.runNumber}`}
      />
      <PageBody>
        <div className="flex min-h-full flex-col lg:flex-row">
          <div className="min-w-0 flex-1">
            <div className="px-5 py-8 sm:px-8">
              <div className="flex items-start gap-3">
                <RunStatusIcon className="mt-1.5" size={18} status={state} />
                <div className="min-w-0">
                  <h1 className="text-xl font-semibold tracking-[-0.01em] text-foreground">{run.workflowName}</h1>
                  <p className="mt-1 text-sm text-muted-foreground">{run.repository} · run #{run.runNumber}{run.runAttempt > 1 ? ` · attempt ${run.runAttempt}` : ""}</p>
                </div>
              </div>

              <div className="mt-8">
                <SectionHeading
                  actions={run.jobs.length ? <span className="tabular text-xs text-faint">{failed ? <span className="text-danger">{failed} failed · </span> : null}{run.jobs.length} jobs</span> : null}
                  className="mb-2"
                >
                  Jobs
                </SectionHeading>
                {run.jobs.length ? (
                  <div className="overflow-hidden rounded-lg border border-border [&>*:last-child]:border-b-0">
                    {run.jobs.map((job) => <JobRow job={job} key={job.id} />)}
                  </div>
                ) : (
                  <div className="rounded-lg border border-dashed border-border-strong px-4 py-10 text-center text-sm text-muted-foreground">Job details arrive through workflow job webhooks and polling.</div>
                )}
              </div>
            </div>
          </div>

          <PropertiesPanel>
            <PropertyGroup title="Run">
              <PropertyRow label="Status"><StatusBadge status={state} /></PropertyRow>
              <PropertyRow label="Repository" title={run.repository}><span className="truncate">{run.repository}</span></PropertyRow>
              <PropertyRow label="Branch" title={run.headBranch ?? "detached"}><GitBranch className="size-3.5 shrink-0 text-muted-foreground" /><span className="truncate font-mono text-xs">{run.headBranch ?? "detached"}</span></PropertyRow>
              <PropertyRow label="Commit"><GitCommitHorizontal className="size-3.5 shrink-0 text-muted-foreground" /><span className="font-mono text-xs">{run.headSha.slice(0, 7)}</span></PropertyRow>
              <PropertyRow label="Trigger"><span className="font-mono text-xs">{run.event}</span></PropertyRow>
              <PropertyRow label="Actor"><Avatar name={run.actorLogin ?? "github"} size={16} src={githubAvatar(run.actorLogin)} />{run.actorLogin ?? "GitHub Actions"}</PropertyRow>
            </PropertyGroup>
            <PropertyGroup title="Timing">
              <PropertyRow label="Created">{formatDateTime(run.createdAt)}</PropertyRow>
              <PropertyRow label="Started">{run.startedAt ? formatDateTime(run.startedAt) : "Not started"}</PropertyRow>
              <PropertyRow label="Duration"><span className="tabular">{formatDuration(run.startedAt, run.completedAt)}</span></PropertyRow>
              <PropertyRow label="Attempt"><span className="tabular">{run.runAttempt}</span></PropertyRow>
            </PropertyGroup>
          </PropertiesPanel>
        </div>
      </PageBody>
    </>
  );
}

function JobRow({ job }: { job: Job }) {
  const state = job.conclusion ?? job.status;
  const logTarget = job.liveRunnerId ?? job.archivedLogId ?? undefined;
  return (
    <div className="border-b border-border">
      <div className={cn(listRowClassName, "border-b-0")}>
        <Tooltip content={statusLabel(state)}><span className="inline-flex"><RunStatusIcon status={state} /></span></Tooltip>
        <span className="min-w-0 flex-1 truncate font-medium">{job.name}</span>
        <span className="hidden max-w-60 items-center gap-1 overflow-hidden md:flex">
          {job.labels.slice(0, 3).map((label) => <Badge key={label} variant="outline">{label}</Badge>)}
          {job.labels.length > 3 ? <span className="text-2xs text-faint">+{job.labels.length - 3}</span> : null}
        </span>
        {logTarget ? (
          <Link className="hidden max-w-44 truncate rounded-md px-1.5 py-0.5 font-mono text-2xs text-muted-foreground hover:bg-selected hover:text-foreground sm:block" search={{ target: logTarget }} to="/live-logs">{job.runnerName ?? "runner"}</Link>
        ) : <span className="hidden max-w-44 truncate font-mono text-2xs text-faint sm:block">{job.runnerName ?? "unassigned"}</span>}
        <span className="tabular w-14 shrink-0 text-right text-xs text-muted-foreground">{formatDuration(job.startedAt, job.completedAt)}</span>
        <span className="flex shrink-0 items-center gap-0.5">
          {logTarget ? (
            <Tooltip content="View logs"><Link aria-label={`View logs for ${job.name}`} className={buttonVariants({ size: "icon-xs", variant: "ghost" })} search={{ target: logTarget }} to="/live-logs"><Terminal /></Link></Tooltip>
          ) : null}
          <Tooltip content="Open on GitHub"><a aria-label={`Open ${job.name} on GitHub`} className={buttonVariants({ size: "icon-xs", variant: "ghost" })} href={job.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink /></a></Tooltip>
        </span>
      </div>
      {job.diagnosis ? <JobDiagnosis diagnosis={job.diagnosis} /> : null}
    </div>
  );
}

function JobDiagnosis({ diagnosis }: { diagnosis: NonNullable<Job["diagnosis"]> }) {
  return (
    <div className="mx-4 mb-3 ml-11 rounded-md border border-border-strong bg-panel-subtle px-3 py-2.5 text-xs leading-5">
      <p className="text-secondary-foreground"><span className="font-medium text-warning">Waiting · </span>{diagnosis.summary}</p>
      {diagnosis.candidates.length ? (
        <div className="mt-2 space-y-1 border-t border-border pt-2">
          {diagnosis.candidates.map((candidate) => (
            <div className="flex items-center gap-2" key={candidate.id}>
              {candidate.status === "ready" ? <CircleCheck className="size-3.5 shrink-0 text-success" /> : <CircleX className="size-3.5 shrink-0 text-faint" />}
              <Link className="font-medium hover:underline" params={{ poolId: candidate.id }} to="/runner-pools/$poolId">{candidate.name}</Link>
              <span className="text-muted-foreground">{candidate.status === "ready" ? `${candidate.availableCapacity} slot${candidate.availableCapacity === 1 ? "" : "s"} available` : candidate.status.replaceAll("_", " ")}</span>
              <span className="tabular ml-auto text-faint">{candidate.busyRunners}/{candidate.activeRunners} busy · max {candidate.maximumRunners}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
