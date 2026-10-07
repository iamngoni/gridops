import { LoaderCircle, Sparkles } from "lucide-react";

import type { FixButtonState } from "./agent-state";
import { Button } from "~/components/ui/button";
import { Tooltip } from "~/components/ui/tooltip";

/** "Fix with agent" in a job's header; icon-only on phones. */
export function FixWithAgentButton({
  state,
  pending,
  onStart,
}: {
  state: Extract<FixButtonState, { visible: true }>;
  pending: boolean;
  onStart: () => void;
}) {
  const button = (
    <Button aria-label="Fix with agent" disabled={state.disabled || pending} onClick={onStart} size="sm" variant="outline">
      {pending ? <LoaderCircle className="animate-spin" /> : <Sparkles />}
      <span className="hidden sm:inline">{pending ? "Starting…" : "Fix with agent"}</span>
    </Button>
  );
  // A disabled button ignores the pointer, so the tooltip hangs off a focusable wrapper.
  if (state.disabled && !pending) {
    return <Tooltip content={state.reason}><span aria-label={`Fix with agent: ${state.reason ?? "unavailable"}`} className="inline-flex" role="group" tabIndex={0}>{button}</span></Tooltip>;
  }
  return <Tooltip content="Diagnose this failure in a sandbox and open a draft pull request with a fix">{button}</Tooltip>;
}
