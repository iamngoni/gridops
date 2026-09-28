import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { CircleCheck, CircleX, ExternalLink, GitBranch, GitCommitHorizontal, GitPullRequestArrow, Radio, X } from "lucide-react";

import { PageBody, PageHeader, PropertiesPanel, PropertyGroup, PropertyRow, SectionHeading, listRowClassName } from "~/components/page";
import { ResourcePageLoading } from "~/components/resource-page-loading";
import { RunStatusIcon, StatusBadge, statusLabel } from "~/components/status-icon";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Badge } from "~/components/ui/badge";
import { Button, buttonVariants } from "~/components/ui/button";
import { Dialog, DialogClose, SheetContent } from "~/components/ui/dialog";
import { Tooltip } from "~/components/ui/tooltip";
import { type WorkflowRunDetail, getWorkflowRunDetailAction } from "~/features/operations/operations.functions";
import { JobLogViewer } from "~/features/workflow-runs/job-log-viewer";
import { RunActionsMenu, isActiveRun } from "~/features/workflow-runs/run-actions";
import { useLiveRouteRefresh } from "~/lib/use-live-route-refresh";
import { cn, formatDateTime, formatDuration } from "~/lib/utils";

export const Route = createFileRoute("/_app/workflow-runs/$runId")({
  validateSearch: (search: Record<string, unknown>): { job?: number } => {
    const job = Number(search.job);
    return Number.isInteger(job) && job > 0 ? { job } : {};
  },
  loader: ({ params }) => getWorkflowRunDetailAction({ data: { runId: Number(params.runId) } }),
  pendingComponent: () => <ResourcePageLoading icon={GitPullRequestArrow} title="Workflow run" />,
  component: WorkflowRunDetailPage,
});

type Job = WorkflowRunDetail["jobs"][number];

function WorkflowRunDetailPage() {
  const run = Route.useLoaderData();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  useLiveRouteRefresh(3_000, isActiveRun(run));
  const openJob = run.jobs.find((job) => job.id === search.job);
  const showJob = (jobId: number | undefined) => void navigate({ search: jobId ? { job: jobId } : {}, replace: Boolean(search.job && jobId) });
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
                    {run.jobs.map((job) => <JobRow job={job} key={job.id} onOpen={() => showJob(job.id)} selected={job.id === search.job} />)}
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
      <Dialog onOpenChange={(open) => { if (!open) showJob(undefined); }} open={Boolean(openJob)}>
        {openJob ? (
          <SheetContent aria-describedby={undefined} bare className="max-w-[min(1040px,calc(100%-1rem))]" heading={`${openJob.name} logs`} onOpenAutoFocus={(event) => event.preventDefault()}>
            <JobLogViewer
              actions={
                <>
                  {openJob.liveRunnerId || openJob.archivedLogId ? (
                    <Tooltip content="Open in Live logs">
                      <Link aria-label="Open in Live logs" className={buttonVariants({ size: "icon-sm", variant: "ghost" })} search={{ target: openJob.liveRunnerId ?? openJob.archivedLogId ?? undefined }} to="/live-logs"><Radio /></Link>
                    </Tooltip>
                  ) : null}
                  <Tooltip content="Open on GitHub">
                    <a aria-label="Open job on GitHub" className={buttonVariants({ size: "icon-sm", variant: "ghost" })} href={openJob.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink /></a>
                  </Tooltip>
                  <DialogClose asChild><Button aria-label="Close logs" size="icon-sm" variant="ghost"><X /></Button></DialogClose>
                </>
              }
              fallback={{ name: openJob.name, status: openJob.conclusion ?? openJob.status, repository: run.repository, workflowName: run.workflowName, runNumber: run.runNumber, runId: run.id }}
              jobId={openJob.id}
              key={openJob.id}
              showRunLink={false}
            />
          </SheetContent>
        ) : null}
      </Dialog>
    </>
  );
}

function JobRow({ job, onOpen, selected }: { job: Job; onOpen: () => void; selected: boolean }) {
  const state = job.conclusion ?? job.status;
  const waiting = job.status === "queued" || job.status === "waiting";
  return (
    <div className="border-b border-border">
      <div className={cn(listRowClassName, "relative border-b-0", selected && "bg-selected")}>
        <button aria-label={`Show logs for ${job.name}`} className="absolute inset-0 outline-none" data-list-row="" onClick={onOpen} type="button" />
        <Tooltip content={statusLabel(state)}><span className="relative inline-flex"><RunStatusIcon status={state} /></span></Tooltip>
        <span className="min-w-0 flex-1 truncate font-medium">{job.name}</span>
        <span className="hidden max-w-60 items-center gap-1 overflow-hidden md:flex">
          {job.labels.slice(0, 3).map((label) => <Badge key={label} variant="outline">{label}</Badge>)}
          {job.labels.length > 3 ? <span className="text-2xs text-faint">+{job.labels.length - 3}</span> : null}
        </span>
        <span className="hidden max-w-44 truncate font-mono text-2xs text-faint sm:block">{job.runnerName ?? (waiting ? "no runner yet" : "—")}</span>
        {waiting ? (
          <Tooltip content="Time since the job was queued">
            <span className="tabular relative w-24 shrink-0 text-right text-xs text-warning">queued {formatDuration(job.startedAt, null)}</span>
          </Tooltip>
        ) : <span className="tabular w-24 shrink-0 text-right text-xs text-muted-foreground">{formatDuration(job.startedAt, job.completedAt)}</span>}
        <Tooltip content="Open on GitHub">
          <a aria-label={`Open ${job.name} on GitHub`} className={cn(buttonVariants({ size: "icon-xs", variant: "ghost" }), "relative")} href={job.htmlUrl} rel="noreferrer" target="_blank"><ExternalLink /></a>
        </Tooltip>
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
