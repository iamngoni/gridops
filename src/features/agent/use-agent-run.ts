import { useCallback, useEffect, useRef, useState } from "react";

import { AGENT_RUN_POLL_MS, isTerminalAgentStatus } from "./agent-state";
import { type AgentRun, type AgentRunStatus, getAgentRun } from "./agent.functions";

type Entry = { run: AgentRun | null; error: string | null };

/**
 * One agent run, re-read every few seconds while it's queued or running and the
 * page is visible. `onSettled` fires once when a run this hook watched finishes.
 */
export function useAgentRun(agentRunId: string | null, { onSettled }: { onSettled?: (run: AgentRun) => void } = {}) {
  // Keyed by id so a late answer for a run the viewer moved away from can't replace the current one.
  const [entries, setEntries] = useState<Record<string, Entry>>({});
  const entry = agentRunId ? entries[agentRunId] : undefined;
  const run = entry?.run ?? null;
  const error = entry?.error ?? null;
  const lastStatus = useRef(new Map<string, AgentRunStatus>());
  const loading = useRef(new Set<string>());
  const settled = useRef(onSettled);

  useEffect(() => {
    settled.current = onSettled;
  }, [onSettled]);

  /** Records a run read from any endpoint (load, start, cancel). */
  const accept = useCallback((next: AgentRun) => {
    const previous = lastStatus.current.get(next.id);
    lastStatus.current.set(next.id, next.status);
    setEntries((current) => ({ ...current, [next.id]: { run: next, error: null } }));
    if (previous && !isTerminalAgentStatus(previous) && isTerminalAgentStatus(next.status)) settled.current?.(next);
  }, []);

  const load = useCallback(async () => {
    if (!agentRunId || loading.current.has(agentRunId)) return;
    loading.current.add(agentRunId);
    try {
      accept(await getAgentRun(agentRunId));
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : "Could not load the agent run.";
      setEntries((current) => ({ ...current, [agentRunId]: { run: current[agentRunId]?.run ?? null, error: message } }));
    } finally {
      loading.current.delete(agentRunId);
    }
  }, [accept, agentRunId]);

  useEffect(() => {
    if (!agentRunId) return undefined;
    const initial = window.setTimeout(() => void load(), 0);
    return () => window.clearTimeout(initial);
  }, [agentRunId, load]);

  // Poll while the run is unfinished; a first load that failed waits for Retry instead.
  const polling = Boolean(agentRunId) && (run ? !isTerminalAgentStatus(run.status) : !error);
  useEffect(() => {
    if (!polling) return undefined;
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") void load();
    }, AGENT_RUN_POLL_MS);
    return () => window.clearInterval(interval);
  }, [load, polling]);

  return { run, error, accept, reload: load };
}
