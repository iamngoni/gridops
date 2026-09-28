import { Link } from "@tanstack/react-router";
import { GitBranch } from "lucide-react";
import type * as React from "react";

import { listRowClassName } from "~/components/page";
import { RunStatusIcon, statusLabel } from "~/components/status-icon";
import { Avatar, githubAvatar } from "~/components/ui/avatar";
import { Tooltip } from "~/components/ui/tooltip";
import { cn, formatAge, formatDuration } from "~/lib/utils";

export type RunRowData = {
  id: number;
  workflow: string;
  repository: string;
  runNumber?: number;
  branch: string | null;
  status: string;
  conclusion: string | null;
  actor?: string | null;
  startedAt: string | null;
  completedAt: string | null;
  createdAt?: string | null;
};

/** One workflow run in Linear's issue-row shape: status, identifier, title, then metadata on the right. */
export function RunRow({ run, meta, trailing, className }: { run: RunRowData; meta?: React.ReactNode; trailing?: React.ReactNode; className?: string }) {
  const state = run.conclusion ?? run.status;
  const repositoryName = run.repository.split("/").pop() ?? run.repository;
  return (
    <div className={cn(listRowClassName, "relative gap-3 pr-2 sm:pr-4", className)}>
      <Link aria-label={`${run.workflow} in ${run.repository}`} className="absolute inset-0 z-0 outline-none" data-list-row="" params={{ runId: String(run.id) }} to="/workflow-runs/$runId" />
      <Tooltip content={statusLabel(state)}><span className="relative z-[1] inline-flex"><RunStatusIcon status={state} /></span></Tooltip>
      <span className="hidden w-40 shrink-0 truncate text-xs text-muted-foreground sm:block" title={run.repository}>
        {repositoryName}{run.runNumber ? <span className="text-faint"> #{run.runNumber}</span> : null}
      </span>
      <span className="min-w-0 flex-1 truncate font-medium text-foreground">{run.workflow}</span>
      {meta}
      {run.branch ? (
        <span className="hidden max-w-44 shrink-0 items-center gap-1 truncate rounded-full border border-border-strong px-2 py-0.5 font-mono text-2xs text-muted-foreground md:inline-flex" title={run.branch}>
          <GitBranch className="size-3 shrink-0" /><span className="truncate">{run.branch}</span>
        </span>
      ) : null}
      <span className="tabular hidden w-16 shrink-0 text-right text-xs text-muted-foreground sm:block">{formatDuration(run.startedAt, run.completedAt)}</span>
      {run.actor !== undefined ? (
        <Tooltip content={run.actor ? `@${run.actor}` : "GitHub Actions"}>
          <span className="relative z-[1] inline-flex"><Avatar name={run.actor ?? "github"} size={18} src={githubAvatar(run.actor)} /></span>
        </Tooltip>
      ) : null}
      {run.createdAt ? <span className="tabular w-14 shrink-0 text-right text-xs text-faint">{formatAge(run.createdAt)}</span> : null}
      {trailing ? <div className="relative z-[1] flex shrink-0 items-center">{trailing}</div> : null}
    </div>
  );
}
