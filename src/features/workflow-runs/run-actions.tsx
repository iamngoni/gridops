import { ExternalLink, FileArchive, MoreHorizontal, OctagonX, RefreshCw, RotateCcw, Square } from "lucide-react";

import { rowActionClassName } from "~/components/page";
import { Button } from "~/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "~/components/ui/dropdown-menu";
import { workflowRunAction } from "~/features/operations/operations.functions";
import { useAction } from "~/lib/use-action";

type RunTarget = { id: number; workflowName: string; runNumber: number; status: string; conclusion: string | null; htmlUrl: string; canManage: boolean };

export function isActiveRun(run: { status: string }) {
  return run.status === "queued" || run.status === "in_progress" || run.status === "waiting" || run.status === "pending";
}

export function RunActionsMenu({ run, triggerVariant = "ghost", showDownload = false }: { run: RunTarget; triggerVariant?: "ghost" | "outline"; showDownload?: boolean }) {
  const perform = useAction();
  const control = workflowRunAction;
  const active = isActiveRun(run);
  const label = `${run.workflowName} #${run.runNumber}`;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button aria-label={`Actions for ${label}`} className={triggerVariant === "ghost" ? rowActionClassName : "data-[state=open]:bg-hover"} size={triggerVariant === "ghost" ? "icon-xs" : "icon-sm"} variant={triggerVariant}><MoreHorizontal /></Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-56">
        {run.canManage && active ? (
          <>
            <DropdownMenuItem icon={<Square />} onSelect={() => void perform({ action: () => control({ data: { runId: run.id, action: "cancel" } }), confirm: `Cancel ${label}?`, confirmOptions: { confirmLabel: "Cancel run" }, success: "Cancellation requested." })}>Cancel run</DropdownMenuItem>
            <DropdownMenuItem destructive icon={<OctagonX />} onSelect={() => void perform({ action: () => control({ data: { runId: run.id, action: "force-cancel" } }), confirm: `Force-cancel ${label}? Use this only when normal cancellation is blocked.`, success: "Force cancellation requested." })}>Force cancel</DropdownMenuItem>
          </>
        ) : null}
        {run.canManage && !active ? <DropdownMenuItem icon={<RotateCcw />} onSelect={() => void perform({ action: () => control({ data: { runId: run.id, action: "rerun" } }), success: "Rerun requested." })}>Rerun all jobs</DropdownMenuItem> : null}
        {run.canManage && run.conclusion === "failure" ? <DropdownMenuItem icon={<RefreshCw />} onSelect={() => void perform({ action: () => control({ data: { runId: run.id, action: "rerun-failed" } }), success: "Failed jobs rerun requested." })}>Rerun failed jobs</DropdownMenuItem> : null}
        {run.canManage ? <DropdownMenuSeparator /> : null}
        {showDownload ? <DropdownMenuItem asChild icon={<FileArchive />}><a href={`/api/workflow-runs/${run.id}/logs`}>Download logs</a></DropdownMenuItem> : null}
        <DropdownMenuItem asChild icon={<ExternalLink />}><a href={run.htmlUrl} rel="noreferrer" target="_blank">Open on GitHub</a></DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
