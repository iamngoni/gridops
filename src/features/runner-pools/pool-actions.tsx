import { useNavigate } from "@tanstack/react-router";
import { ExternalLink, Minus, MoreHorizontal, Pause, Play, Plus, RefreshCw, RotateCcw, Settings2, Trash2 } from "lucide-react";

import { runnerPoolAction } from "./runner-pools.functions";
import { Button } from "~/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "~/components/ui/dropdown-menu";
import { useAction } from "~/lib/use-action";

export type PoolControlTarget = {
  id: string;
  name: string;
  desiredCount: number;
  minCount: number;
  maxCount: number;
  paused: boolean;
  provisionCircuitOpen: boolean;
};

export function PoolActionsMenu({ pool, showOpen = true, triggerVariant = "ghost" }: { pool: PoolControlTarget; showOpen?: boolean; triggerVariant?: "ghost" | "outline" }) {
  const run = useAction();
  const navigate = useNavigate();
  const control = runnerPoolAction;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button aria-label={`Actions for ${pool.name}`} className="data-[state=open]:bg-hover" size={triggerVariant === "ghost" ? "icon-xs" : "icon-sm"} variant={triggerVariant}>
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-56">
        {showOpen ? <DropdownMenuItem icon={<ExternalLink />} onSelect={() => void navigate({ to: "/runner-pools/$poolId", params: { poolId: pool.id } })}>Open pool</DropdownMenuItem> : null}
        <DropdownMenuItem icon={<Settings2 />} onSelect={() => void navigate({ to: "/runner-pools/$poolId/settings", params: { poolId: pool.id } })}>Edit configuration</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          disabled={pool.desiredCount >= pool.maxCount || pool.paused}
          icon={<Plus />}
          onSelect={() => void run({ action: () => control({ data: { action: "scale", poolId: pool.id, desiredCount: pool.desiredCount + 1 } }), success: `${pool.name} target raised to ${pool.desiredCount + 1}.` })}
        >
          Add a runner
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={pool.desiredCount <= pool.minCount || pool.paused}
          icon={<Minus />}
          onSelect={() => void run({ action: () => control({ data: { action: "scale", poolId: pool.id, desiredCount: pool.desiredCount - 1 } }), success: `${pool.name} target lowered to ${pool.desiredCount - 1}.` })}
        >
          Remove a runner
        </DropdownMenuItem>
        <DropdownMenuItem
          icon={pool.paused ? <Play /> : <Pause />}
          onSelect={() => void run({
            action: () => control({ data: { action: pool.paused ? "resume" : "pause", poolId: pool.id } }),
            confirm: pool.paused ? undefined : `Pause ${pool.name}? Idle runners are drained immediately.`,
            success: pool.paused ? `${pool.name} resumed.` : `${pool.name} is draining.`,
          })}
        >
          {pool.paused ? "Resume pool" : "Pause pool"}
        </DropdownMenuItem>
        {pool.provisionCircuitOpen ? (
          <DropdownMenuItem icon={<RotateCcw />} onSelect={() => void run({ action: () => control({ data: { action: "retry", poolId: pool.id } }), success: "Provisioning circuit reset." })}>
            Retry provisioning now
          </DropdownMenuItem>
        ) : null}
        <DropdownMenuItem icon={<RefreshCw />} onSelect={() => void run({ action: () => control({ data: { action: "reconcile", poolId: pool.id } }), success: `${pool.name} reconciled.` })}>
          Reconcile now
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          destructive
          icon={<Trash2 />}
          onSelect={() => void run({
            action: () => control({ data: { action: "delete", poolId: pool.id } }),
            after: () => navigate({ to: "/runner-pools" }),
            confirm: `Delete ${pool.name} and every runner it manages? This cannot be undone.`,
            success: `${pool.name} deleted.`,
          })}
        >
          Delete pool
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function providerLabel(provider: "docker" | "tart", runtime?: "vm" | "native") {
  if (provider === "docker") return "Linux";
  return runtime === "native" ? "macOS · native" : "macOS · VM";
}
